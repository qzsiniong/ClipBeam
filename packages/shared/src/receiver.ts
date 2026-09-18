import * as fzstd from 'fzstd'
import { b32Decode, crcB32 } from './b32'
import crc32 from './crc32'

/* 入站键盘帧：0 clipbeam{1|2} 0 <小写 base32 payload> 0 <7 字符 digest> 1
   magic 中 1=未压缩、2=fzstd 压缩。 */
const MAGIC_PREFIX = 'clipbeam'
const IDLE_TIMEOUT = 90_000
const TYPE_TAIL = 96
const DATA_RE = /^[a-z2-7]$/

export type ReceiverStatusClass = '' | 'run' | 'ok' | 'err' | 'warn'

export interface ReceiverCallbacks {
  /** 暂停开关：暂停期间任何按键都会复位状态机。 */
  isPaused: () => boolean
  /** 状态条样式与文案。 */
  onStatus: (cls: ReceiverStatusClass, msg: string) => void
  /** 接收中进度条显隐。 */
  onBar: (on: boolean) => void
  /** 打字回显：null=隐藏并清空；text=尾部窗口内容；suffix=网格后方追加文本（digest 阶段）。 */
  onTyped: (text: string | null, suffix?: string) => void
  /** CRC 校验、解码全部通过后的最终文本（由视图层写剪贴板）。 */
  onText: (text: string) => void
}

export interface KbReceiver {
  /** 绑定到 window 的 capture 阶段 keydown 监听。 */
  handle: (e: KeyboardEvent) => void
  /** 复位到 idle（隐藏进度条与回显）。 */
  reset: () => void
  /** 10 秒空闲检测；建议每 500ms 调用一次。 */
  checkTimeout: (now?: number) => void
}

export function createReceiver(cb: ReceiverCallbacks): KbReceiver {
  const enc = new TextEncoder()
  const dec = new TextDecoder()

  type KbState = 'idle' | 'magic' | 'payload' | 'digest'
  let kbState: KbState = 'idle'
  let magicBuf = ''
  let payloadBuf = ''
  let digestBuf = ''
  let lastEvt = 0
  let frameVersion = 1

  function reset() {
    kbState = 'idle'
    magicBuf = payloadBuf = digestBuf = ''
    cb.onBar(false)
    cb.onTyped(null)
  }

  function finalizeFrame(payload: string, digest: string, version: number) {
    let expect: string
    try {
      expect = crcB32(crc32(enc.encode(payload))).toLowerCase()
    }
    catch (ex) {
      cb.onStatus('err', `校验计算异常: ${(ex as Error).message}`)
      return
    }
    if (expect !== digest.toLowerCase()) {
      cb.onStatus('err', `✗ CRC 校验失败（收到 ${digest}，应为 ${expect}）。常见原因：键盘布局非 US/QWERTY、丢键/重复键。请重新发送。`)
      return
    }
    let bytes: Uint8Array
    try {
      bytes = b32Decode(payload)
    }
    catch (ex) {
      cb.onStatus('err', `base32 解码失败: ${(ex as Error).message}`)
      return
    }
    if (version === 2) {
      try {
        bytes = fzstd.decompress(bytes)
      }
      catch (ex) {
        cb.onStatus('err', `zstd 解压失败: ${(ex as Error).message}`)
        return
      }
    }
    try {
      cb.onText(dec.decode(bytes))
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
    const isData = DATA_RE.test(k)
    lastEvt = Date.now()

    if (kbState === 'idle') {
      if (k === '0') {
        kbState = 'magic'
        magicBuf = ''
        cb.onTyped('')
      }
      return // 未成帧前不吞正常输入
    }
    e.preventDefault() // 魔术头之后全部吞掉

    if (kbState === 'magic') {
      if (magicBuf.length < MAGIC_PREFIX.length) {
        if (k === MAGIC_PREFIX.charAt(magicBuf.length)) {
          magicBuf += k
          cb.onTyped(magicBuf)
        }
        else {
          reset()
        }
      }
      else if (magicBuf.length === MAGIC_PREFIX.length) {
        if (k === '1' || k === '2') {
          frameVersion = Number(k)
          magicBuf += k
          cb.onTyped(magicBuf)
        }
        else {
          reset()
        }
      }
      else if (k === '0') {
        kbState = 'payload'
        payloadBuf = ''
        cb.onTyped('')
        cb.onStatus('run', `正在接收…(v${frameVersion})`)
        cb.onBar(true)
      }
      else {
        reset()
      }
    }
    else if (kbState === 'payload') {
      if (isData) {
        payloadBuf += k
        cb.onTyped(payloadBuf.slice(-TYPE_TAIL))
        if (payloadBuf.length % 128 === 0)
          cb.onStatus('run', `正在接收… ${payloadBuf.length} 字符`)
      }
      else if (k === '0') {
        kbState = 'digest'
        digestBuf = ''
      }
      else {
        reset()
        cb.onStatus('', '等待接收…（含非法字符，已复位）')
      }
    }
    else if (kbState === 'digest') {
      if (isData) {
        digestBuf += k
        cb.onTyped(payloadBuf.slice(-TYPE_TAIL), ` · ${digestBuf}`)
      }
      else if (k === '1') {
        const p = payloadBuf
        const d = digestBuf
        const v = frameVersion
        reset()
        finalizeFrame(p, d, v)
      }
      else {
        reset()
      }
    }
  }

  function checkTimeout(now: number = Date.now()) {
    if (kbState !== 'idle' && now - lastEvt > IDLE_TIMEOUT) {
      reset()
      cb.onStatus('', '等待接收…（10 秒无输入，已复位）')
    }
  }

  return { handle, reset, checkTimeout }
}
