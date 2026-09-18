import { b32EncodeUpper, crcB32 } from './b32'
import crc32 from './crc32'

const enc = new TextEncoder()

/** 数字参数解析并夹取到 [lo, hi]；非法值回退为 lo。 */
export function clamp(v: number | string, lo: number, hi: number): number {
  const n = parseInt(String(v), 10)
  return Number.isNaN(n) ? lo : Math.max(lo, Math.min(hi, n))
}

/**
 * 出站分帧：CB1.<total>.<index>.<digest>.<payload>
 * digest 对大写 payload 字符串再算 CRC32（与宿主机 receive.rs 对齐）。
 */
export function buildFrames(text: string, chunkSize: number): { frames: string[], total: number } {
  const payload = b32EncodeUpper(enc.encode(text))
  const digest = crcB32(crc32(enc.encode(payload)))
  const chunks: string[] = []
  for (let i = 0; i < payload.length; i += chunkSize)
    chunks.push(payload.slice(i, i + chunkSize))
  const total = chunks.length
  const frames = chunks.map((ch, i) => `CB1.${total}.${i}.${digest}.${ch}`)
  return { frames, total }
}
