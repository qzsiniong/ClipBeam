/**
 * 协议 Q（二维码通道）：ClipBeam protocol v2 的二维码帧编解码 + 反馈消息。
 *
 * 与协议 K（{@link ./protocol-k}）是两条**故意不同**的通道：
 *
 * | | K（键盘） | Q（二维码） |
 * | --- | --- | --- |
 * | 谁发 | 宿主端敲键盘 | 接收页/宿主端屏幕 |
 * | 传送方式 | 字符逐个敲入 | 摄像头扫描，整帧原子到达 |
 * | 字面形态 | `0…0…1`，只能 `[a-z2-7]` | `<magic>.<total>.<index>.<crc>.<payload>`，大写 base32 + 十进制 |
 *
 * Q 通道**不需要** K 的字符集纪律：一帧就是一张二维码的全部内容，扫码是原子的，
 * 不存在「敲到一半没敲完」的中间态；分隔符 `.` 也不在 base32 字母表里，
 * 所以 `total`/`index` 直接用十进制可读性更好，调试时肉眼就能看出这是第几片。
 *
 * 线上形态：
 *
 * ```text
 * <magic>.<total>.<index>.<crc>.<payload>
 * ```
 *
 * - `magic`：`QBA` 剪贴板负载 / `QBB` 控制消息（JSON）。
 * - `total`/`index`：十进制整数，`0 <= index < total`。
 * - `crc`（7 字符）：{@link qPayloadCrc} —— **整条消息**负载（分帧之前的那一整串 base32）
 *   的 CRC32，再 base32 大写。同一条消息的**每一片都带同一个 crc**，它同时是两个东西：
 *   代码里的「批次号」，和重组完成后的「整条消息校验和」。
 * - `payload`：该片的大写 base32 无填充片段 —— `QBA` 是剪贴板文本 UTF-8、`QBB` 是 JSON
 *   UTF-8 经 base32 后再切开的一段（切点落在 base32 字符之间，不会破坏字节边界）。
 * - **没有终结符**：一张二维码就是完整一帧，不需要 `1` 这种「敲完了」的信号。
 *
 * ## 为什么 Q 的 CRC 覆盖整条消息，而 K 的覆盖单帧
 *
 * 因为两条通道的**投递语义不同**：
 *
 * - K（{@link ./protocol-k}）的每一帧都是**独立投递**的 —— 敲一次键、进一次剪贴板、
 *   发一班车。接收端可能只收到其中几帧，所以每一帧必须能**自证**没被敲错，CRC 只能覆盖
 *   它自己的 tail。
 * - Q 的这些帧是**同一组二维码在屏幕上轮播**，不是 N 次独立投递，而是同一条消息的 N 种
 *   视图：任一帧单独拿出来都不构成一条完整消息，自校验没有意义。校验和属于整条消息，
 *   落在一个**每片都相同**的 crc 字段上，于是它还免费当了批次号 —— 收到不同 crc 的片
 *   就知道是另一条消息。
 *
 * 这处不对称看上去像 bug，其实是两边投递模型不同的直接结果；改任何一边前先读这段。
 */

import { b32Decode, b32EncodeUpper } from './b32'
import crc32 from './crc32'

/** `QBA`：剪贴板负载。 */
export const Q_MAGIC_CLIP = 'QBA'
/** `QBB`：控制消息（JSON）。 */
export const Q_MAGIC_CONTROL = 'QBB'

/** 字段分隔符。`.` 不在 base32 字母表内，永远不会与负载混淆。 */
export const Q_SEP = '.'

/** 合法的 payload 字符集（大写 base32）。`*` 而不是 `+`：整条消息为空时只有一片空片段。 */
const PAYLOAD_RE = /^[A-Z2-7]*$/
/** `crc` 字段：7 字符大写 base32（crc32 的 4 字节 → base32 无填充恰好 7 字符）。 */
const CRC_RE = /^[A-Z2-7]{7}$/
/**
 * `parseFeedback` 兜底解码「已重组的负载」时认的字符集。
 *
 * Q 通道的线上负载一律是大写（QR alphanumeric 最优），但调用方也可能递进来一个小写的中间形态；
 * 两种都收，省得每个调用点各写一份 `toUpperCase()`。
 */
const B32_ANY_CASE_RE = /^[a-z2-7]+$/i
/** 十进制整数字段。不用 `Number.parseInt`：它会接受 `12abc`、` 12` 这类脏输入。 */
const DEC_RE = /^\d+$/

/** 反馈状态。 */
export type FeedbackStatus
  /** 用户还没在接收页授权保存目录 —— 宿主端应提示去点「选择保存目录」。 */
  = | 'unauthorized'
  /** 已授权，可以开始发送。 */
    | 'ready'
  /** 只收到一部分，`missing` 给出缺的片号区间。 */
    | 'partial'
  /** 全部收齐且整文件 MD5 校验通过，`saved` 是实际落盘的文件名。 */
    | 'complete'
  /** 出错，`reason` 给人类可读的文案。 */
    | 'error'

/** 反馈消息（`QBB` 里那坨 JSON）。 */
export interface Feedback {
  type: 'feedback'
  status: FeedbackStatus
  /** `partial` 时缺失的分片区间，形如 `0-2,7,9-11`（0 基、闭区间、逗号分隔、无空格）。 */
  missing?: string
  /**
   * `complete` 时实际写入的文件名。
   *
   * **可能与被请求的文件名不同**：目标目录已有同名文件时，接收页会自动改名
   * （`报告 final (1).pdf`），不告诉发送端「实际叫什么」，用户就会去找一个不存在的文件。
   */
  saved?: string
  /** `error` 时的人类可读原因。 */
  reason?: string
}

/** 解析出的一帧二维码消息。 */
export interface QFrame {
  magic: string
  total: number
  index: number
  crc: string
  payload: string
}

const enc = new TextEncoder()
const dec = new TextDecoder()

/** `feedback` 是唯一的控制消息类型；`type` 字段将来可扩展成别的消息。 */
const FEEDBACK_TYPE = 'feedback'

/** 已知状态取值。未知状态一律当「不是反馈」处理（向前兼容：将来新增状态时旧端静默忽略）。 */
const FEEDBACK_STATUSES: readonly FeedbackStatus[] = ['unauthorized', 'ready', 'partial', 'complete', 'error']

/** `QBA`/`QBB` 之外的 magic 一律拒绝。 */
function isKnownMagic(magic: string): boolean {
  return magic === Q_MAGIC_CLIP || magic === Q_MAGIC_CONTROL
}

/**
 * **整条消息**负载（分帧之前的那一整串大写 base32）的 CRC32 → 4 字节大端 → base32 大写（7 字符）。
 *
 * 同一条消息的每一帧都带这同一个值，所以它一举两得：
 * 1. **批次号**：`(magic, total, crc)` 三元组唯一标识一条正在重组中的消息，收到不同 crc
 *    的片就知道是另一条消息，不会混进旧批次；
 * 2. **整条消息的完整性校验**：重组完之后对拼起来的负载再算一次，就能发现「某一片内容
 *    是坏的」这种单帧无法自证的问题（见 {@link finishQMessage}）。
 *
 * 摘要形态与 K 通道的 `crc` 字段对齐（base32、7 字符，一低一大写），
 * 宿主侧解码器可以原样复用；正因为要对齐，这里必须用 `b32EncodeUpper` 而不是小写。
 *
 * **为什么算的是 base32 串而不是解码后的字节**：发送端手上就是这串字符，不用先解码再
 * 编码；接收端也能在解码负载之前先比 crc，坏批次根本走不到解码器。与协议 K 对「线上
 * 字符」算 CRC 是同一条理由。
 */
export function qPayloadCrc(wholePayload: string): string {
  const sum = crc32(enc.encode(wholePayload))
  return b32EncodeUpper([(sum >>> 24) & 0xFF, (sum >>> 16) & 0xFF, (sum >>> 8) & 0xFF, sum & 0xFF])
}

/**
 * 构造一帧：`<magic>.<total>.<index>.<crc>.<payload>`。
 *
 * `crc` 必须由调用方显式传入 {@link qPayloadCrc} 的结果：分帧循环里它对**整条消息**
 * 只算一次，然后每一片都填同一个值。不在这里自动算（也没法算 —— 这里只有片段），
 * 是为了让「同一批必须同 crc」这件事在调用点可见，而不是藏在函数里。
 *
 * 传非法参数（未知 magic、`total < 1`、`index` 越界、`crc` 不是 7 位大写 base32、
 * payload 里不是大写 base32）时**抛错**而不是返回空串：这是本地构造路径，参数错误是
 * 程序 bug，早点炸在源头比在「接收端说这不是帧」的时候再查便宜。
 */
export function buildQFrame(magic: string, total: number, index: number, crc: string, payload: string): string {
  if (!isKnownMagic(magic))
    throw new Error(`未知的二维码 magic：${magic}`)
  if (!Number.isSafeInteger(total) || total < 1)
    throw new Error(`total 必须是 >= 1 的整数：${total}`)
  if (!Number.isSafeInteger(index) || index < 0 || index >= total)
    throw new Error(`index 必须落在 [0, total)：${index}`)
  if (!CRC_RE.test(crc))
    throw new Error('crc 必须是 7 位大写 base32')
  if (!PAYLOAD_RE.test(payload))
    throw new Error('payload 必须是大写 base32 无填充')
  return `${magic}${Q_SEP}${total}${Q_SEP}${index}${Q_SEP}${crc}${Q_SEP}${payload}`
}

/**
 * 解析一帧。不是本协议的文本（画面里可能有别的二维码）一律返回 `null`。
 *
 * 拒绝：字段数不是 5、未知 magic、`total < 1`、`index` 越界、`crc` 不是 7 字符大写 base32、
 * payload 含 `[A-Z2-7]` 之外的字符（payload 允许为空：整条消息为空时只有一片空片段）。
 *
 * **刻意不校验 CRC**，也校验不了：`crc` 是整条消息的摘要，单看一片根本没有可比的基准 ——
 * 拿 `qPayloadCrc(本片 payload)` 去比一定不等（除非整条消息恰好只有这一片）。完整性由
 * {@link finishQMessage} 在重组之后负责。
 */
export function parseQFrame(raw: string): QFrame | null {
  const parts = raw.trim().split(Q_SEP)
  if (parts.length !== 5)
    return null
  const [magic, totalText, indexText, crc, payload] = parts
  if (!isKnownMagic(magic))
    return null
  if (!DEC_RE.test(totalText) || !DEC_RE.test(indexText))
    return null
  const total = Number(totalText)
  const index = Number(indexText)
  if (!Number.isSafeInteger(total) || !Number.isSafeInteger(index))
    return null
  if (total < 1 || index < 0 || index >= total)
    return null
  if (!CRC_RE.test(crc))
    return null
  if (!PAYLOAD_RE.test(payload))
    return null
  return { magic, total, index, crc, payload }
}

/**
 * 重组完成后的最后一步：**校验整条消息的 CRC**，对得上才返回负载，否则 `null`。
 *
 * 单帧解析（{@link parseQFrame}）没法自校验，重组器（{@link createQAssembler}）也只管
 * 「片齐了没有」；把校验固化成这个必须调用的函数，是为了让「忘了校验」这个错误不可能
 * 悄悄发生 —— 拼接出来的负载在解码/落盘之前一定会经过这里。
 */
export function finishQMessage(batch: { crc: string, payload: string }): string | null {
  return qPayloadCrc(batch.payload) === batch.crc ? batch.payload : null
}

/** `unknown` → 索引对象；数组与 `null` 都不算。 */
function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    return null
  return value as Record<string, unknown>
}

/**
 * 把反馈编成 `QBB` 二维码文本。
 *
 * JSON 的字段顺序按「最常用的在前」固定成 `type/status` + 各自专属字段。顺序不影响
 * 解析（解析只按键取值），固定它只是为了 golden vector 稳定、以及人肉看二维码内容时顺眼。
 * 未知字段会被原样带上：将来新增字段时旧接收端忽略、新接收端读到，不需要换 magic。
 */
export function buildFeedbackFrame(fb: Feedback): string {
  const msg: Record<string, string> = { type: fb.type, status: fb.status }
  if (fb.missing !== undefined)
    msg.missing = fb.missing
  if (fb.saved !== undefined)
    msg.saved = fb.saved
  if (fb.reason !== undefined)
    msg.reason = fb.reason
  // 反馈很短，恒定单帧：负载一次编好，crc 也就是对它的摘要
  const payload = b32EncodeUpper(enc.encode(JSON.stringify(msg)))
  return buildQFrame(Q_MAGIC_CONTROL, 1, 0, qPayloadCrc(payload), payload)
}

/**
 * 从**重组后的 QBB 负载**或**完整 QBB 帧文本**里解出反馈消息；不是反馈就 `null`。
 *
 * 两种入参都要支持，是因为调用点天然分两类：宿主脚本拿到的是画面上扫到的整帧文本
 * （`QBB.1.0.…`），而接收页在本地组装时手里只有负载字符串 —— 与其让两边各写一份
 * 容错解析，不如把这个判断收在协议层一次做对。
 *
 * 容错策略（都是 `null` 而不是抛异常：调用者面对的是屏幕上的任意二维码文本）：
 * - 先按整帧解析，成功且 magic 是 `QBB` 就用解码后的 JSON；
 * - 否则若整串看着像 JSON 就直接用；再否则按 base32（大小写都收）试解一次，
 *   兼容「已重组的负载」这种中间形态；
 * - JSON 解析失败、不是对象、`type !== 'feedback'`、`status` 不在已知集合 → `null`；
 * - **未知 JSON 键一律忽略**（向前兼容），已知键类型不对也只当作没带该键。
 */
export function parseFeedback(raw: string): Feedback | null {
  const text = raw.trim()
  if (!text)
    return null

  // 先按整帧解。magic 不是 QBB 时（比如 QBA）不能把负载当控制消息读
  const frame = parseQFrame(text)
  let json: string = text
  if (frame) {
    if (frame.magic !== Q_MAGIC_CONTROL)
      return null
    // 反馈恒定单帧（`buildFeedbackFrame`），所以这里直接把片段当整条负载解；
    // 万一是多帧消息的一片，解出来的 JSON 必然不完整，会在下面的 JSON.parse 处失败
    const decoded = decodeJsonPayload(frame.payload, PAYLOAD_RE)
    if (decoded === null)
      return null
    json = decoded
  }
  else if (!text.startsWith('{')) {
    // 既不是完整帧、也不像 JSON：按「已重组的 base32 负载」试解一次（大小写都收）
    const decoded = decodeJsonPayload(text, B32_ANY_CASE_RE)
    if (decoded === null)
      return null
    json = decoded
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  }
  catch {
    return null
  }
  const msg = asRecord(parsed)
  if (!msg || msg.type !== FEEDBACK_TYPE)
    return null
  if (typeof msg.status !== 'string' || !FEEDBACK_STATUSES.includes(msg.status as FeedbackStatus))
    return null

  const fb: Feedback = { type: FEEDBACK_TYPE, status: msg.status as FeedbackStatus }
  // 只在键存在且是字符串时带上：类型不对（比如 `{"missing":42}`）当没带，而不是整条拒掉
  if (typeof msg.missing === 'string')
    fb.missing = msg.missing
  if (typeof msg.saved === 'string')
    fb.saved = msg.saved
  if (typeof msg.reason === 'string')
    fb.reason = msg.reason
  return fb
}

/** 按给定字符集把负载解成 JSON 文本；字符集不符或不是合法 UTF-8 时 `null`。 */
function decodeJsonPayload(payload: string, re: RegExp): string | null {
  if (!re.test(payload))
    return null
  try {
    return dec.decode(b32Decode(payload))
  }
  catch {
    return null
  }
}

/**
 * `[[0,2],[7,7],[9,11]]` → `"0-2,7,9-11"`；空数组 → `""`。
 *
 * 单点区间（`a === b`）写成裸数字：`7` 比 `7-7` 少一个字符，而缺失片号常常大量是单点，
 * 二维码里省下来的长度是实打实的容量。
 *
 * 输入假定已升序且不重叠（{@link parseMissingRanges} 的产物天然满足）；
 * 不合法的区间（非整数、负数、`a > b`）被跳过而不是抛错 —— 发送端只是上报缺失情况，
 * 没必要为此中断整条链路。调用方若要严格校验，用 `parseMissingRanges` 的往返来验证。
 */
export function formatMissingRanges(ranges: Array<[number, number]>): string {
  const parts: string[] = []
  for (const [a, b] of ranges) {
    if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b) || a < 0 || b < a)
      continue
    parts.push(a === b ? String(a) : `${a}-${b}`)
  }
  return parts.join(',')
}

/**
 * {@link formatMissingRanges} 的逆。畸形输入返回 `null`（不抛异常）。
 *
 * 严格到什么程度：任何 `[0-9,-]` 之外的字符、空段（`1,,2`）、首尾逗号、`a > b`、
 * 降序或重叠区间、非安全整数，全部拒绝。空串是**合法**的：它表示「什么都不缺」，
 * 与「格式错误」必须区分开 —— 前者是正常汇报，后者是坏数据。
 */
export function parseMissingRanges(s: string): Array<[number, number]> | null {
  if (s === '')
    return []
  if (!/^[\d,-]+$/.test(s))
    return null

  const out: Array<[number, number]> = []
  for (const segment of s.split(',')) {
    if (!segment)
      return null
    const dash = segment.indexOf('-')
    let aText: string
    let bText: string
    if (dash < 0) {
      aText = segment
      bText = segment
    }
    else {
      aText = segment.slice(0, dash)
      bText = segment.slice(dash + 1)
      // 只认恰好一个 `-`：`1-2-3` 会切出含 `-` 的 bText，被下面的数字校验挡下
    }
    if (!/^\d+$/.test(aText) || !/^\d+$/.test(bText))
      return null
    const a = Number(aText)
    const b = Number(bText)
    if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b) || a < 0 || b < a)
      return null
    // 升序且不重叠：下一段必须严格大于上一段的右端
    const prev = out[out.length - 1]
    if (prev && a <= prev[1])
      return null
    out.push([a, b])
  }
  return out
}

/** 组装批次。key 之外的字段都是这批消息自己的状态。 */
interface QBatch {
  magic: string
  total: number
  crc: string
  /** 尚缺的片号 → 负载。收到一片就删一个键，`size === 0` 即齐。 */
  parts: Map<number, string>
}

/**
 * 多帧 Q 消息的重组器。
 *
 * 按 `(magic, total, crc)` 分组。这里能拿 `crc` 当批次号，靠的正是它由 {@link qPayloadCrc}
 * 对**整条消息**算出来 —— 同一条消息的每一片都带同一个值，而两条内容不同的消息几乎不可能
 * 撞出同一个 32 位摘要。于是一批收到一半时来了另一批（用户重发、或换了内容），会各自进
 * 各自的桶，不会互相污染。
 *
 * 齐了就把负载按 index 顺序拼起来返回（**含 `crc`**，交给 {@link finishQMessage} 做最后
 * 校验，别直接把拼接结果拿去解码）；重复帧是幂等的（同一个 index 覆盖同一个键，再喂一次
 * 结果不变）。`total = 1` 的单帧消息在一帧之后就完成。
 */
export function createQAssembler(): { ingest: (frame: QFrame) => { magic: string, crc: string, payload: string } | null, reset: () => void } {
  // 用数组而不是 Map：正常情况同时只有一两个桶，线性查找比哈希更便宜，也天然有「先后」顺序
  let batches: QBatch[] = []

  return {
    /**
     * 喂一帧。凑齐返回整条消息（含 crc），否则 `null`。
     *
     * 凑齐之后这批就被丢弃（不再保留）：重复帧与「完成后再来一片」都自然返回 `null`，
     * 调用方不会把同一条消息处理两次。
     */
    ingest: (frame: QFrame): { magic: string, crc: string, payload: string } | null => {
      // 越界片号不该进桶：宁可丢掉一帧脏数据，也不要让一条消息永远差点数
      if (!Number.isSafeInteger(frame.index) || frame.index < 0 || frame.index >= frame.total)
        return null

      let batch = batches.find(b => b.magic === frame.magic && b.total === frame.total && b.crc === frame.crc)
      if (!batch) {
        // 只留最近 4 批：扫码场景里旧批次几乎不可能再补齐，无限攒桶等于给脏二维码留内存泄漏
        if (batches.length >= 4)
          batches = batches.slice(-3)
        batch = { magic: frame.magic, total: frame.total, crc: frame.crc, parts: new Map() }
        batches.push(batch)
      }

      // 重复 index 直接覆盖：同批同片的负载必然相同（同一 crc 摘要），索性不去比
      batch.parts.set(frame.index, frame.payload)
      if (batch.parts.size < batch.total)
        return null

      const payload = Array.from({ length: batch.total }, (_, i) => batch.parts.get(i) ?? '').join('')
      batches = batches.filter(b => b !== batch)
      return { magic: batch.magic, crc: batch.crc, payload }
    },

    /** 清空所有未完成的批次（例如接收页换了目标文件，旧批次作废）。 */
    reset: (): void => {
      batches = []
    },
  }
}
