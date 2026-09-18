const T = new Uint32Array(256)
for (let n = 0; n < 256; n++) {
  let c = n
  for (let k = 0; k < 8; k++)
    c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1
  T[n] = c >>> 0
}

/** IEEE CRC32（多项式 0xEDB88320），与宿主机 Rust crc32 实现一致。 */
export default function crc32(d: Uint8Array): number {
  let crc = 0xFFFFFFFF
  for (let i = 0; i < d.length; i++)
    crc = T[(crc ^ d[i]) & 0xFF] ^ (crc >>> 8)
  return (crc ^ 0xFFFFFFFF) >>> 0
}
