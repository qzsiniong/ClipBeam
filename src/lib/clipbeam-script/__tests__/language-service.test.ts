// 编辑器语义功能的单元测试（TypeScript 语言服务）。
//
// 被测的是「语言服务对虚拟文件到底给不给类型」这件事 —— 这正是 hover 为空、
// 补全很弱那两个 bug 的根因。测试直接问语言服务，不经过 CodeMirror，
// 因此不需要 DOM：
//
//   1. hover `ClipBeam` / `$.type_str` / 变量 → 有类型与 JSDoc；
//   2. 补全 `ClipBeam.` / `bytes.` → 有成员；
//   3. 签名提示 `$.zstd(` → 有参数；
//   4. 诊断仍能报出类型错误（不能因为重构把 lint 弄丢）。
//
// 断言用「包含子串」而不是全文比对：TypeScript 的措辞会随版本微调，
// 我们关心的是「有没有真的拿到类型」。

import type { SignatureInfo } from '../language-service'
import { beforeAll, describe, expect, it } from 'vitest'
import { tidyDocumentation } from '../hover'
import {
  completions,
  quickInfo,
  resetScript,
  signatureHelp,
  typecheck,
  warmup,
} from '../language-service'

/**
 * 测试脚本：覆盖 hover 与补全要验证的几种位置。
 *
 * 位置常量都按字符下标算好（见各用例），避免用 indexOf 在字符串里反复猜。
 */
const SCRIPT = `const bytes = await $.read('/tmp/clipbeam-editor-demo.bin')
const digest = $.md5(bytes)
ClipBeam.type_str('hello', 10)
await $.zstd(bytes, 1024)
`

/** 脚本名（`.ts` 会走 TypeScript 模式，`.js` 走 checkJs）。 */
const NAME = 'editor-check.js'

/** 取某个子串在脚本里的下标（用于 hover 定位）。 */
function offsetOf(needle: string, from = 0): number {
  const index = SCRIPT.indexOf(needle, from)
  expect(index, `脚本里应当包含 ${needle}`).toBeGreaterThanOrEqual(0)
  return index
}

/** 取某个子串之后的下标（用于「在 `.` 后面请求成员补全」）。 */
function offsetAfter(needle: string): number {
  return offsetOf(needle) + needle.length
}

beforeAll(async () => {
  await warmup()
  resetScript()
}, 30_000)

describe('hover（悬停提示）', () => {
  it('悬停 ClipBeam 给出它的类型', async () => {
    const info = await quickInfo(NAME, SCRIPT, offsetOf('ClipBeam.type_str') + 2)
    expect(info, 'ClipBeam 上应当有 hover 信息').not.toBeNull()
    expect(info?.signature).toContain('ClipBeam')
  })

  it('悬停 $.type_str 给出签名与 JSDoc', async () => {
    // 落在 `type_str` 中间
    const info = await quickInfo(NAME, SCRIPT, offsetOf('type_str') + 3)
    expect(info, '$.type_str 上应当有 hover 信息').not.toBeNull()
    expect(info?.signature).toContain('type_str')
    // 声明文件里的 @param/JSDoc 应当被带出来
    expect(info?.signature).toContain('text: string')
    expect(info?.documentation.length, 'type_str 应当带 JSDoc 说明').toBeGreaterThan(0)
  })

  it('悬停变量给出推断出的类型', async () => {
    const info = await quickInfo(NAME, SCRIPT, offsetOf('bytes'))
    expect(info, 'bytes 上应当有 hover 信息').not.toBeNull()
    expect(info?.signature).toContain('ArrayBuffer')
  })

  it('悬停普通表达式也能给出类型（IDE 手感）', async () => {
    const info = await quickInfo(NAME, SCRIPT, offsetOf('digest'))
    expect(info?.signature).toContain('string')
  })
})

describe('completions（代码补全）', () => {
  it('clipbeam. 之后列出能力', async () => {
    const entries = await completions(NAME, SCRIPT, offsetAfter('ClipBeam.'))
    expect(entries, 'ClipBeam. 应当有补全').not.toBeNull()
    const names = (entries ?? []).map(entry => entry.name)
    for (const expected of ['read', 'write_text', 'md5', 'base32', 'crc32', 'zstd', 'type_str', 'confirm']) {
      expect(names, `成员补全应当包含 ${expected}`).toContain(expected)
    }
    // sleep 是引擎提供的标准全局，不在命名空间上
    expect(names, 'sleep 不该出现在成员补全里').not.toContain('sleep')
  })

  it('$. 之后列出同样的能力（$ 是 ClipBeam 的别名）', async () => {
    const entries = await completions(NAME, SCRIPT, offsetAfter('$.'))
    const names = (entries ?? []).map(entry => entry.name)
    expect(names).toContain('md5')
    expect(names).toContain('type_str')
  })

  it('标识符位置给出作用域内的变量（不是只有能力名）', async () => {
    // 光标落在 `digest` 中间，TypeScript 应当把同作用域的 `bytes`、`digest` 一并列出
    const entries = await completions('vars.ts', 'const bytes = 1\nconst digest = 2\ndig\n', 'const bytes = 1\nconst digest = 2\ndig'.length)
    const names = (entries ?? []).map(entry => entry.name)
    expect(names, '应当能补出同作用域的变量').toContain('digest')
    expect(names, '也应当能补出其它变量').toContain('bytes')
  })

  it('arraybuffer 变量之后列出它的成员（类型推断生效）', async () => {
    const source = `const bytes = await $.read('/tmp/clipbeam-editor-demo.bin')\nbytes.\n`
    const entries = await completions('member.ts', source, source.indexOf('bytes.') + 'bytes.'.length)
    const names = (entries ?? []).map(entry => entry.name)
    // `ArrayBuffer` 不一定有 byteLength（那是视图的属性），但一定继承自 ArrayBufferLike/ArrayBuffer 接口
    expect(names.length, '成员补全不应当为空').toBeGreaterThan(0)
    expect(names.some(name => ['byteLength', 'slice', 'constructor'].includes(name)), `应当含 ArrayBuffer 的成员：${names.slice(0, 12).join(',')}`).toBe(true)
  })

  it('能力补全带签名详情（来自声明文件）', async () => {
    const entries = await completions(NAME, SCRIPT, offsetAfter('ClipBeam.'))
    const typeStr = (entries ?? []).find(entry => entry.name === 'type_str')
    expect(typeStr, '应当能补全出 type_str').toBeDefined()
    const details = typeStr?.getDetails()
    expect(details?.signature).toContain('type_str')
    expect(details?.documentation.length ?? 0).toBeGreaterThan(0)
  })
})

describe('signatureHelp（参数信息）', () => {
  it('在 zstd( 里给出签名与参数', async () => {
    // 落在 `1024` 前面，也就是第一个/第二个参数的位置
    const offset = offsetOf('$.zstd(bytes')
    const info: SignatureInfo | null = await signatureHelp(NAME, SCRIPT, offset + 8)
    expect(info, '$.zstd( 里应当有签名提示').not.toBeNull()
    expect(info?.label).toContain('zstd')
    expect(info?.label).toContain('level')
    expect(info?.parameters.length).toBeGreaterThanOrEqual(2)
  })
})

describe('typecheck（诊断，防回归）', () => {
  it('类型不匹配仍然报错', async () => {
    // 字符串对 md5 是合法入参（按 UTF-8 编码），但 `read` 只接受字符串路径
    const bad = `await $.read(123)\n`
    const diagnostics = await typecheck({ name: 'bad.js', source: bad })
    expect(diagnostics.length, '传错类型应当有诊断').toBeGreaterThan(0)
    expect(diagnostics.some(item => item.severity === 'error')).toBe(true)
  })

  it('正确脚本没有 error 级诊断', async () => {
    const diagnostics = await typecheck({ name: NAME, source: SCRIPT })
    const errors = diagnostics.filter(item => item.severity === 'error')
    expect(errors, `不该有错误诊断：${JSON.stringify(errors)}`).toHaveLength(0)
  })
})

describe('hover 展示（tooltip 尺寸与文案）', () => {
  it('jsdoc 里的 markdown 噪声被清掉', () => {
    const raw = '说明一。\n\n```ts\ninterface X { a: number }\n```\n\n**强调**与`反引号`。'
    const tidy = tidyDocumentation(raw)
    expect(tidy).not.toContain('```')
    expect(tidy).not.toContain('`')
    expect(tidy).not.toContain('**')
    expect(tidy).toContain('说明一。')
    // 代码块里的内容本身要保留（是有用的示例）
    expect(tidy).toContain('interface X')
  })

  it('超长 JSDoc 会被截断，避免 tooltip 盖住代码', () => {
    const tidy = tidyDocumentation('很长'.repeat(400))
    expect(tidy.length).toBeLessThanOrEqual(300)
    expect(tidy.endsWith('…（完整说明见声明文件与 README）')).toBe(true)
  })

  it('clipbeam 的悬停说明是「一小段」而不是整篇声明', async () => {
    const source = 'const $ = ClipBeam\n'
    const info = await quickInfo('hover.js', source, source.indexOf('ClipBeam') + 3)
    const tidy = tidyDocumentation(info?.documentation ?? '')
    expect(tidy.length).toBeGreaterThan(0)
    expect(tidy.length, `说明过长会盖住代码：${tidy}`).toBeLessThan(400)
  })
})
