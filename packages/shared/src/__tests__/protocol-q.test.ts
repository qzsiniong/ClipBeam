// 协议 Q（二维码通道）帧、反馈消息、重组器与缺失区间的单元测试。
//
// 盯住五件事：
//   1. **golden vector**：QBA / QBB 各钉死一整条帧文本（含 crc 那 7 个字符）；
//   2. **整条消息的 crc**：同一条消息的每一片共用同一个 crc（批次号），
//      单帧解析**不**校验它，重组之后由 `finishQMessage` 校验；
//   3. **重组**：乱序到达、重复帧幂等、不同 crc 不串批；
//   4. **缺失区间**：往返闭合 + 所有畸形输入都返回 null；
//   5. **parseFeedback 容错**：五种状态、未知键忽略、垃圾返回 null 而不是抛异常。
//
// 测试里的 base32 用**独立实现**（下面几十行），不复用 `../b32`。

import type { Feedback } from '../protocol-q'
import { describe, expect, it } from 'vitest'
import {
  buildFeedbackFrame,
  buildQFrame,
  createQAssembler,
  finishQMessage,
  formatMissingRanges,
  parseFeedback,
  parseMissingRanges,
  parseQFrame,
  Q_MAGIC_CLIP,
  Q_MAGIC_CONTROL,
  qPayloadCrc,
} from '../protocol-q'

/* ── 独立参照实现（RFC4648 base32 无填充） ────────────────────────────────── */

const TABLE = 'abcdefghijklmnopqrstuvwxyz234567'

function refEncode(s: string, upper: boolean): string {
  const bytes = new TextEncoder().encode(s)
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
  return upper ? out.toUpperCase() : out
}

function refDecodeBytes(s: string): Uint8Array {
  const table = 'abcdefghijklmnopqrstuvwxyz234567'
  const src = s.toLowerCase()
  let val = 0
  let bits = 0
  const out: number[] = []
  for (const ch of src) {
    const v = table.indexOf(ch)
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

const refUpper = (s: string) => refEncode(s, true)
const refLower = (s: string) => refEncode(s, false)

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

/** 独立算 crc 字段：CRC32(整条负载字符) → 4 字节大端 → base32 大写。 */
function refPayloadCrc(wholePayload: string): string {
  const n = crc32Ref(wholePayload)
  return refEncodeBytesToUpper([(n >>> 24) & 0xFF, (n >>> 16) & 0xFF, (n >>> 8) & 0xFF, n & 0xFF])
}

function refEncodeBytesToUpper(bytes: number[]): string {
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
  return out.toUpperCase()
}

/* ── golden vector ────────────────────────────────────────────────────────── */

// **golden vector**：先用本文件的 builder 跑出真实产物，再把字面量硬编码回测试。
// 它们钉死的是「线上长什么样」：字段顺序、crc 的算法与大小写、负载编码。任何一次
// 「顺手重构」只要改动了线上字节，这里必然红 —— 想改就显式改字面量并说明原因。
//
// `hello` 的 base32 大写是 `NBSWY3DP`，它的 crc 是 `4XPME4A`
// （另一份独立实现 Python 验证过：zlib.crc32(b'NBSWY3DP') = 0xe5dec270）。
const GOLDEN_QBA_HELLO = 'QBA.1.0.4XPME4A.NBSWY3DP'
/** `hello world` 的整条负载与它的 crc（分 3 片时三片共用同一个 crc）。 */
const GOLDEN_QBA_WHOLE = 'NBSWY3DPEB3W64TMMQ'
const GOLDEN_QBA_WHOLE_CRC = 'I4XGHRQ'
const GOLDEN_QBA_FRAMES = [
  'QBA.3.0.I4XGHRQ.NBSWY',
  'QBA.3.1.I4XGHRQ.3DPEB',
  'QBA.3.2.I4XGHRQ.3W64TMMQ',
]
/** `{"type":"feedback","status":"unauthorized"}` 的单帧反馈。 */
const GOLDEN_QBB_UNAUTHORIZED = 'QBB.1.0.IAQLFJY.PMRHI6LQMURDUITGMVSWIYTBMNVSELBCON2GC5DVOMRDUITVNZQXK5DIN5ZGS6TFMQRH2'

describe('golden vector（线上形态一旦改动就必须显式改这里）', () => {
  it('qba 单帧：`hello`', () => {
    expect(buildQFrame(Q_MAGIC_CLIP, 1, 0, qPayloadCrc(refUpper('hello')), refUpper('hello'))).toBe(GOLDEN_QBA_HELLO)
    expect(GOLDEN_QBA_HELLO.split('.')).toEqual(['QBA', '1', '0', '4XPME4A', 'NBSWY3DP'])
    expect(new TextDecoder().decode(refDecodeBytes('NBSWY3DP'))).toBe('hello')
  })

  it('crc 字段与独立参照实现一致（含大小写）', () => {
    expect(qPayloadCrc(refUpper('hello'))).toBe('4XPME4A')
    expect(qPayloadCrc(GOLDEN_QBA_WHOLE)).toBe(GOLDEN_QBA_WHOLE_CRC)
    // 参照实现（独立 CRC32 + 独立 base32）必须给出同一个 7 字符
    expect(refPayloadCrc(refUpper('hello'))).toBe('4XPME4A')
    expect(refPayloadCrc(GOLDEN_QBA_WHOLE)).toBe(GOLDEN_QBA_WHOLE_CRC)
    // 大写：与小写形态不是同一个串（Q 通道的线上负载一律大写）
    expect(qPayloadCrc(refLower('hello'))).not.toBe('4XPME4A')
  })

  it('qba 分 3 片：每片都带同一个 crc', () => {
    const whole = refUpper('hello world')
    const crc = qPayloadCrc(whole)
    const frags = [whole.slice(0, 5), whole.slice(5, 10), whole.slice(10)]
    const frames = frags.map((f, i) => buildQFrame(Q_MAGIC_CLIP, 3, i, crc, f))
    expect(frames).toEqual(GOLDEN_QBA_FRAMES)
    for (const raw of frames)
      expect(raw.split('.')[3]).toBe(crc)
  })

  it('qbb 单帧反馈：`unauthorized`', () => {
    expect(buildFeedbackFrame({ type: 'feedback', status: 'unauthorized' })).toBe(GOLDEN_QBB_UNAUTHORIZED)
  })

  it('magic 常量与分隔符', () => {
    expect([Q_MAGIC_CLIP, Q_MAGIC_CONTROL]).toEqual(['QBA', 'QBB'])
    expect(GOLDEN_QBA_HELLO.includes('.')).toBe(true)
    expect(GOLDEN_QBB_UNAUTHORIZED.startsWith('QBB.')).toBe(true)
  })
})

/* ── parseQFrame ──────────────────────────────────────────────────────────── */

describe('parseQFrame', () => {
  it('往返：拆出全部字段', () => {
    const parsed = parseQFrame(GOLDEN_QBA_HELLO)
    expect(parsed).toEqual({ magic: 'QBA', total: 1, index: 0, crc: '4XPME4A', payload: 'NBSWY3DP' })
  })

  it('前后空白容错', () => {
    expect(parseQFrame(`  ${GOLDEN_QBA_HELLO}\n`)?.payload).toBe('NBSWY3DP')
  })

  it('接受空 payload（整条消息为空时的单帧）', () => {
    const raw = buildQFrame('QBA', 1, 0, qPayloadCrc(''), '')
    expect(qPayloadCrc('')).toBe(refPayloadCrc(''))
    expect(parseQFrame(raw)).toEqual({ magic: 'QBA', total: 1, index: 0, crc: qPayloadCrc(''), payload: '' })
  })

  it('接受「crc 与本片负载对不上」的帧 —— 这是合法的（crc 属于整条消息）', () => {
    // 分片帧的 crc 是整条消息的摘要，拿单片去比必然不等
    const frame = parseQFrame(GOLDEN_QBA_FRAMES[0])
    expect(frame).not.toBeNull()
    expect(frame!.crc).not.toBe(qPayloadCrc(frame!.payload))
    expect(parseQFrame(GOLDEN_QBA_FRAMES[0])).not.toBeNull()
  })

  it('拒绝：未知 magic / 小写 magic', () => {
    for (const bad of ['QBC.1.0.4XPME4A.NBSWY3DP', 'qba.1.0.4XPME4A.NBSWY3DP', 'CB1.1.0.4XPME4A.NBSWY3DP', 'ZZZ.1.0.4XPME4A.NBSWY3DP'])
      expect(parseQFrame(bad), bad).toBeNull()
  })

  it('拒绝：字段数不是 5', () => {
    for (const bad of [
      'QBA.1.0.4XPME4A',
      'QBA.1.0.4XPME4A.NBSWY3DP.extra',
      'QBA.1.0..NBSWY3DP',
      '',
      'QBA',
    ])
      expect(parseQFrame(bad), bad).toBeNull()
  })

  it('拒绝：total < 1 / index 越界 / 非十进制', () => {
    const crc = '4XPME4A'
    for (const bad of [
      `QBA.0.0.${crc}.NBSWY3DP`,
      `QBA.1.1.${crc}.NBSWY3DP`,
      `QBA.2.2.${crc}.NBSWY3DP`,
      `QBA.-1.0.${crc}.NBSWY3DP`,
      `QBA.x.0.${crc}.NBSWY3DP`,
      `QBA.1.x.${crc}.NBSWY3DP`,
      // `Number.parseInt` 会接受这些；本协议的十进制字段必须是纯数字
      `QBA.1e1.0.${crc}.NBSWY3DP`,
      `QBA. 1.0.${crc}.NBSWY3DP`,
    ])
      expect(parseQFrame(bad), bad).toBeNull()
    // 整串的**首尾**空白是允许的（扫码/剪贴板常见），由 `parseQFrame` 自己 trim
    expect(parseQFrame(`QBA.1.0.${crc}.NBSWY3DP `)?.payload).toBe('NBSWY3DP')
    expect(parseQFrame(` QBA.1.0.${crc}.NBSWY3DP`)?.payload).toBe('NBSWY3DP')
  })

  it('拒绝：crc 长度 / 字符集不对', () => {
    for (const crc of ['', '4XPME4', '4XPME4AA', '4xpmE4A', '4XPME4-', '4XPME40', '4XPME41'])
      expect(parseQFrame(`QBA.1.0.${crc}.NBSWY3DP`), crc).toBeNull()
  })

  it('拒绝：payload 字符集不对', () => {
    for (const payload of ['nbswy3dp', 'NBSWY3DP=', 'NBSWY3DP-', 'NBS 3DP', 'NBSW03DP', '你好'])
      expect(parseQFrame(`QBA.1.0.4XPME4A.${payload}`), payload).toBeNull()
  })
})

/* ── buildQFrame 的参数校验 ───────────────────────────────────────────────── */

describe('buildQFrame 参数校验', () => {
  it('未知 magic / total / index / crc / payload 非法时抛错', () => {
    expect(() => buildQFrame('QBC', 1, 0, '4XPME4A', 'NBSWY3DP')).toThrow()
    expect(() => buildQFrame('QBA', 0, 0, '4XPME4A', 'NBSWY3DP')).toThrow()
    expect(() => buildQFrame('QBA', 1, 1, '4XPME4A', 'NBSWY3DP')).toThrow()
    expect(() => buildQFrame('QBA', 1, 0, 'shrt', 'NBSWY3DP')).toThrow()
    expect(() => buildQFrame('QBA', 1, 0, '4XPME4A', 'lower')).toThrow()
    // 合法边界不抛
    expect(parseQFrame(buildQFrame('QBA', 1, 0, '4XPME4A', ''))?.payload).toBe('')
  })
})

/* ── finishQMessage ───────────────────────────────────────────────────────── */

describe('finishQMessage（重组后的整条消息校验）', () => {
  it('crc 对得上 → 返回负载', () => {
    expect(finishQMessage({ crc: GOLDEN_QBA_WHOLE_CRC, payload: GOLDEN_QBA_WHOLE })).toBe(GOLDEN_QBA_WHOLE)
  })

  it('负载被改过（crc 不变）→ null', () => {
    const tampered = `${GOLDEN_QBA_WHOLE.slice(0, -1)}A`
    expect(tampered).not.toBe(GOLDEN_QBA_WHOLE)
    expect(finishQMessage({ crc: GOLDEN_QBA_WHOLE_CRC, payload: tampered })).toBeNull()
    // 空负载配非空 crc 也拒
    expect(finishQMessage({ crc: GOLDEN_QBA_WHOLE_CRC, payload: '' })).toBeNull()
  })
})

/* ── createQAssembler ─────────────────────────────────────────────────────── */

/** 按 golden 帧造一个「切 3 片发 hello world」的发射端。 */
function makeBatch(text: string, total: number, magic = Q_MAGIC_CLIP) {
  const whole = refUpper(text)
  const crc = qPayloadCrc(whole)
  const per = Math.ceil(whole.length / total)
  const frames = Array.from({ length: total }, (_, i) =>
    buildQFrame(magic, total, i, crc, whole.slice(i * per, (i + 1) * per)))
  return { whole, crc, frames }
}

describe('createQAssembler（多帧重组）', () => {
  it('按序到达、并校验整条消息', () => {
    const { whole, crc, frames } = makeBatch('hello world', 3)
    const asm = createQAssembler()
    let done: { magic: string, crc: string, payload: string } | null = null
    for (const raw of frames) {
      const result = asm.ingest(parseQFrame(raw)!)
      if (result)
        done = result
    }
    expect(done).toEqual({ magic: 'QBA', crc, payload: whole })
    expect(finishQMessage(done!)).toBe(whole)
  })

  it('乱序到达：凑齐才返回，且顺序按 index 拼', () => {
    const { whole, crc, frames } = makeBatch('乱序也没关系', 3)
    const asm = createQAssembler()
    expect(asm.ingest(parseQFrame(frames[2])!)).toBeNull()
    expect(asm.ingest(parseQFrame(frames[0])!)).toBeNull()
    const done = asm.ingest(parseQFrame(frames[1])!)
    expect(done).toEqual({ magic: 'QBA', crc, payload: whole })
    expect(finishQMessage(done!)).toBe(whole)
  })

  it('重复帧幂等：同一片喂多次不改变结果，也不提前完成', () => {
    const { whole, crc, frames } = makeBatch('hello world', 3)
    const asm = createQAssembler()
    expect(asm.ingest(parseQFrame(frames[2])!)).toBeNull()
    expect(asm.ingest(parseQFrame(frames[2])!)).toBeNull()
    expect(asm.ingest(parseQFrame(frames[0])!)).toBeNull()
    expect(asm.ingest(parseQFrame(frames[0])!)).toBeNull()
    const done = asm.ingest(parseQFrame(frames[1])!)
    expect(done?.payload).toBe(whole)
    expect(done?.crc).toBe(crc)
    // 完成后重复喂任何一片都不会「再完成一次」
    expect(asm.ingest(parseQFrame(frames[1])!)).toBeNull()
    expect(asm.ingest(parseQFrame(frames[0])!)).toBeNull()
  })

  it('crc 不同（另一批）不会混进旧批', () => {
    const a = makeBatch('first message', 2)
    const b = makeBatch('second message!', 2)
    expect(a.crc).not.toBe(b.crc)
    const asm = createQAssembler()
    expect(asm.ingest(parseQFrame(a.frames[0])!)).toBeNull()
    // 另一批的第 0 片进的是另一个桶
    expect(asm.ingest(parseQFrame(b.frames[0])!)).toBeNull()
    // 旧批的第 1 片补上 → 旧批完成，内容还是旧的
    expect(asm.ingest(parseQFrame(a.frames[1])!)).toEqual({ magic: 'QBA', crc: a.crc, payload: a.whole })
    // 新批再补一片 → 新批完成
    expect(asm.ingest(parseQFrame(b.frames[1])!)).toEqual({ magic: 'QBA', crc: b.crc, payload: b.whole })
  })

  it('不同 magic / total 也不同桶', () => {
    const asm = createQAssembler()
    const clip = makeBatch('same text', 1, Q_MAGIC_CLIP)
    const ctrl = makeBatch('same text', 1, Q_MAGIC_CONTROL)
    expect(asm.ingest(parseQFrame(clip.frames[0])!)).toEqual({ magic: 'QBA', crc: clip.crc, payload: clip.whole })
    expect(asm.ingest(parseQFrame(ctrl.frames[0])!)).toEqual({ magic: 'QBB', crc: ctrl.crc, payload: ctrl.whole })
  })

  it('越界 index 的帧直接丢弃（不让一条消息永远差点数）', () => {
    const asm = createQAssembler()
    expect(asm.ingest({ magic: 'QBA', total: 2, index: 2, crc: '4XPME4A', payload: 'NBSWY3DP' })).toBeNull()
    expect(asm.ingest({ magic: 'QBA', total: 2, index: -1, crc: '4XPME4A', payload: 'NBSWY3DP' })).toBeNull()
    // 丢弃之后它不会污染批次：合法两片仍然能拼齐
    const one = buildQFrame('QBA', 2, 0, GOLDEN_QBA_WHOLE_CRC, 'NBSWY')
    const two = buildQFrame('QBA', 2, 1, GOLDEN_QBA_WHOLE_CRC, '3DPEB')
    expect(asm.ingest(parseQFrame(one)!)).toBeNull()
    expect(asm.ingest(parseQFrame(two)!)).toEqual({ magic: 'QBA', crc: GOLDEN_QBA_WHOLE_CRC, payload: 'NBSWY3DPEB' })
  })

  it('reset() 清空未完成的批次', () => {
    const { frames } = makeBatch('hello world', 2)
    const asm = createQAssembler()
    expect(asm.ingest(parseQFrame(frames[0])!)).toBeNull()
    asm.reset()
    // reset 之后第 1 片到达也不会完成（第 0 片已经不在桶里）
    expect(asm.ingest(parseQFrame(frames[1])!)).toBeNull()
  })

  it('单帧（total=1）一帧即完成', () => {
    const asm = createQAssembler()
    expect(asm.ingest(parseQFrame(GOLDEN_QBA_HELLO)!)).toEqual({
      magic: 'QBA',
      crc: '4XPME4A',
      payload: 'NBSWY3DP',
    })
  })
})

/* ── 缺失区间 ─────────────────────────────────────────────────────────────── */

describe('formatMissingRanges / parseMissingRanges', () => {
  it('golden：单点省掉 `-b`，多段用逗号连接', () => {
    expect(formatMissingRanges([[0, 2], [7, 7], [9, 11]])).toBe('0-2,7,9-11')
    expect(formatMissingRanges([[0, 0]])).toBe('0')
    expect(formatMissingRanges([])).toBe('')
  })

  it('往返闭合', () => {
    const cases: Array<Array<[number, number]>> = [
      [],
      [[0, 0]],
      [[0, 2], [7, 7], [9, 11]],
      [[5, 5000]],
      [[0, 0], [2, 2], [4, 4]],
    ]
    for (const ranges of cases) {
      const text = formatMissingRanges(ranges)
      expect(parseMissingRanges(text), text).toEqual(ranges)
    }
  })

  it('空串 → []（「什么都不缺」与「格式错误」必须分开）', () => {
    expect(parseMissingRanges('')).toEqual([])
  })

  it('拒绝：非 [0-9,-] 字符', () => {
    for (const bad of ['0-2, 7', '1..2', '+1', 'a', '1;2', '1-2\n', ' 1', '1 '])
      expect(parseMissingRanges(bad), JSON.stringify(bad)).toBeNull()
  })

  it('拒绝：空段 / 首尾逗号', () => {
    for (const bad of ['1,,2', ',1', '1,', ',', '0-2,'])
      expect(parseMissingRanges(bad), JSON.stringify(bad)).toBeNull()
  })

  it('拒绝：a > b / 非法区间写法', () => {
    for (const bad of ['5-3', '1-2-3', '1-', '-1', '--1', '0-2-', '3-3-3'])
      expect(parseMissingRanges(bad), JSON.stringify(bad)).toBeNull()
  })

  it('拒绝：降序 / 重叠', () => {
    for (const bad of ['5,3', '0-5,4-9', '0-5,5-9', '0-5,0', '1-2,2-3', '7,7'])
      expect(parseMissingRanges(bad), JSON.stringify(bad)).toBeNull()
  })

  it('接受：严格升序且不挨着（间隙允许）', () => {
    expect(parseMissingRanges('0-2,4-5,9')).toEqual([[0, 2], [4, 5], [9, 9]])
  })

  it('拒绝：超出安全整数的数字', () => {
    expect(parseMissingRanges('9007199254740993')).toBeNull()
    expect(parseMissingRanges(`0-${'9'.repeat(20)}`)).toBeNull()
  })
})

/* ── 反馈消息 ─────────────────────────────────────────────────────────────── */

describe('buildFeedbackFrame / parseFeedback', () => {
  const CASES: Feedback[] = [
    { type: 'feedback', status: 'unauthorized' },
    { type: 'feedback', status: 'ready' },
    { type: 'feedback', status: 'partial', missing: '0-2,7,9-11' },
    { type: 'feedback', status: 'complete', saved: '报告 final (1).pdf' },
    { type: 'feedback', status: 'error', reason: '整文件 MD5 不符' },
  ]

  it('五种状态各自往返（帧文本 → Feedback）', () => {
    for (const fb of CASES) {
      const raw = buildFeedbackFrame(fb)
      expect(parseQFrame(raw)?.magic).toBe(Q_MAGIC_CONTROL)
      expect(parseFeedback(raw), raw).toEqual(fb)
    }
  })

  it('同一条消息既能从「整帧文本」也能从「已重组负载」解析出来', () => {
    for (const fb of CASES) {
      const raw = buildFeedbackFrame(fb)
      const frame = parseQFrame(raw)!
      // 重组形态（base32 大写负载）
      expect(parseFeedback(frame.payload)).toEqual(fb)
      // 已解码的小写 base32 形态（中间态）
      expect(parseFeedback(refLower(new TextDecoder().decode(refDecodeBytes(frame.payload))))).toEqual(fb)
      // JSON 文本形态
      const json = new TextDecoder().decode(refDecodeBytes(frame.payload))
      expect(parseFeedback(json)).toEqual(fb)
      expect(parseFeedback(`  ${json}  `)).toEqual(fb)
    }
  })

  it('未知 JSON 键被忽略（向前兼容）', () => {
    const json = JSON.stringify({ type: 'feedback', status: 'partial', missing: '0-2', futureField: 42, nested: { a: 1 } })
    const payload = refUpper(json)
    expect(parseFeedback(json)).toEqual({ type: 'feedback', status: 'partial', missing: '0-2' })
    expect(parseFeedback(payload)).toEqual({ type: 'feedback', status: 'partial', missing: '0-2' })
    expect(parseFeedback(buildQFrame(Q_MAGIC_CONTROL, 1, 0, qPayloadCrc(payload), payload))).toEqual({
      type: 'feedback',
      status: 'partial',
      missing: '0-2',
    })
  })

  it('已知键类型不对时只当没带该键（不整条拒掉）', () => {
    const json = JSON.stringify({ type: 'feedback', status: 'partial', missing: 42, saved: ['x'] })
    expect(parseFeedback(json)).toEqual({ type: 'feedback', status: 'partial' })
  })

  it('拒绝：未知 status / 缺 status / 不是 feedback', () => {
    const bads = [
      JSON.stringify({ type: 'feedback', status: 'done' }),
      JSON.stringify({ type: 'feedback', status: 'READY' }),
      JSON.stringify({ type: 'feedback' }),
      JSON.stringify({ type: 'feedback', status: 42 }),
      JSON.stringify({ type: 'other', status: 'ready' }),
      JSON.stringify({ status: 'ready' }),
      JSON.stringify('feedback'),
      JSON.stringify([{ type: 'feedback', status: 'ready' }]),
      JSON.stringify(null),
      'null',
      '[]',
    ]
    for (const bad of bads)
      expect(parseFeedback(bad), bad).toBeNull()
  })

  it('拒绝：垃圾输入（不抛异常）', () => {
    const bads = [
      '',
      '   ',
      'not json at all',
      '{',
      '{"type":"feedback","status":',
      GOLDEN_QBA_HELLO, // QBA 不是控制消息
      'QBB.1.0.4XPME4A.NBSWY3DP', // 负载不是 JSON
      'QBB.1.0.4XPME4A.', // 空负载
      'QBC.1.0.4XPME4A.NBSWY3DP',
      'NBSWY3DP', // 大写 base32 但不是 JSON
      '!!!!!',
    ]
    for (const bad of bads)
      expect(parseFeedback(bad), JSON.stringify(bad)).toBeNull()
  })

  it('反馈帧恒定单帧，且 crc 是整条负载的摘要', () => {
    const raw = buildFeedbackFrame({ type: 'feedback', status: 'ready' })
    const frame = parseQFrame(raw)!
    expect(frame.total).toBe(1)
    expect(frame.index).toBe(0)
    expect(frame.crc).toBe(qPayloadCrc(frame.payload))
    expect(finishQMessage({ crc: frame.crc, payload: frame.payload })).toBe(frame.payload)
  })
})
