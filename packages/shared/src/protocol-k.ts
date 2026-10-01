/**
 * 协议 K（键盘通道）：ClipBeam protocol v2 的按键帧编解码。
 *
 * 这是「字符集纪律」的收敛版（旧文本通道与文件通道各有一套，v2 合成一套）：
 * **除定界符本身，线上不得出现 `0` 与 `1`**，
 * 于是 `0` 可以无歧义地作字段分隔、`1` 可以无歧义地作整帧终结 —— 全程只用数字行与
 * 字母，任何键盘布局都能敲出，不需要 Shift。
 *
 * 线上形态：
 *
 * ```text
 * 0 <magic> 0 <flags> 0 <crc> 0 <tail…> 1
 * ```
 *
 * - `magic`（3 字符）**就是消息类型**：`kba` 剪贴板文本 / `kbb` 文件探测 / `kbc` 文件分片。
 *   不再有单独的 type 与 version 字段 —— 少两个字段就少两处可以敲错、可以对不上的地方；
 *   版本演进靠 magic 本身（新类型换新 magic），旧端遇到不认识的 magic 直接丢弃即可。
 * - `flags`（1 个 base32 数字 = 5 bit）：bit0 表示「整段负载 / 整文件经过 zstd 压缩」，
 *   其余位保留。
 * - `crc`（7 字符）：{@link kCrc}，覆盖 `magic 0 flags 0 tail` —— 也就是**除引导符与
 *   `crc` 字段本身之外的全部内容**。注意它不是帧里的连续子串（`0<crc>0` 夹在中间），
 *   而是一个明确定义的拼接。把 `flags` 纳进来是必须的：它决定载荷要不要解压，
 *   若落在校验之外，一个被敲坏的字符会**静默**走进错误的解码分支。
 * - `tail`：按 magic 不同，1 个或 5 个字段，全部经过 base32 小写无填充编码。
 */

import { b32Decode, b32EncodeLower } from './b32'
import crc32 from './crc32'

/** `kba`：剪贴板文本。 */
export const K_MAGIC_TEXT = 'kba'
/** `kbb`：文件探测（文件名 / 大小 / 分片方案 / 整文件指纹）。 */
export const K_MAGIC_PROBE = 'kbb'
/** `kbc`：文件分片。 */
export const K_MAGIC_CHUNK = 'kbc'

/** bit0：整段负载（`kba`/`kbc`）或整文件（`kbb`）经 zstd 压缩。 */
export const K_FLAG_ZSTD = 1

/**
 * 已知的 flag 位。解析时超出这一集合的位一律拒绝。
 *
 * 保留位不是「忽略就行」：`flags` 进了 CRC 覆盖范围，敲错一位会静默改变语义 ——
 * 与其猜，不如把未知位当作「本端不认识的更新协议」直接丢弃（见 {@link parseKFrame}）。
 * 这也是为什么新增 flag 必须换 magic：否则新旧两端对同一串字符的理解会分叉。
 */
const K_FLAGS_KNOWN = K_FLAG_ZSTD

/** 字段分隔符（数字行，免 Shift）。 */
const SEP = '0'
/** 整帧终结符（数字行，免 Shift）。 */
const END = '1'

/**
 * base32 小写字母表。
 *
 * 本模块只需要「单个 base32 数字」这一个操作（`flags` 字段），而 `b32.ts` 只提供
 * 整字节的编解码（编 1 字节会给出 2 个字符），所以这里显式写一份 32 字符表。
 * 它是 RFC4648 的固定表，与 `b32.ts` 里的表逐字符一致。
 */
const B32 = 'abcdefghijklmnopqrstuvwxyz234567'

/** 合法的已编码字段：base32 小写字母表，空串由调用方按位置单独放行。 */
const FIELD_RE = /^[a-z2-7]+$/
/** 允许为空的位置：`kba`/`kbc` 的**末字段**（0 字节文件会切出一片空载荷）。 */
const FIELD_RE_MAYBE_EMPTY = /^[a-z2-7]*$/

const enc = new TextEncoder()
const dec = new TextDecoder()

/**
 * `numB32(n)`：数值 → **十进制字符串** → 该字符串 UTF-8 字节的 base32 小写无填充。
 *
 * **为什么先转十进制字符串**：需要的是「数字本身」而不是它的二进制形态，接收端解出来
 * 必须还是人能核对的那个十进制数（`4096` 就是 `"4096"`，不是 `0x1000` 的 2 字节）。
 * 而且这条路径天然满足字符集纪律：十进制数字里的 `0`/`1` 只出现在**输入**侧，
 * 编码后必然落在字母表 `[a-z2-7]` 内，于是不需要定宽、不需要长度前缀。
 *
 * 例：`0`→`ga`、`1`→`ge`、`4096`→`gqydsnq`。
 */
export function numB32(n: number): string {
  return b32EncodeLower(enc.encode(String(n)))
}

/**
 * `numB32` 的逆：base32 小写、**解码回来必须是纯十进制数字串**，否则 `null`。
 *
 * 只认 `^\d+$`（非负整数）：`numB32` 的入参在协议里全是序号 / 大小 / 分片长，
 * 没有负数；放宽成 `-?\d+` 只会让 `fuyq` 这类历史形态混进来。
 * 也**不**用 `Number('')` 那套宽容语义（空串会变 0），避免静默接受垃圾。
 */
export function parseNumB32(s: string): number | null {
  if (!FIELD_RE.test(s))
    return null
  let text: string
  try {
    text = dec.decode(b32Decode(s))
  }
  catch {
    return null
  }
  if (!/^\d+$/.test(text))
    return null
  const value = Number(text)
  return Number.isSafeInteger(value) ? value : null
}

/** `kba` 解析结果：剪贴板文本。 */
export interface KTextFrame {
  kind: 'text'
  zstd: boolean
  /** base32 小写无填充的负载（保持文本形态，调用方自行解码）。 */
  payload: string
}

/** `kbb` 解析结果：文件探测。 */
export interface KProbeFrame {
  kind: 'probe'
  zstd: boolean
  total: number
  /** 文件原始字节数。 */
  size: number
  /** 发送端配置的每片字节数。 */
  chunkSize: number
  /** 文件名（已解码为文本）。 */
  name: string
  /** 整文件原始字节 MD5 的 base32 小写无填充（26 字符）。 */
  fileMd5: string
}

/** `kbc` 解析结果：文件分片。 */
export interface KChunkFrame {
  kind: 'chunk'
  zstd: boolean
  seq: number
  total: number
  /** 文件原始字节数（每片都带，便于任意一片自证属于哪个文件）。 */
  size: number
  /** 发送端的分片长度，与探测帧重复（见 {@link buildChunkFrame}）。 */
  chunkSize: number
  /** base32 小写无填充的该片负载。 */
  payload: string
}

/** 一帧键盘消息。 */
export type KFrame = KTextFrame | KProbeFrame | KChunkFrame

/**
 * `flags` 值 → 单个 base32 数字（`a`..`7` 恰好覆盖 0..31）。
 *
 * 不用 `b32EncodeLower([v])`：那会把 8 bit 编成 **2** 个字符，而这里要的是 5 bit 的
 * 单个数字。`& 31` 之后下标查表就是「取低 5 位」。
 */
function flagsDigit(v: number): string {
  return B32[v & 31]
}

/**
 * `flags` 数字 → 数值；不是单个 base32 数字则 `null`。
 */
function parseFlagsDigit(s: string): number | null {
  if (s.length !== 1)
    return null
  const v = B32.indexOf(s)
  return v < 0 ? null : v
}

/**
 * 7 字符 CRC 摘要：`base32lower(crc32(被校验的字符串字节))`。
 *
 * **为什么对被校验的那串字符算 CRC，而不是对解码后的字节算**：
 * 1. 这一层要防的是「敲错 / 串进脏字符」造成的**字段级**损坏，编码串本身就是线上唯一
 *    存在的东西 —— 对它算，任何一处字符变化都会被发现，包括 `seq`/`total`/`size` 这些
 *    头部字段（对解码后字节算就只能覆盖负载，头部错了要到很后面才炸）。
 * 2. 发送端算 CRC 时无需先解码再编码，省一趟；接收端可以把「解码负载」推迟到 CRC 通过
 *    之后再做，垃圾帧不会走到解码器。
 */
function kCrc(text: string): string {
  const sum = crc32(enc.encode(text))
  return b32EncodeLower([(sum >>> 24) & 0xFF, (sum >>> 16) & 0xFF, (sum >>> 8) & 0xFF, sum & 0xFF])
}

/**
 * CRC 的被覆盖内容：`magic 0 flags 0 tail`。
 *
 * 解析侧必须能逐字节拼出同一串 —— 正因为 `tail` 里只有末字段可能为空（见
 * {@link parseKFrame} 的空字段规则），`fields.join(SEP)` 才能无损还原它。
 */
function crcCovered(magic: string, flagsText: string, tailText: string): string {
  return `${magic}${SEP}${flagsText}${SEP}${tailText}`
}

/** 把 magic/flags/crc/tail 拼成完整帧（tail 是已编码字段数组）。 */
function assemble(magic: string, flags: number, tail: string[]): string {
  const flagsText = flagsDigit(flags)
  const tailText = tail.join(SEP)
  const crc = kCrc(crcCovered(magic, flagsText, tailText))
  return `${SEP}${magic}${SEP}${flagsText}${SEP}${crc}${SEP}${tailText}${END}`
}

/**
 * 构造 `kba`：`payload` 是**已经编码好**的 base32 小写字符串。
 *
 * 刻意收已编码串而不是原文：调用方（发送侧）需要先把文本转成字节再决定是否 zstd，
 * 编码这一步本来就在它手上；这里不做二次编码，避免「编码了两次」这种最难查的错。
 */
export function buildTextFrame(payload: string, zstd: boolean): string {
  return assemble(K_MAGIC_TEXT, zstd ? K_FLAG_ZSTD : 0, [payload])
}

/**
 * 构造 `kbb` 文件探测帧。
 *
 * `name` 收**明文**（这里内部 base32 编码）：它是唯一一个来自用户、可能带任意 Unicode
 * 的字段，让构造方去关心「文件名要先编码」太容易漏；收明文则不可能漏。
 * `fileMd5` 收 26 字符的 base32 小写串（`md5B32` 的产物），保持与宿主脚本同形态。
 */
export function buildProbeFrame(f: { total: number, size: number, chunkSize: number, name: string, zstd: boolean, fileMd5: string }): string {
  return assemble(K_MAGIC_PROBE, f.zstd ? K_FLAG_ZSTD : 0, [
    numB32(f.total),
    numB32(f.size),
    numB32(f.chunkSize),
    b32EncodeLower(enc.encode(f.name)),
    f.fileMd5,
  ])
}

/**
 * 构造 `kbc` 文件分片帧（`payload` 是已编码的 base32 小写串）。
 *
 * **`chunkSize` 与探测帧重复是故意的**：接收端可能先后收到两个不同会话（用户改了每片
 * 大小、或换了个文件）的片子，只靠 `seq/total` 无法区分 —— 一片属于哪个会话，要由
 * 「文件大小 + 分片长度」共同定义。把它写进每一片，接收端就能在**混入之前**把不属于
 * 当前会话的片直接拒掉，而不是等拼完才发现 MD5 对不上。多敲几个字符换一次可判定的拒绝，
 * 划算。
 */
export function buildChunkFrame(f: { seq: number, total: number, size: number, chunkSize: number, payload: string, zstd: boolean }): string {
  return assemble(K_MAGIC_CHUNK, f.zstd ? K_FLAG_ZSTD : 0, [
    numB32(f.seq),
    numB32(f.total),
    numB32(f.size),
    numB32(f.chunkSize),
    f.payload,
  ])
}

/**
 * 解析一帧键盘消息。任何可疑之处都返回 `null`（调用方一律忽略：画面里可能有别的二维码、
 * 键盘也可能只敲到半截）。
 *
 * 拒绝的条件（顺序即下面的执行顺序）：
 * 1. 不以 `0` 开头 / 不以 `1` 结尾（含空串、只有定界符）；
 * 2. 未知 magic、该 magic 的字段数不符、字段含 `[a-z2-7]` 之外的字符或位置不对的空字段、
 *    `flags` 不是单个 base32 数字或带保留位；
 * 3. `crc` 与 `magic 0 flags 0 tail` 的线上字符对不上 —— **先验 CRC 再看数值语义**，因为 CRC 是唯一能兜住
 *    「一个字符敲错」的判据；语义检查（数值范围）反而可能被坏字符蒙对；
 * 4. 数值解不出，或越界（`total < 1`、`seq >= total`、`size < 0`、`chunkSize < 1`）、
 *    `fileMd5` 长度不是 26。
 */
export function parseKFrame(raw: string): KFrame | null {
  // 键盘输入可能带前后空白；先 trim 再判形态（`0`/`1` 是 ASCII，trim 不会削掉它们）
  const text = raw.trim()
  if (text.length < 2 || !text.startsWith(SEP) || !text.endsWith(END))
    return null

  const body = text.slice(1, -1)
  // 定界符先行：`0`/`1` 之外的字符一律不参与判定。`split(SEP)` 不会在开头产生空串
  // （body 的首字符必然是 magic 的首字符），只会把**末尾的空字段**表示成「末元素为空串」。
  const fields = body.split(SEP)
  if (fields.length < 4)
    return null

  const [magic, flagsText, crc, ...tail] = fields
  if (magic !== K_MAGIC_TEXT && magic !== K_MAGIC_PROBE && magic !== K_MAGIC_CHUNK)
    return null

  // 字段数必须**精确**匹配：`kba` 只有 1 个 tail 字段，`kbb`/`kbc` 各有 5 个。
  // 多一个少一个都说明这不是一帧完整的话（或负载里混进了 `0` 把字段切碎了）
  const want = magic === K_MAGIC_TEXT ? 1 : 5
  if (tail.length !== want)
    return null

  // flags 只占 5 bit：不是单个 base32 数字、或带了本端不认识的保留位，都拒绝
  const flags = parseFlagsDigit(flagsText)
  if (flags === null || (flags & ~K_FLAGS_KNOWN) !== 0)
    return null

  // **只有 `kba`/`kbc` 的末字段（负载）允许为空**：0 字节文件会切出一片空载荷，
  // 而它是最后一个字段、后面紧跟终结符，所以无歧义。其余位置的空字段说明帧被切碎了
  // （或有人手工拼帧），一律拒绝。`kbb` 的末字段是 `fileMd5`，同样不许为空。
  const allowEmptyLast = magic === K_MAGIC_TEXT || magic === K_MAGIC_CHUNK
  for (let i = 0; i < tail.length; i++) {
    const re = allowEmptyLast && i === tail.length - 1 ? FIELD_RE_MAYBE_EMPTY : FIELD_RE
    if (!re.test(tail[i]))
      return null
  }

  // CRC 覆盖 `magic 0 flags 0 tail` 在线上原样的字符，必须逐字节还原：上面已经保证
  // 「只有末字段可能为空」，于是 `join(SEP)` 与原始子串等价（末字段为空时 join 会补回
  // 那个末尾分隔符）。放在字符集检查之后、数值解析之前：先保证比的是「同一形态」的串。
  if (kCrc(crcCovered(magic, flagsText, tail.join(SEP))) !== crc)
    return null

  const zstd = (flags & K_FLAG_ZSTD) !== 0

  if (magic === K_MAGIC_TEXT)
    return { kind: 'text', zstd, payload: tail[0] }

  if (magic === K_MAGIC_PROBE) {
    const [totalText, sizeText, chunkSizeText, nameText, fileMd5] = tail
    const total = parseNumB32(totalText)
    const size = parseNumB32(sizeText)
    const chunkSize = parseNumB32(chunkSizeText)
    if (total === null || size === null || chunkSize === null)
      return null
    // 指纹定长 26：base32(md5 的 16 字节) 恰好 26 字符；长度不对说明这帧不完整或串了别的东西
    if (fileMd5.length !== 26)
      return null
    if (total < 1 || size < 0 || chunkSize < 1)
      return null

    let name: string
    try {
      name = dec.decode(b32Decode(nameText))
    }
    catch {
      return null
    }
    return { kind: 'probe', zstd, total, size, chunkSize, name, fileMd5 }
  }

  const [seqText, totalText, sizeText, chunkSizeText, payload] = tail
  const seq = parseNumB32(seqText)
  const total = parseNumB32(totalText)
  const size = parseNumB32(sizeText)
  const chunkSize = parseNumB32(chunkSizeText)
  if (seq === null || total === null || size === null || chunkSize === null)
    return null
  if (total < 1 || seq >= total || size < 0 || chunkSize < 1)
    return null

  return { kind: 'chunk', zstd, seq, total, size, chunkSize, payload }
}
