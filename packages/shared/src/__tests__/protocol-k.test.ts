// 协议 K（键盘通道）帧的单元测试（纯 TS，不需要 DOM）。
//
// 盯住四件事：
//   1. **golden vector**：三种 magic 各自钉死一整条帧文本（含 crc 那 7 个字符）——
//      任何一次「顺手重构」只要改动了线上字节，这里必然红；
//   2. **字符集不变量**：除定界符 `0`/`1`，线上只允许 `[a-z2-7]`；
//   3. **CRC 覆盖的是 tail 的线上字符**：改一个字符但不重算 crc 必须被拒；
//   4. **拒绝路径**：未知 magic、字段数不符、空字段、坏数值、坏字符集一律 null。
//
// 测试里的 base32 用**独立实现**（下面几十行），不复用 `../b32` —— 否则实现改错时
// 测试会跟着一起错，两边一起漂移还能通过。

import { describe, expect, it } from 'vitest'
import {
  buildChunkFrame,
  buildProbeFrame,
  buildTextFrame,
  K_FLAG_ZSTD,
  K_MAGIC_CHUNK,
  K_MAGIC_PROBE,
  K_MAGIC_TEXT,
  numB32,
  parseKFrame,
  parseNumB32,
} from '../protocol-k'

/* ── 独立参照实现（RFC4648 base32 无填充，小写） ──────────────────────────── */

const TABLE = 'abcdefghijklmnopqrstuvwxyz234567'

function refEncodeLower(text: string): string {
  return refEncodeBytes(new TextEncoder().encode(text))
}

function refEncodeBytes(bytes: Uint8Array): string {
  let val = 0
  let bits = 0
  let out = ''
  for (const byte of bytes) {
    val = (val << 8) | byte
    bits += 8
    while (bits >= 5) {
      bits -= 5
      out += TABLE[(val >>> bits) & 31]
    }
  }
  if (bits > 0)
    out += TABLE[(val << (5 - bits)) & 31]
  return out
}

function refDecodeBytes(s: string): Uint8Array {
  let val = 0
  let bits = 0
  const out: number[] = []
  for (const ch of s) {
    const v = TABLE.indexOf(ch)
    if (v < 0)
      throw new Error(`非法 base32 字符：${ch}`)
    val = (val << 5) | v
    bits += 5
    if (bits >= 8) {
      bits -= 8
      out.push((val >>> bits) & 0xFF)
    }
  }
  return new Uint8Array(out)
}

function refDecodeLower(s: string): string {
  return new TextDecoder().decode(refDecodeBytes(s))
}

/* ── 测试专用的手工组帧 ───────────────────────────────────────────────────── */

/**
 * 按协议 K 的形态组一帧：`0<magic>0<flags>0<crc>0<tail…>1`。
 *
 * `flags` 传数字（0..31），这里自己查 base32 表 —— 不调用被测代码。
 * `withCrc = false` 用来手工造「crc 与覆盖内容不匹配」的帧。
 *
 * CRC 覆盖 `magic 0 flags 0 tail`（除引导符与 crc 字段本身之外的一切）。
 */
function frame(magic: string, flags: number, tail: string[], opts: { withCrc?: boolean, crc?: string } = {}): string {
  const tailText = tail.join('0')
  const flagsText = TABLE[flags & 31]
  const covered = `${magic}0${flagsText}0${tailText}`
  const crc = opts.withCrc === false ? (opts.crc ?? 'aaaaaaa') : (opts.crc ?? crcField(covered))
  return `0${magic}0${flagsText}0${crc}0${tailText}1`
}

/** 独立算 crc 字段：CRC32(被覆盖的字符) → 4 字节大端 → base32 小写。 */
function crcField(text: string): string {
  return refEncodeBytes(u32be(crc32Ref(text)))
}

function u32be(n: number): Uint8Array {
  return new Uint8Array([(n >>> 24) & 0xFF, (n >>> 16) & 0xFF, (n >>> 8) & 0xFF, n & 0xFF])
}

/** 独立 CRC32（IEEE），与 `../crc32` 无共享代码。 */
function crc32Ref(text: string): number {
  const data = new TextEncoder().encode(text)
  let crc = 0xFFFFFFFF
  for (const byte of data) {
    crc ^= byte
    for (let k = 0; k < 8; k++)
      crc = crc & 1 ? 0xEDB88320 ^ (crc >>> 1) : crc >>> 1
  }
  return (crc ^ 0xFFFFFFFF) >>> 0
}

/* ── golden vector ────────────────────────────────────────────────────────── */

// 下面这些常量是**golden vector**：先用本文件的 builder 跑出真实产物，再把字面量
// 硬编码回测试。它们钉死的是「线上长什么样」，不是「代码怎么实现」——
// 任何人改动字段顺序、flags 取值、crc 的算法或大小写，都必须在这里显式改字面量并
// 解释原因（而不是让测试跟着实现一起漂移）。
const GOLDEN_TEXT = '0kba0a0rh5kcra0nbswy3dp1'
const GOLDEN_PROBE = '0kbb0a0rknpegq0gi0geya0gu0mexge2lo0aaaaaaaaaaaaaaaaaaaaaaaaaa1'
const GOLDEN_CHUNK = '0kbc0a0gvf4pty0ga0gi0geya0gu0nbswy3dp1'
/** zstd 位打开时，只有 flags 字段和 crc 变。 */
const GOLDEN_TEXT_ZSTD = '0kba0b0mlgrury0nbswy3dp1'
/** 0 字节文件的最后一片：空 payload，帧以 `01` 结尾。 */
const GOLDEN_CHUNK_EMPTY_ZSTD = '0kbc0b0tvlj3wi0ge0gi0geya0gu01'

const FILE_MD5 = 'a'.repeat(26)

describe('golden vector（线上形态一旦改动就必须显式改这里）', () => {
  it('kba：`hello` 的 base32 负载，逐字符钉死', () => {
    expect(buildTextFrame(refEncodeLower('hello'), false)).toBe(GOLDEN_TEXT)
    // 手工拆一遍，确认 golden 串的每一段都符合协议（不是「实现自己说自己对」）
    expect(GOLDEN_TEXT).toBe('0kba0a0rh5kcra0nbswy3dp1')
    expect(GOLDEN_TEXT.slice(1, -1).split('0')).toEqual(['kba', 'a', 'rh5kcra', 'nbswy3dp'])
    expect(refDecodeLower('nbswy3dp')).toBe('hello')
  })

  it('kba：zstd 标志位只体现在 flags 与 crc 上', () => {
    expect(buildTextFrame(refEncodeLower('hello'), true)).toBe(GOLDEN_TEXT_ZSTD)
  })

  it('kbb：`a.bin`，total=2 size=10 chunkSize=5', () => {
    const built = buildProbeFrame({ total: 2, size: 10, chunkSize: 5, name: 'a.bin', zstd: false, fileMd5: FILE_MD5 })
    expect(built).toBe(GOLDEN_PROBE)
    // 字段顺序：total/size/chunkSize/name(编码后)/fileMd5
    expect(built.slice(1, -1).split('0').slice(3)).toEqual(['gi', 'geya', 'gu', 'mexge2lo', FILE_MD5])
    expect(refDecodeLower('mexge2lo')).toBe('a.bin')
  })

  it('kbc：seq=0 total=2 size=10 chunkSize=5 payload=`hello`', () => {
    const built = buildChunkFrame({ seq: 0, total: 2, size: 10, chunkSize: 5, payload: refEncodeLower('hello'), zstd: false })
    expect(built).toBe(GOLDEN_CHUNK)
    expect(built.slice(1, -1).split('0').slice(3)).toEqual(['ga', 'gi', 'geya', 'gu', 'nbswy3dp'])
  })

  it('kbc：空 payload + zstd 时帧以 `01` 结尾', () => {
    const built = buildChunkFrame({ seq: 1, total: 2, size: 10, chunkSize: 5, payload: '', zstd: true })
    expect(built).toBe(GOLDEN_CHUNK_EMPTY_ZSTD)
    expect(built.endsWith('01')).toBe(true)
  })

  it('magic 常量就是三个字面量，且都是合法 base32 字符', () => {
    expect([K_MAGIC_TEXT, K_MAGIC_PROBE, K_MAGIC_CHUNK]).toEqual(['kba', 'kbb', 'kbc'])
    for (const m of [K_MAGIC_TEXT, K_MAGIC_PROBE, K_MAGIC_CHUNK])
      expect(m).toMatch(/^[a-z2-7]+$/)
    expect(K_FLAG_ZSTD).toBe(1)
    // 全部 golden 帧的字符集：除定界符外只有 [a-z2-7]
    for (const golden of [GOLDEN_TEXT, GOLDEN_PROBE, GOLDEN_CHUNK, GOLDEN_TEXT_ZSTD, GOLDEN_CHUNK_EMPTY_ZSTD]) {
      expect(golden.startsWith('0')).toBe(true)
      expect(golden.endsWith('1')).toBe(true)
      // 用 split/join 而不是 replaceAll：仓库的 lib 目标包含 ES2020
      expect(golden.slice(1, -1).split('0').join('')).toMatch(/^[a-z2-7]+$/)
    }
  })
})

/* ── 往返 ─────────────────────────────────────────────────────────────────── */

describe('build → parse 往返', () => {
  it('kba：zstd 开 / 关', () => {
    for (const zstd of [false, true]) {
      const payload = refEncodeLower('中文 + emoji 🚀 / 122')
      const parsed = parseKFrame(buildTextFrame(payload, zstd))
      expect(parsed).toEqual({ kind: 'text', zstd, payload })
    }
  })

  it('kbb：zstd 开 / 关（含中文文件名）', () => {
    for (const zstd of [false, true]) {
      const name = '季度报告 final (1).pdf'
      const built = buildProbeFrame({ total: 12, size: 49152, chunkSize: 4096, name, zstd, fileMd5: FILE_MD5 })
      expect(parseKFrame(built)).toEqual({ kind: 'probe', zstd, total: 12, size: 49152, chunkSize: 4096, name, fileMd5: FILE_MD5 })
    }
  })

  it('kbc：zstd 开 / 关', () => {
    for (const zstd of [false, true]) {
      const built = buildChunkFrame({ seq: 3, total: 12, size: 49152, chunkSize: 4096, payload: refEncodeLower('chunk-3'), zstd })
      expect(parseKFrame(built)).toEqual({ kind: 'chunk', zstd, seq: 3, total: 12, size: 49152, chunkSize: 4096, payload: refEncodeLower('chunk-3') })
    }
  })

  it('zstd 位只影响 zstd 字段（其余字段逐字节相同）', () => {
    const off = parseKFrame(buildTextFrame('nbswy3dp', false))
    const on = parseKFrame(buildTextFrame('nbswy3dp', true))
    expect({ ...off, zstd: null }).toEqual({ ...on, zstd: null })
    expect(off?.zstd).toBe(false)
    expect(on?.zstd).toBe(true)
  })

  it('前后空白容错（键盘输入可能带空格）', () => {
    expect(parseKFrame(`  ${GOLDEN_TEXT}  `)?.kind).toBe('text')
    expect(parseKFrame(`\t${GOLDEN_CHUNK}\n`)?.kind).toBe('chunk')
  })
})

/* ── 字符集不变量 ─────────────────────────────────────────────────────────── */

describe('字符集不变量', () => {
  const ALL = 'abcdefghijklmnopqrstuvwxyz0123456789'

  it('负载含全部小写字母与数字，帧里除 0/1 外只有 [a-z2-7]', () => {
    // 负载先编码（这正是纪律的落点：明文里的 0/1 不会出现在线上）
    const payload = refEncodeLower(ALL)
    for (const built of [
      buildTextFrame(payload, false),
      buildChunkFrame({ seq: 0, total: 1, size: ALL.length, chunkSize: 64, payload, zstd: false }),
    ]) {
      const body = built.slice(1, -1)
      // body 里剩下的 `0` 全是分隔符；每段的字符集必须在 base32 表内
      for (const field of body.split('0'))
        expect(field, `字段 ${field}`).toMatch(/^[a-z2-7]+$/)
      expect(built.slice(0, -1)).not.toContain('1') // 1 只能出现在整帧结尾
    }
  })

  it('探测帧的名字也是编码过的（明文里的 0/1 不会泄漏到线上）', () => {
    const name = 'a0b1c.txt'
    const built = buildProbeFrame({ total: 1, size: 9, chunkSize: 9, name, zstd: false, fileMd5: FILE_MD5 })
    // 明文的 `0`/`1` 必须已经被编码：body 切出来的段数 = magic+flags+crc+5 个 tail 字段
    expect(built.slice(1, -1).split('0')).toHaveLength(8)
    expect(parseKFrame(built)?.kind === 'probe' && (parseKFrame(built) as { name: string }).name).toBe(name)
  })
})

/* ── CRC 覆盖线上 tail ────────────────────────────────────────────────────── */

describe('crc（覆盖 `magic 0 flags 0 tail`）', () => {
  it('改一个 tail 字符但不重算 crc → 拒绝', () => {
    // golden：tail = `ga0gi0geya0gu0nbswy3dp`，把首字段 `ga`（seq=0）改成 `ge`（seq=1）
    const tampered = GOLDEN_CHUNK.replace('0ga0', '0ge0')
    expect(tampered).not.toBe(GOLDEN_CHUNK)
    expect(parseKFrame(GOLDEN_CHUNK)?.kind).toBe('chunk')
    expect(parseKFrame(tampered)).toBeNull()
  })

  it('改 flags 但不重算 crc → 拒绝（flags 在校验覆盖范围内）', () => {
    // 这是「flags 必须进 CRC 覆盖面」的回归测试。`flags` 的 bit0 决定载荷要不要解压，
    // 若它落在校验之外，一个被敲坏的字符会**静默**走进错误的解码分支：
    // 把未压缩的当压缩解（解压报错），或把压缩的当未压缩（得到乱码字节）。
    const tampered = GOLDEN_TEXT.replace('0kba0a0', '0kba0b0')
    expect(tampered).not.toBe(GOLDEN_TEXT)
    expect(parseKFrame(tampered)).toBeNull()
  })

  it('改 flags 且重算 crc → 合法（bit0 是已知位，翻转它就是合法的另一帧）', () => {
    const rebuilt = frame(K_MAGIC_TEXT, 1, ['nbswy3dp'])
    expect(parseKFrame(rebuilt)).toEqual({ kind: 'text', zstd: true, payload: 'nbswy3dp' })
  })

  it('crc 的覆盖内容确实包含 magic（隔离验证，不依赖其它检查）', () => {
    // 「改 magic 会被拒」这件事其实无法单独由 CRC 证明 —— 三个 magic 的 tail 字段数不同
    // （kba 是 1 个，kbb/kbc 是 5 个），所以字段数检查会先命中。
    // 这里直接比对 crc 字段本身：flags 与 tail 完全相同时，只换 magic，crc 必须不同。
    const sameCovered = crcField('kba0a0nbswy3dp')
    expect(crcField('kbc0a0nbswy3dp')).not.toBe(sameCovered)
    // 反过来：覆盖内容逐字节相同 → crc 必然相同（crc 是内容的函数）
    expect(crcField('kba0a0nbswy3dp')).toBe(sameCovered)
    // 并且它与被测 builder 产物里的 crc 字段一致
    expect(GOLDEN_TEXT.split('0')[3]).toBe(sameCovered)
  })

  it('编码后等价的 tail 仍然解析成功（crc 是「字符」的函数，不是「写法」的函数）', () => {
    // 用独立参照实现重算整条帧：负载的编码与实现无关，等价写法必须得到同一串
    const payload = refEncodeLower('同一段文本')
    const tail = ['ga', 'gi', 'geya', 'gu', payload]
    const raw = frame(K_MAGIC_CHUNK, 0, tail)
    const parsed = parseKFrame(raw)
    expect(parsed?.kind).toBe('chunk')
    // 与被测 builder 的产物逐字节相同
    expect(raw).toBe(buildChunkFrame({ seq: 0, total: 2, size: 10, chunkSize: 5, payload, zstd: false }))
    // 而「同样的语义、不同的编码写法」不成立：base32 编码是确定的，所以这里只验证
    // 参照实现与实现产物一致，没有第二条合法写法可以偷换
    expect(refDecodeLower(payload)).toBe('同一段文本')
  })

  it('crc 字段本身写错（字符集合法）→ 拒绝', () => {
    const raw = frame(K_MAGIC_TEXT, 0, ['nbswy3dp'], { withCrc: false, crc: 'aaaaaaa' })
    expect(parseKFrame(raw)).toBeNull()
  })

  it('crc 字段长度不对 → 拒绝', () => {
    expect(parseKFrame('0kba0a0doqbrx0nbswy3dp1')).toBeNull()
    expect(parseKFrame('0kba0a0doqbrxaa0nbswy3dp1')).toBeNull()
  })
})

/* ── 空载荷 ───────────────────────────────────────────────────────────────── */

describe('空载荷（0 字节文件 → 最后一片 payload 为空）', () => {
  it('kbc 空 payload 解析正确', () => {
    const parsed = parseKFrame(GOLDEN_CHUNK_EMPTY_ZSTD)
    expect(parsed).toEqual({ kind: 'chunk', zstd: true, seq: 1, total: 2, size: 10, chunkSize: 5, payload: '' })
  })

  it('kba 空 payload 解析正确', () => {
    const built = buildTextFrame('', false)
    expect(built.slice(1, -1).split('0')).toEqual(['kba', 'a', expect.any(String), ''])
    expect(parseKFrame(built)).toEqual({ kind: 'text', zstd: false, payload: '' })
  })

  it('空字段只允许出现在最后一位：末尾 payload 之外的空字段一律拒绝', () => {
    // 每一条都是「某个非末字段为空」，必须拒绝
    const cases = [
      frame(K_MAGIC_CHUNK, 0, ['', 'gi', 'geya', 'gu', 'nbswy3dp']),
      frame(K_MAGIC_CHUNK, 0, ['ga', '', 'geya', 'gu', 'nbswy3dp']),
      frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'geya', '', 'nbswy3dp']),
      // flags 为空（`0kba00…`）：不是单个 base32 数字
      `0kba00${crcField('nbswy3dp')}0nbswy3dp1`,
      // crc 为空
      frame(K_MAGIC_TEXT, 0, ['nbswy3dp'], { crc: '' }),
      // 探测帧的末字段是 fileMd5（不是负载），同样不许为空
      frame(K_MAGIC_PROBE, 0, ['gi', 'geya', 'gu', 'mexge2lo', '']),
    ]
    for (const raw of cases)
      expect(parseKFrame(raw), raw).toBeNull()
  })

  it('非末位空字段与末位空载荷的形状差别被区分开', () => {
    // seq 为空 → 拒绝；seq 有值而 payload 为空 → 合法（0 字节文件的最后一片）
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['', 'gi', 'geya', 'gu']))).toBeNull()
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'geya', 'gu', '']))).toEqual({
      kind: 'chunk',
      zstd: false,
      seq: 0,
      total: 2,
      size: 10,
      chunkSize: 5,
      payload: '',
    })
  })

  it('两个连续的空字段（`00` 结尾）也被拒绝：字段数对不上', () => {
    // `0kbc0a0<crc>0ga0gi0geya0gu001`：空 payload 之后又多一个空字段
    const raw = `${GOLDEN_CHUNK_EMPTY_ZSTD.slice(0, -2)}001`
    expect(parseKFrame(raw)).toBeNull()
  })
})

/* ── 拒绝路径 ─────────────────────────────────────────────────────────────── */

describe('parseKFrame 拒绝路径', () => {
  it('未知 magic', () => {
    expect(parseKFrame(frame('kbd', 0, ['nbswy3dp']))).toBeNull()
    expect(parseKFrame(frame('zzz', 0, ['nbswy3dp']))).toBeNull()
    expect(parseKFrame(frame('KBA', 0, ['nbswy3dp']))).toBeNull()
  })

  it('字段数不符', () => {
    // kba 多带一个字段
    expect(parseKFrame(frame(K_MAGIC_TEXT, 0, ['nbswy3dp', 'ga']))).toBeNull()
    // kbc 少一个字段（缺 chunkSize）
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'geya', 'nbswy3dp']))).toBeNull()
    // kbc 多一个字段
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'geya', 'gu', 'nbswy3dp', 'ga']))).toBeNull()
    // kbb 少一个字段
    expect(parseKFrame(frame(K_MAGIC_PROBE, 0, ['gi', 'geya', 'gu', 'mexge2lo']))).toBeNull()
  })

  it('没有引导符 / 没有终结符 / 空串 / 只有定界符', () => {
    expect(parseKFrame('')).toBeNull()
    expect(parseKFrame(GOLDEN_TEXT.slice(1))).toBeNull()
    expect(parseKFrame(GOLDEN_TEXT.slice(0, -1))).toBeNull()
    expect(parseKFrame('01')).toBeNull()
    expect(parseKFrame('0')).toBeNull()
    expect(parseKFrame('1')).toBeNull()
  })

  it('字段含 [a-z2-7] 之外的字符', () => {
    // 大写（base32 大写不在键盘通道字母表里）
    expect(parseKFrame(frame(K_MAGIC_TEXT, 0, ['NBSWY3DP']))).toBeNull()
    // `0`/`1` 之外的非法字符：`!` 会挂在 crc 校验之前（crc 是照含 `!` 的 tail 算的，
    // 所以这里必须手工造 crc 匹配的帧，看的是字符集检查那一关）
    const withBang = frame(K_MAGIC_TEXT, 0, ['nbswy3d!'])
    expect(parseKFrame(withBang)).toBeNull()
    // 中文（未编码）也不可能通过
    expect(parseKFrame(frame(K_MAGIC_TEXT, 0, ['你好']))).toBeNull()
  })

  it('数值解不出 / 越界', () => {
    // 非数字编码（`ab` 解出来是 NUL 字节）
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ab', 'gi', 'geya', 'gu', 'nbswy3dp']))).toBeNull()
    // total < 1
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'ga', 'geya', 'gu', 'nbswy3dp']))).toBeNull()
    // seq === total（2 / 2）
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, [numB32(2), numB32(2), 'geya', 'gu', 'nbswy3dp']))).toBeNull()
    // seq > total（3 / 2）
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, [numB32(3), numB32(2), 'geya', 'gu', 'nbswy3dp']))).toBeNull()
    // chunkSize < 1
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'geya', 'ga', 'nbswy3dp']))).toBeNull()
    // 探测帧 total < 1
    expect(parseKFrame(frame(K_MAGIC_PROBE, 0, ['ga', 'geya', 'gu', 'mexge2lo', FILE_MD5]))).toBeNull()
    // 边界：seq === total - 1 是合法的最后一片
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, [numB32(1), numB32(2), 'geya', 'gu', 'nbswy3dp']))?.kind).toBe('chunk')
  })

  it('size < 0 不可能出现：numB32 只编码非负整数，带 `-` 的编码解不出数字', () => {
    // `fuyq` 是 numB32(-1) 的形态；parseNumB32 只认纯十进制，所以它解不出
    expect(parseNumB32('fuyq')).toBeNull()
    expect(parseKFrame(frame(K_MAGIC_CHUNK, 0, ['ga', 'gi', 'fuyq', 'gu', 'nbswy3dp']))).toBeNull()
  })

  it('fileMd5 长度不是 26', () => {
    expect(parseKFrame(frame(K_MAGIC_PROBE, 0, ['gi', 'geya', 'gu', 'mexge2lo', 'short']))).toBeNull()
    expect(parseKFrame(frame(K_MAGIC_PROBE, 0, ['gi', 'geya', 'gu', 'mexge2lo', 'a'.repeat(27)]))).toBeNull()
  })

  it('flags 出现未知位 → 拒绝（保留位不是「忽略就行」）', () => {
    // flags=2（bit1，本协议未定义）
    const raw = frame(K_MAGIC_TEXT, 2, ['nbswy3dp'])
    expect(parseKFrame(raw)).toBeNull()
    // flags 长度不是 1
    const twoChars = `0kba0aa0${crcField('nbswy3dp')}0nbswy3dp1`
    expect(parseKFrame(twoChars)).toBeNull()
    // flags 不是 base32 数字
    const notDigit = `0kba0A0${crcField('nbswy3dp')}0nbswy3dp1`
    expect(parseKFrame(notDigit)).toBeNull()
  })

  it('带 `0` 的负载被切碎后字段数不符 → 拒绝（不是「静默截断」）', () => {
    // 模拟负载里混进 `0`：字段数从 5 变成 6
    const raw = `0kbc0a0${crcField('ga0gi0geya0gu0nb0swy3dp')}0ga0gi0geya0gu0nb0swy3dp1`
    expect(parseKFrame(raw)).toBeNull()
  })
})

/* ── numB32 ───────────────────────────────────────────────────────────────── */

describe('numB32 / parseNumB32', () => {
  it('golden：0/1/4/9/31/4096/100000 的编码逐个钉死', () => {
    const table: [number, string][] = [
      [0, 'ga'],
      [1, 'ge'],
      [4, 'gq'],
      [9, 'he'],
      [31, 'gmyq'],
      [4096, 'gqydsnq'],
      [100000, 'geydambqga'],
    ]
    for (const [value, expected] of table)
      expect(numB32(value), `numB32(${value})`).toBe(expected)
  })

  it('往返闭合', () => {
    for (const value of [0, 1, 4, 9, 31, 4096, 100000])
      expect(parseNumB32(numB32(value)), `parseNumB32(numB32(${value}))`).toBe(value)
  })

  it('与独立参照实现一致（numB32 = base32lower(十进制字符串)）', () => {
    for (const value of [0, 7, 10, 99, 100, 255, 4096, 123456, 2 ** 31]) {
      expect(numB32(value), `numB32(${value})`).toBe(refEncodeLower(String(value)))
      expect(refDecodeLower(numB32(value))).toBe(String(value))
    }
  })

  it('产物只含 [a-z2-7]，绝不含 0/1', () => {
    for (let n = 0; n < 2000; n++)
      expect(numB32(n), `numB32(${n})`).toMatch(/^[a-z2-7]+$/)
  })

  it('拒绝非数字 / 非法字符', () => {
    for (const bad of ['', '0', '1', 'ga0', 'ga1', 'GA', 'NBSWY3DP', 'fuyq', 'ab', 'nbswy3dp!', '你好'])
      expect(parseNumB32(bad), `parseNumB32(${bad})`).toBeNull()
  })

  it(`\`ab\` 能解码（1 个 NUL 字节），但不是十进制串 → 拒绝`, () => {
    // 说明「能解码」不等于「是数字」：parseNumB32 必须再看一遍文本形态
    expect([...refDecodeBytes('ab')]).toEqual([0])
    expect(parseNumB32('ab')).toBeNull()
  })
})
