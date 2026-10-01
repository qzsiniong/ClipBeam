import { describe, expect, it } from 'vitest'
import md5 from '../md5'

const enc = new TextEncoder()

describe('md5', () => {
  it('matches the RFC 1321 test suite', () => {
    const cases: [string, string][] = [
      ['', 'd41d8cd98f00b204e9800998ecf8427e'],
      ['a', '0cc175b9c0f1b6a831c399e269772661'],
      ['abc', '900150983cd24fb0d6963f7d28e17f72'],
      ['message digest', 'f96b697d7cb7938d525a2f31aaf161d0'],
      ['abcdefghijklmnopqrstuvwxyz', 'c3fcd3d76192e4007dfb496cca67e13b'],
      ['ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789', 'd174ab98d277d9f5a5611c2c9f419d9f'],
      ['12345678901234567890123456789012345678901234567890123456789012345678901234567890', '57edf4a22be3c955ac49da2e2107b67a'],
    ]
    for (const [input, expected] of cases)
      expect(md5(enc.encode(input)), JSON.stringify(input)).toBe(expected)
  })

  it('handles the padding boundaries (55 / 56 / 63 / 64 / 65 bytes)', () => {
    // 这些长度各自踩到「补位后是否需要多一个块」的边界
    const sizes = [55, 56, 57, 63, 64, 65, 119, 120, 128]
    const seen = new Set<string>()
    for (const n of sizes) {
      const digest = md5(new Uint8Array(n).fill(0x61))
      expect(digest, `n=${n}`).toMatch(/^[0-9a-f]{32}$/)
      seen.add(digest)
    }
    // 不同长度必须产出不同摘要（防止把长度算错却仍返回一个稳定值）
    expect(seen.size).toBe(sizes.length)
  })
})

describe('md5Bytes', () => {
  it('是 md5 的字节形态，且字节序与「小端十六进制」一致', async () => {
    const { md5Bytes } = await import('../md5')
    const bytes = md5Bytes(new TextEncoder().encode('abc'))
    expect(bytes).toHaveLength(16)
    // 900150983cd24fb0d6963f7d28e17f72 的字节序列
    expect([...bytes].map(b => b.toString(16).padStart(2, '0')).join('')).toBe(
      '900150983cd24fb0d6963f7d28e17f72',
    )
  })
})
