/**
 * 文件接收端（K 通道的 `kbb`/`kbc` 帧）：按字符喂入键盘帧 → 落盘分片 →
 * 用二维码（Q 通道的 `QBB`）回报进度。
 *
 * # 与文本通道共用一条 keydown 流
 *
 * v2 里文本帧（`kba`）与文件帧（`kbb`/`kbc`）都以 `0` 开头、以 `1` 结尾，所以
 * 页面把每个字符**同时**喂给两条通道，靠 **magic** 分流：本模块只认 `0kbb`/`0kbc`，
 * 一旦发现第 4 个字符是 `a`（也就是 `0kba`）就自行复位、不再吃后续字符；
 * 文本通道反过来只认 `0kba`。两边各自校验前缀、各自复位，不需要一个中央路由器。
 *
 * # 会话身份：内容 + 分片大小，而不是文件名
 *
 * 临时分片目录的键是 `<fileMd5>-<chunkSize>`：
 *
 * 同名但内容不同 → `fileMd5` 不同 → 目录不同，**分片绝不混用**；
 * 同一文件但分片大小变了 → `chunkSize` 不同 → 旧分片不会被错当成新会话的片
 *   （解出来的字节边界完全不同，混用会拼出垃圾）；
 * 两者都相同 → 同一会话 → 正常续传（源文件改名也不影响）。
 *
 * 键是纯 `[a-z2-7]`+`-`+数字，**不需要任何路径消毒**。
 *
 * # 落盘时序
 *
 * 帧的 CRC 在解析阶段就验过了（`parseKFrame` 返回 null 的帧根本到不了这里），
 * 所以「校验通过才落盘」是天然成立的。收齐后：拼接 → 整体解压 → 校验长度 →
 * 校验整文件 MD5 → **不覆盖同名文件**地落盘 → 删临时目录 → 回报 complete。
 * 任何一步失败都**保留**临时分片（保留才能续传与诊断）。
 */

import type { KChunkFrame, KProbeFrame } from './protocol-k'
import type { Feedback } from './protocol-q'
import type { ReceiverStatusClass } from './receiver'
import * as fzstd from 'fzstd'
import { b32Decode } from './b32'
import { debug } from './debug'
import { md5B32 } from './md5'
import { parseKFrame } from './protocol-k'
import { buildFeedbackFrame } from './protocol-q'

// ---------------------------------------------------------------------------
// File System Access API 的类型声明
//
// TS 的默认 lib 里没有这套 API（它不在 ES 标准库里），而接收页又是纯浏览器代码。
// 只声明我们真正用到的部分，不引 `@types/wicg-file-system-access` —— 为一个页面
// 多加一个依赖不值得。
// ---------------------------------------------------------------------------

interface FsWritable {
  write: (data: Uint8Array) => Promise<void>
  close: () => Promise<void>
}

interface FsFileHandle {
  getFile: () => Promise<File>
  createWritable: () => Promise<FsWritable>
}

interface FsDirectoryHandle {
  getDirectoryHandle: (name: string, options?: { create?: boolean }) => Promise<FsDirectoryHandle>
  getFileHandle: (name: string, options?: { create?: boolean }) => Promise<FsFileHandle>
  removeEntry: (name: string, options?: { recursive?: boolean }) => Promise<void>
  entries: () => AsyncIterableIterator<[string, unknown]>
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

/** 空闲多久复位（毫秒）。比文本通道的 90s 宽松：文件帧更长，段间间隔更大。 */
const IDLE_TIMEOUT = 120_000
/** 临时目录名（放在用户选的目录下，写盘成功后清理）。 */
const TMP_DIR = '.clipbeam-tmp'
/** 单帧的合理上限（分片帧约 6.6K 字符）；超过说明不是本协议。 */
const MAX_FRAME_CHARS = 32_768
/** 文件帧的 magic 前缀（`0` 后面这两位必须一致）。 */
const FILE_MAGIC_PREFIX = 'kb'
/** 文件帧 magic 的第三个字符：`b`=探测、`c`=分片。`a` 是文本帧，不归本模块。 */
const FILE_MAGIC_KINDS = 'bc'

export interface TransferCallbacks {
  /** 状态条文案与样式。 */
  onStatus: (cls: ReceiverStatusClass, msg: string) => void
  /** 分片进度（已收到的片数 / 总片数；总数未知时为 0）。 */
  onProgress: (got: number, total: number) => void
  /** 把状态编成的二维码文本交给视图渲染（仅在状态变化时调用）。 */
  onQr: (text: string) => void
}

export interface TransferSession {
  /** 是否已授权保存目录。 */
  readonly authorized: boolean
  /** 让用户挑一个目录并授权（页面上的按钮）。返回是否拿到授权。 */
  pickDirectory: () => Promise<boolean>
  /**
   * 喂入一条按键字符。
   *
   * 返回 `true` 表示这一字符属于文件通道、已被本模块消费；`false` 表示不该由本模块
   * 接手（调用方仍应把它交给文本通道）。
   */
  accept: (ch: string) => boolean
  /** 复位到空闲（页面刷新按钮 / 空闲超时）。 */
  reset: () => void
  /** 空闲检测；建议每 500ms 调用一次。 */
  checkTimeout: (now?: number) => void
}

/** 一个已建立的会话（由探测帧 `kbb` 开出）。 */
interface ActiveSession {
  total: number
  /** **原始**文件字节数（用于校验解压后的长度）。 */
  size: number
  chunkSize: number
  zstd: boolean
  name: string
  /** base32(原始 16 字节) = 26 字符；同时是会话键的一部分。 */
  fileMd5: string
  /** 已落盘的分片序号。 */
  received: Set<number>
}

export function createTransferSession(cb: TransferCallbacks): TransferSession {
  let authorized = false
  let frameBuffer = ''
  let inFrame = false
  let idleAt = 0
  let session: ActiveSession | null = null
  let dir: FsDirectoryHandle | null = null
  let lastQr = ''
  /** 落盘/校验正在跑，避免重入（续传时会连续触发）。 */
  let busy = false

  /** 反馈二维码文本；相同内容不重绘（分片连续喂入，每片都重画会白白闪）。 */
  function emitQr(fb: Feedback): void {
    const text = buildFeedbackFrame(fb)
    if (text === lastQr)
      return
    lastQr = text
    cb.onQr(text)
    debug('transfer', '反馈二维码', fb, text)
  }

  function status(): Feedback {
    return { type: 'feedback', status: authorized ? 'ready' : 'unauthorized' }
  }

  function reset(): void {
    frameBuffer = ''
    inFrame = false
    session = null
    lastQr = ''
    busy = false
    cb.onProgress(0, 0)
    emitQr(status())
  }

  /** 会话键：内容 + 分片大小（见模块头）。 */
  function sessionKey(s: ActiveSession): string {
    return `${s.fileMd5}-${s.chunkSize}`
  }

  /** 缺失分片 → 区间清单（0-based 闭区间），用于回报并让宿主只补缺口。 */
  function missingRanges(s: ActiveSession): Array<[number, number]> {
    const out: Array<[number, number]> = []
    let start = -1
    for (let i = 0; i < s.total; i++) {
      if (!s.received.has(i)) {
        if (start < 0)
          start = i
      }
      else if (start >= 0) {
        out.push([start, i - 1])
        start = -1
      }
    }
    if (start >= 0)
      out.push([start, s.total - 1])
    return out
  }

  /** 回报当前进度：还有缺口就报 `partial` + 区间，齐了就交给 finalize。 */
  async function reportProgress(): Promise<void> {
    if (!session)
      return
    cb.onProgress(session.received.size, session.total)
    const missing = missingRanges(session)
    if (missing.length === 0) {
      debug('transfer', '分片已齐，开始拼装校验')
      await finalize()
      return
    }
    const text = formatRanges(missing)
    debug(
      'transfer',
      `进度 ${String(session.received.size)}/${String(session.total)}，缺口 ${text}`,
    )
    emitQr({
      type: 'feedback',
      status: 'partial',
      missing: text,
    })
  }

  /** 取（或建立）本次会话在磁盘上的临时分片目录。 */
  async function sessionDir(): Promise<FsDirectoryHandle> {
    if (!session)
      throw new Error('会话未建立')
    if (!dir)
      throw new Error('保存目录未授权')
    const root = await dir.getDirectoryHandle(TMP_DIR, { create: true })
    return root.getDirectoryHandle(sessionKey(session), { create: true })
  }

  /**
   * 分片在磁盘上的期望长度。
   *
   * 除最后一片外都必须**正好**是 `chunkSize`；最后一片是余数，只要求不超过它。
   * 之所以不按「文件总大小」精确算最后一片：`zstd` 时送上线的是**压缩流**，
   * 它的长度与原始大小无关，而探测帧里的 `size` 是**原始**大小（用来校验解压结果）。
   * 这一段长度校验抓的是「上次写到一半就被中断」留下的截断片 —— 那是最现实的坏片形态；
   * 剩下的交给整文件 MD5 兜底。
   */
  function expectedChunkLen(seq: number): { exact: boolean, len: number } | null {
    if (!session)
      return null
    if (seq < session.total - 1)
      return { exact: true, len: session.chunkSize }
    return { exact: false, len: session.chunkSize }
  }

  /**
   * 扫一遍临时目录，把上一次会话（页面刷新 / 上次中止）已经落下的分片算进进度。
   *
   * 这是断点续传的关键，而**校验每片长度**是「分片不错用」的另一半：长度不符
   * （上次崩溃留下的截断片）就删掉当没收到，免得拼出一条垃圾流。
   */
  async function restoreProgress(): Promise<void> {
    if (!session || !dir)
      return
    const key = sessionKey(session)
    try {
      const root = await dir.getDirectoryHandle(TMP_DIR)
      const sub = await root.getDirectoryHandle(sessionKey(session))
      let kept = 0
      let dropped = 0
      for await (const [entryName] of sub.entries()) {
        const match = /^p(\d+)$/.exec(entryName)
        if (!match)
          continue
        const seq = Number(match[1])
        if (!Number.isInteger(seq) || seq < 0 || seq >= session.total)
          continue
        const want = expectedChunkLen(seq)
        if (!want)
          continue
        const file = await (await sub.getFileHandle(entryName)).getFile()
        const ok = want.exact ? file.size === want.len : file.size <= want.len
        if (ok) {
          session.received.add(seq)
          kept++
        }
        else {
          // 上次写到一半被中断留下的截断片：删掉当没收到
          debug('transfer', `丢弃截断分片 p${String(seq)}（${String(file.size)} 字节，长度不符）`)
          await sub.removeEntry(entryName).catch(() => {})
          dropped++
        }
      }
      debug('transfer', `续传重建：目录 ${key}，复用 ${String(kept)} 片，丢弃 ${String(dropped)} 片`)
    }
    catch {
      // 目录不存在 = 全新会话，正常
      debug('transfer', `续传重建：目录 ${key} 不存在，按全新会话处理`)
    }
  }

  /** 落一片**线上原始字节**（base32 解码之后、解压之前）。 */
  async function writeChunk(seq: number, slice: Uint8Array): Promise<void> {
    const target = await sessionDir()
    const file = await target.getFileHandle(chunkFileName(seq), { create: true })
    const writable = await file.createWritable()
    await writable.write(slice)
    await writable.close()
  }

  /**
   * 全部收齐：拼接 → **整体**解压（`zstd` 时）→ 校验长度 → 校验整文件 MD5 →
   * 不覆盖同名地落盘 → 清理临时目录。
   *
   * 解压必须在这里做、且只做一次：发送端压的是**整个文件**，各分片只是同一条 zstd 流的
   * 片段，单独一段不是合法的 zstd 帧。
   */
  async function finalize(): Promise<void> {
    if (!session || !dir || busy)
      return
    busy = true
    try {
      const target = await sessionDir()
      debug('transfer', `开始拼装：${String(session.total)} 片，zstd=${String(session.zstd)}`)

      const parts: BlobPart[] = []
      for (let i = 0; i < session.total; i++) {
        const file = await target.getFileHandle(chunkFileName(i))
        parts.push(await file.getFile())
      }
      const stream = new Uint8Array(await new Blob(parts).arrayBuffer())
      debug('transfer', `拼接完成：线上 ${String(stream.byteLength)} 字节`)

      let bytes: Uint8Array
      if (session.zstd) {
        try {
          bytes = fzstd.decompress(stream)
        }
        catch (ex) {
          const detail = ex instanceof Error ? ex.message : String(ex)
          await fail(`整条 zstd 流解压失败：${detail}`)
          return
        }
      }
      else {
        bytes = stream
      }

      debug('transfer', `解压后 ${String(bytes.byteLength)} 字节（声明 ${String(session.size)}）`)
      // 解压结果必须正好等于发送端声明的原始大小 —— 不符说明流被截断或掺了别的数据
      if (bytes.byteLength !== session.size) {
        await fail(`大小不符（收到 ${bytes.byteLength}，应为 ${session.size}）`)
        return
      }
      // 整文件指纹：`fileMd5` 在帧里是 base32 形态，所以这里也编成 base32 再比
      const digestB32 = md5B32(bytes)
      debug('transfer', `整文件 MD5 ${digestB32}（帧里声明 ${session.fileMd5}）`)
      if (digestB32 !== session.fileMd5) {
        await fail(`整文件 MD5 不符（收到 ${digestB32}）`)
        return
      }

      // 同名不覆盖：内容相同则幂等（不重写），不同则退到 `name (1).ext`
      const resolved = await resolveTarget(dir, session.name, session.fileMd5)
      debug(
        'transfer',
        resolved.identical
          ? `目标「${resolved.name}」已存在且内容相同，跳过写入`
          : `落盘目标「${resolved.name}」`,
      )
      if (!resolved.identical) {
        const out = await dir.getFileHandle(resolved.name, { create: true })
        const writable = await out.createWritable()
        await writable.write(bytes)
        await writable.close()
      }

      // 只在写盘且校验通过之后清理临时分片
      await dir.removeEntry(TMP_DIR, { recursive: true }).catch(() => {})

      debug('transfer', `临时目录已清理：${TMP_DIR}`)
      const note = resolved.identical
        ? `✓ 远端已有同一文件「${resolved.name}」（内容一致，未重写）`
        : `✓ 已接收并保存「${resolved.name}」（${bytes.byteLength} 字节，MD5 校验通过）`
      cb.onStatus('ok', note)
      emitQr({ type: 'feedback', status: 'complete', saved: resolved.name })
    }
    catch (ex) {
      const message = ex instanceof Error ? ex.message : String(ex)
      await fail(`落盘失败：${message}`)
    }
    finally {
      busy = false
    }
  }

  /** 失败路径：**保留**临时分片（保留才能续传/诊断），报 error。 */
  async function fail(reason: string): Promise<void> {
    debug('transfer', `失败：${reason}（临时分片已保留）`)
    cb.onStatus('err', `✗ ${reason}。临时分片已保留，可让宿主重发。`)
    // 报错时带上当前缺口，宿主可以据此只补缺的那几片
    const missing = session ? formatRanges(missingRanges(session)) : ''
    emitQr({ type: 'feedback', status: 'error', reason, missing })
  }

  /**
   * 探测帧：建立会话 → 重建已有进度 → 回报状态。
   *
   * 回报的状态直接决定宿主怎么发：`complete` 表示远端已有同一文件（一个字符都不用敲），
   * `partial` 带上缺口（只补缺口），`ready` 表示从头发。
   */
  async function beginProbe(frame: KProbeFrame): Promise<void> {
    debug('transfer', '收到探测帧', {
      name: frame.name,
      total: frame.total,
      chunkSize: frame.chunkSize,
      size: frame.size,
      zstd: frame.zstd,
      fileMd5: frame.fileMd5,
    })
    session = {
      total: frame.total,
      size: frame.size,
      chunkSize: frame.chunkSize,
      zstd: frame.zstd,
      name: frame.name,
      fileMd5: frame.fileMd5,
      received: new Set<number>(),
    }
    cb.onStatus(
      'run',
      `准备接收「${frame.name}」：共 ${frame.total} 片、每片 ${frame.chunkSize} 字节、${frame.size} 字节`,
    )
    try {
      await restoreProgress()
      // 目标文件已经在、且内容一致 → 让宿主别发了
      if (dir) {
        const resolved = await resolveTarget(dir, frame.name, frame.fileMd5)
        if (resolved.identical) {
          debug('transfer', `远端已有同一文件「${resolved.name}」，回报 complete`)
          cb.onProgress(frame.total, frame.total)
          cb.onStatus('ok', `✓ 远端已有同一文件「${resolved.name}」（内容一致，未重写）`)
          emitQr({ type: 'feedback', status: 'complete', saved: resolved.name })
          return
        }
      }
      await reportProgress()
    }
    catch (ex) {
      const message = ex instanceof Error ? ex.message : String(ex)
      await fail(message)
    }
  }

  /** 分片帧。会话参数不一致的片**拒收**，绝不混进当前文件。 */
  async function handleChunk(frame: KChunkFrame): Promise<void> {
    if (!session)
      return
    if (
      frame.total !== session.total
      || frame.size !== session.size
      || frame.chunkSize !== session.chunkSize
      || frame.zstd !== session.zstd
    ) {
      debug('transfer', `拒收第 ${String(frame.seq)} 片：会话参数与探测帧不一致`, {
        frame: { total: frame.total, size: frame.size, chunkSize: frame.chunkSize, zstd: frame.zstd },
        session: { total: session.total, size: session.size, chunkSize: session.chunkSize, zstd: session.zstd },
      })
      cb.onStatus('warn', `✗ 第 ${frame.seq} 片的会话参数与探测帧不一致，已拒收`)
      return
    }
    if (session.received.has(frame.seq)) {
      debug('transfer', `忽略重复分片 seq=${String(frame.seq)}`)
      return
    }
    try {
      // 这一片在**线上**的原始字节：base32 解码即可。
      // `zstd` 时它是「整条 zstd 流的一段」，**不能**在这里解压 ——
      // 整文件压缩的流只有收齐后才能解压一次（解压放在 finalize）。
      const slice = b32Decode(frame.payload)
      const want = expectedChunkLen(frame.seq)
      if (want && want.exact && slice.byteLength !== want.len) {
        debug('transfer', `丢弃第 ${String(frame.seq)} 片：${String(slice.byteLength)} 字节，应为 ${String(want.len)}`)
        cb.onStatus('warn', `✗ 第 ${frame.seq} 片长度不符，已丢弃待重传`)
        return
      }
      if (want && !want.exact && slice.byteLength > want.len) {
        debug('transfer', `丢弃第 ${String(frame.seq)} 片：${String(slice.byteLength)} 字节，超出上限 ${String(want.len)}`)
        cb.onStatus('warn', `✗ 第 ${frame.seq} 片长度超出上限，已丢弃待重传`)
        return
      }

      debug('transfer', `收到第 ${String(frame.seq)} 片：线上 ${String(slice.byteLength)} 字节`)
      await writeChunk(frame.seq, slice)
      session.received.add(frame.seq)
      await reportProgress()
    }
    catch (ex) {
      const message = ex instanceof Error ? ex.message : String(ex)
      await fail(`分片落盘失败：${message}`)
    }
  }

  return {
    get authorized() {
      return authorized
    },

    async pickDirectory() {
      const picker = (globalThis as unknown as {
        showDirectoryPicker?: () => Promise<FsDirectoryHandle>
      }).showDirectoryPicker
      if (typeof picker !== 'function') {
        cb.onStatus('err', '当前浏览器不支持「选择保存目录」（需要 Chrome/Edge 121+）。文件接收不可用；文本接收不受影响。')
        return false
      }
      try {
        const handle = await picker()
        debug('transfer', '已授权保存目录')
        authorized = true
        dir = handle
        // 授权前可能已经收到探测帧；授权后补上目录并重建进度
        if (session) {
          await beginProbe({
            kind: 'probe',
            zstd: session.zstd,
            total: session.total,
            size: session.size,
            chunkSize: session.chunkSize,
            name: session.name,
            fileMd5: session.fileMd5,
          })
        }
        else {
          emitQr(status())
        }
        cb.onStatus('run', '已授权保存目录，等待宿主发送…')
        return true
      }
      catch {
        // 用户取消选择：不是错误
        return false
      }
    },

    accept(ch: string) {
      idleAt = Date.now()

      if (!inFrame) {
        if (ch !== '0')
          return false
        inFrame = true
        frameBuffer = '0'
        return true
      }

      frameBuffer += ch

      // magic 分流：`0` + `kb` + (`b`|`c`) 才是文件帧。
      // 到第 4 个字符就能判定；是文本帧（`0kba`）就放手，让文本通道去收。
      if (frameBuffer.length <= 4) {
        const at = frameBuffer.length - 1
        const expected = `${'0'}${FILE_MAGIC_PREFIX}`[at]
        const isKindSlot = at === 3
        const ok = isKindSlot
          ? FILE_MAGIC_KINDS.includes(ch)
          : ch === expected
        if (!ok) {
          reset()
          return false
        }
      }

      if (ch !== '1') {
        if (frameBuffer.length > MAX_FRAME_CHARS)
          reset()
        return true
      }

      // 完整一帧（以 `1` 收尾）
      const raw = frameBuffer
      frameBuffer = ''
      inFrame = false
      const parsed = parseKFrame(raw)
      // 形状对但内容非法 / 不是文件帧：吞掉，不再转交文本通道
      if (!parsed) {
        debug('transfer', `整帧校验失败（${String(raw.length)} 字符），已丢弃`, raw.slice(0, 64))
        return true
      }
      if (parsed.kind === 'text')
        return true
      debug('transfer', `收到 ${parsed.kind} 帧（${String(raw.length)} 字符）`)

      if (!authorized) {
        debug('transfer', '收到文件帧但尚未授权保存目录，回报 unauthorized')
        cb.onStatus('warn', '收到文件帧，但还没有选择保存目录 —— 请先点「选择保存目录」。')
        emitQr({ type: 'feedback', status: 'unauthorized' })
        return true
      }

      if (parsed.kind === 'probe')
        void beginProbe(parsed)
      else
        void handleChunk(parsed)
      return true
    },

    reset,

    checkTimeout(now = Date.now()) {
      if (idleAt === 0 || now - idleAt < IDLE_TIMEOUT)
        return
      idleAt = 0
      if (inFrame)
        reset()
    },
  }
}

/** 分片在磁盘上的临时文件名。 */
function chunkFileName(seq: number): string {
  return `p${String(seq)}`
}

/** 缺失区间 → `"0-2,7,9-11"`（与协议里的写法一致）。 */
function formatRanges(ranges: Array<[number, number]>): string {
  return ranges
    .map(([a, b]) => (a === b ? String(a) : `${a}-${b}`))
    .join(',')
}

/**
 * 按 `name` 找一个可写的目标文件名，**绝不覆盖已有文件**。
 *
 * 不存在 → 直接用 `name`
 * 存在但内容 MD5 相同 → 视为幂等，不重写（`identical: true`）
 * 存在且内容不同 → 退到 `name (1).ext`、`name (2).ext`…（沿用系统「同名副本」习惯）
 */
async function resolveTarget(
  dir: FsDirectoryHandle,
  name: string,
  fileMd5: string,
): Promise<{ name: string, identical: boolean }> {
  const { base, ext } = splitName(name)
  for (let i = 0; i < 1000; i++) {
    const candidate = i === 0 ? name : `${base} (${String(i)})${ext}`
    let file: File
    try {
      file = await (await dir.getFileHandle(candidate)).getFile()
    }
    catch {
      // 不存在 → 这个名字可用
      return { name: candidate, identical: false }
    }
    const digest = md5B32(new Uint8Array(await file.arrayBuffer()))
    if (digest === fileMd5)
      return { name: candidate, identical: true }
  }
  throw new Error('同名文件过多，无法生成可用的新文件名')
}

/** 拆出 `{ base, ext }`；点开头的是隐藏文件，没有扩展名。 */
function splitName(name: string): { base: string, ext: string } {
  const at = name.lastIndexOf('.')
  if (at <= 0)
    return { base: name, ext: '' }
  return { base: name.slice(0, at), ext: name.slice(at) }
}
