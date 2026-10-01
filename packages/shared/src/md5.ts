/**
 * MD5（十六进制小写），与宿主机侧 `$.md5` / Rust `md-5` 的实现一致。
 *
 * MD5 早已不适合做安全用途；这里保留它是因为 ClipBeam 的两条协议都用它做
 * 「分片指纹 / 整文件指纹」——两端算法一致比强度重要。
 *
 * 为什么不用 WebCrypto：`crypto.subtle` 不提供 MD5（FIPS 里没有它）。
 * 为什么不留到运行时再引依赖：接收页要求**单文件、零网络**，多一个 npm 包的体积
 * 换不来什么 —— 表驱动实现只有几十行。
 */

import { b32EncodeLower } from './b32'

const S = [
  7,
  12,
  17,
  22,
  7,
  12,
  17,
  22,
  7,
  12,
  17,
  22,
  7,
  12,
  17,
  22,
  5,
  9,
  14,
  20,
  5,
  9,
  14,
  20,
  5,
  9,
  14,
  20,
  5,
  9,
  14,
  20,
  4,
  11,
  16,
  23,
  4,
  11,
  16,
  23,
  4,
  11,
  16,
  23,
  4,
  11,
  16,
  23,
  6,
  10,
  15,
  21,
  6,
  10,
  15,
  21,
  6,
  10,
  15,
  21,
  6,
  10,
  15,
  21,
]

// K[i] = floor(abs(sin(i + 1)) * 2^32)，按位与后即无符号 32 位常量
const K = new Uint32Array(64)
for (let i = 0; i < 64; i++)
  K[i] = Math.floor(Math.abs(Math.sin(i + 1)) * 4294967296) >>> 0

/** 计算 MD5，返回 32 位小写十六进制。 */
export default function md5(data: Uint8Array | string): string {
  if (typeof data === 'string')
    data = new TextEncoder().encode(data)
  return [...md5Bytes(data)]
    .map(byte => byte.toString(16).padStart(2, '0'))
    .join('')
}

/**
 * 计算 MD5，返回 16 字节原始摘要。
 *
 * 文件通道的指纹字段是 `base32(md5_bytes)`（16 字节恰好编成 26 个 base32 字符），
 * 与宿主机脚本的 `$.base32_lower_nopad($.md5 的原始字节)` 对齐 —— 所以这里必须能拿到
 * 字节，而不是先转成十六进制再编码（那是另一种串，两边对不上）。
 */
export function md5Bytes(data: Uint8Array): Uint8Array {
  const bitLen = data.length * 8
  // 补位：0x80，然后补 0 到 56 (mod 64)，最后 8 字节小端比特长度
  const padded = new Uint8Array(((data.length + 8) >> 6) * 64 + 64)
  padded.set(data)
  padded[data.length] = 0x80
  const view = new DataView(padded.buffer)
  // 比特长度超过 2^32 的部分写在低 4 字节之后的 4 字节里；JS 里文件不可能那么大，
  // 但仍然按规范写全，避免 32 位截断
  view.setUint32(padded.length - 8, bitLen >>> 0, true)
  view.setUint32(padded.length - 4, Math.floor(bitLen / 4294967296), true)

  let a0 = 0x67452301
  let b0 = 0xEFCDAB89
  let c0 = 0x98BADCFE
  let d0 = 0x10325476

  const M = new Uint32Array(16)
  for (let off = 0; off < padded.length; off += 64) {
    for (let i = 0; i < 16; i++)
      M[i] = view.getUint32(off + i * 4, true)

    let A = a0
    let B = b0
    let C = c0
    let D = d0

    for (let i = 0; i < 64; i++) {
      let F: number
      let g: number
      if (i < 16) {
        F = (B & C) | (~B & D)
        g = i
      }
      else if (i < 32) {
        F = (D & B) | (~D & C)
        g = (5 * i + 1) % 16
      }
      else if (i < 48) {
        F = B ^ C ^ D
        g = (3 * i + 5) % 16
      }
      else {
        F = C ^ (B | ~D)
        g = (7 * i) % 16
      }

      const tmp = D
      D = C
      C = B
      const sum = (A + F + K[i] + M[g]) >>> 0
      B = (B + ((sum << S[i]) | (sum >>> (32 - S[i])))) >>> 0
      A = tmp
    }

    a0 = (a0 + A) >>> 0
    b0 = (b0 + B) >>> 0
    c0 = (c0 + C) >>> 0
    d0 = (d0 + D) >>> 0
  }

  // 输出小端：每个状态字按低字节在前 —— 分片指纹走 base32，字节序必须是这个
  const out = new Uint8Array(16)
  const outView = new DataView(out.buffer)
  outView.setUint32(0, a0, true)
  outView.setUint32(4, b0, true)
  outView.setUint32(8, c0, true)
  outView.setUint32(12, d0, true)
  return out
}

/**
 * `md5` 的 16 字节摘要 → base32 小写无填充（26 字符）。
 *
 * 这是协议指纹字段的**唯一正确形态**：不能对 `md5()` 返回的十六进制字符串再做
 * base32（那是 52 字符，而且 hex 里含 `0`/`1`，会撞坏帧的定界符）。
 * 把它固化成函数，是为了让这条约定不可能被写错 —— 与宿主机脚本侧的
 * `$.md5(data, 'base32_lower')` 是同一件事。
 */
export function md5B32(data: Uint8Array): string {
  return b32EncodeLower(md5Bytes(data))
}
