import { b32EncodeUpper } from './b32'
import { buildQFrame, Q_MAGIC_CLIP, qPayloadCrc } from './protocol-q'

const enc = new TextEncoder()

/** 数字参数解析并夹取到 [lo, hi]；非法值回退为 lo。 */
export function clamp(v: number | string, lo: number, hi: number): number {
  const n = parseInt(String(v), 10)
  return Number.isNaN(n) ? lo : Math.max(lo, Math.min(hi, n))
}

/**
 * 出站分帧（Q 通道的剪贴板消息）：把本机剪贴板文本切成若干 `QBA` 帧轮播。
 *
 * ```text
 * QBA.<total>.<index>.<crc>.<payload 片段>
 * ```
 *
 * `crc` 是**整条 payload** 的摘要（base32 大写 7 字符），**同一批每一帧都一样**：
 * 它既是宿主机那边的批量身份（换消息就换批），也是重组完成后的整体校验。
 * 这与键盘通道「每帧各算各的」是刻意不同的 —— 二维码是一组轮播，不是独立投递。
 */
export function buildFrames(text: string, chunkSize: number): { frames: string[], total: number } {
  const payload = b32EncodeUpper(enc.encode(text))
  const crc = qPayloadCrc(payload)
  const chunks: string[] = []
  for (let i = 0; i < payload.length; i += chunkSize)
    chunks.push(payload.slice(i, i + chunkSize))
  // 空文本也要有一帧，否则页面上什么都不显示
  if (chunks.length === 0)
    chunks.push('')
  const total = chunks.length
  const frames = chunks.map((ch, i) => buildQFrame(Q_MAGIC_CLIP, total, i, crc, ch))
  return { frames, total }
}
