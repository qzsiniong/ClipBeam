const B32 = 'abcdefghijklmnopqrstuvwxyz234567'

/** RFC4648 base32 无填充解码（输入小写/大写均可，内部 trim）。 */
export function b32Decode(s: string): Uint8Array {
  const src = s.trim().toLowerCase()
  let val = 0
  let bits = 0
  const out: number[] = []
  for (let i = 0; i < src.length; i++) {
    const v = B32.indexOf(src[i])
    if (v < 0)
      throw new Error('非法 base32 字符')
    val = (val << 5) | v
    bits += 5
    if (bits >= 8) {
      bits -= 8
      out.push((val >>> bits) & 0xFF)
    }
  }
  return new Uint8Array(out)
}

/** RFC4648 base32 无填充编码，输出大写。 */
export function b32EncodeUpper(bytes: Uint8Array | readonly number[]): string {
  let val = 0
  let bits = 0
  let out = ''
  for (let i = 0; i < bytes.length; i++) {
    val = (val << 8) | bytes[i]
    bits += 8
    while (bits >= 5) {
      bits -= 5
      out += B32[(val >>> bits) & 31].toUpperCase()
    }
  }
  if (bits > 0)
    out += B32[((val << (5 - bits)) & 31)].toUpperCase()
  return out
}

/** 4 字节 CRC32 的 base32 摘要（7 字符大写）。 */
export function crcB32(crc: number): string {
  const b = [(crc >>> 24) & 255, (crc >>> 16) & 255, (crc >>> 8) & 255, crc & 255]
  return b32EncodeUpper(b)
}
