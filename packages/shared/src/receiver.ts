/**
 * 文本接收端：键盘通道上的 `kba` 帧 → 校验 → 解压 → 交给视图写剪贴板。
 *
 * # v2 的帧
 *
 * ```text
 * 0 kba 0 <flags> 0 <crc> 0 <payload> 1
 * ```
 *
 * `flags` 是单个 base32 字母表数字，bit0 = 载荷经 zstd；
 * `crc` 是 7 字符 base32 小写，算在 **payload 的线上子串** 上；
 * `payload` 是 base32 小写（可为空）；`0`/`1` 不可能出现在任何字段里，
 *   所以「一直缓冲到 `1`」是安全的，且 `1` 一定是终结符。
 *
 * # 与文件通道共用一条 keydown 流
 *
 * v2 里文本帧（`0kba`）与文件帧（`0kbb`/`0kbc`）前三个字符相同，第 4 个字符才分岔。
 * 页面把每个字符同时喂给两条通道，本模块只认 `0kba`，一旦发现第 4 个字符不是 `a`
 * 就复位并把字符留给文件通道 —— 不需要一个中央路由器。
 *
 * **CRC 与字段合法性全部交给 `parseKFrame`**：这里不再自己算一遍。协议只有一处
 * 实现，就不会出现「页面认了、脚本不认」这类两套解析器的老问题。
 */

import type { KTextFrame } from './protocol-k'
import * as fzstd from 'fzstd'
import { b32Decode } from './b32'
import { debug } from './debug'
import { parseKFrame } from './protocol-k'

/** 帧前缀（含引导 `0`）。 */
const MAGIC = '0kba'
/** 载荷起始位置的正则：`0kba0<1 字符 flags>0<7 字符 crc>0<payload…>`。 */
const PAYLOAD_RE = /^0kba0[a-z2-7]0[a-z2-7]{7}0([a-z2-7]*)$/
const IDLE_TIMEOUT = 90_000
const TYPE_TAIL = 96
/** 单帧字符上限（文本帧受宿主上限约束，32K 已经很宽松）。 */
const MAX_FRAME_CHARS = 131_072

export type ReceiverStatusClass = '' | 'run' | 'ok' | 'err' | 'warn'

export interface ReceiverCallbacks {
  /** 暂停开关：暂停期间任何按键都会复位状态机。 */
  isPaused: () => boolean
  /** 状态条样式与文案。 */
  onStatus: (cls: ReceiverStatusClass, msg: string) => void
  /** 接收中进度条显隐。 */
  onBar: (on: boolean) => void
  /** 打字回显：null=隐藏并清空；text=尾部窗口内容；suffix=网格后方追加文本。 */
  onTyped: (text: string | null, suffix?: string) => void
  /** CRC 校验、解码全部通过后的最终文本（由视图层写剪贴板）。 */
  onText: (text: string) => void
}

export interface KbReceiver {
  /** 绑定到 window 的 capture 阶段 keydown 监听。 */
  handle: (e: KeyboardEvent) => void
  /** 复位到 idle（隐藏进度条与回显）。 */
  reset: () => void
  /** 空闲检测；建议每 500ms 调用一次。 */
  checkTimeout: (now?: number) => void
}

export function createReceiver(cb: ReceiverCallbacks): KbReceiver {
  const dec = new TextDecoder()

  let frameBuffer = ''
  let inFrame = false
  let lastEvt = 0

  function reset() {
    inFrame = false
    frameBuffer = ''
    cb.onBar(false)
    cb.onTyped(null)
  }

  /** 帧通过校验后的收尾：解码 → （必要时）解压 → 交文本。 */
  function finalize(frame: KTextFrame) {
    debug('text', `文本帧校验通过：载荷 ${String(frame.payload.length)} 字符，zstd=${String(frame.zstd)}`)
    let bytes: Uint8Array
    try {
      bytes = b32Decode(frame.payload)
    }
    catch (ex) {
      cb.onStatus('err', `base32 解码失败: ${(ex as Error).message}`)
      return
    }
    if (frame.zstd) {
      try {
        bytes = fzstd.decompress(bytes)
      }
      catch (ex) {
        cb.onStatus('err', `zstd 解压失败: ${(ex as Error).message}`)
        return
      }
    }
    try {
      const text = dec.decode(bytes)
      debug('text', `解码完成：${String(bytes.byteLength)} 字节 / ${String(text.length)} 字符`)
      cb.onText(text)
    }
    catch (ex) {
      cb.onStatus('err', `UTF-8 解码失败: ${(ex as Error).message}`)
    }
  }

  function handle(e: KeyboardEvent) {
    if (e.repeat)
      return
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable))
      return
    if (cb.isPaused()) {
      reset()
      return
    }

    const k = e.key
    lastEvt = Date.now()

    if (!inFrame) {
      // 帧必须从 `0` 开始。未成帧前不吞正常输入。
      if (k !== '0')
        return
      inFrame = true
      frameBuffer = '0'
      return
    }

    // 魔术头逐字符校验：`0kba`。第 4 个字符不是 `a` 说明这是文件帧，放手让文件通道收。
    if (frameBuffer.length < MAGIC.length) {
      if (k !== MAGIC.charAt(frameBuffer.length)) {
        reset()
        return
      }
      frameBuffer += k
      if (frameBuffer === MAGIC) {
        debug('text', '识别到文本帧前缀 0kba，开始接收')
        cb.onStatus('run', '正在接收…')
        cb.onBar(true)
        cb.onTyped('')
      }
      // 魔术头还没认全，先不 preventDefault
      return
    }

    // 已是本协议的帧，之后的按键全部吞掉（否则会漏进页面的其它控件）
    e.preventDefault()
    frameBuffer += k

    if (k !== '1') {
      if (frameBuffer.length > MAX_FRAME_CHARS) {
        debug('text', `帧超过 ${String(MAX_FRAME_CHARS)} 字符仍未收尾，复位`)
        reset()
        cb.onStatus('err', '✗ 帧过长，已复位')
        return
      }
      // 回显载荷（尚未收全，取已收到的部分）
      const payload = PAYLOAD_RE.exec(frameBuffer)?.[1]
      if (payload !== undefined) {
        cb.onTyped(payload.slice(-TYPE_TAIL))
        if (payload.length > 0 && payload.length % 256 === 0)
          cb.onStatus('run', `正在接收… ${payload.length} 字符`)
      }
      return
    }

    // 完整一帧
    const raw = frameBuffer
    reset()
    const parsed = parseKFrame(raw)
    debug('text', `收到整帧 ${String(raw.length)} 字符，解析${parsed === null ? '失败' : '成功'}`)
    if (parsed === null) {
      debug('text', '帧内容或 CRC 非法，原文前 96 字符：', raw.slice(0, 96))
      // 形状对但内容/CRC 非法。最可能的原因是丢键、重复键或键盘布局不是 US/QWERTY。
      cb.onStatus('err', '✗ 帧校验失败（CRC 或字段非法）。常见原因：键盘布局非 US/QWERTY、丢键/重复键。请重新发送。')
      return
    }
    if (parsed.kind !== 'text') {
      cb.onStatus('err', '✗ 收到的不是文本帧')
      return
    }
    finalize(parsed)
  }

  function checkTimeout(now: number = Date.now()) {
    if (inFrame && now - lastEvt > IDLE_TIMEOUT) {
      reset()
      cb.onStatus('', '等待接收…（空闲超时，已复位）')
    }
  }

  return { handle, reset, checkTimeout }
}
