// 脚本编辑器：TypeScript 驱动的代码补全。
//
// # 为什么不再手写补全
//
// 之前用 `autocompletion({ override: [clipbeamCompletion(...)] })`：`override` 会**完全替换**
// 语言自带的补全源，于是只剩「`$`/`ClipBeam` 的成员名 + 标准全局」，变量没有任何
// 补全与类型。现在改成把问题交给 TypeScript 语言服务（同一个服务也在算诊断/hover/签名）：
//
// | 场景 | 语言服务给的结果 |
// |---|---|
// | `ClipBeam.` | 全部能力（含签名与 JSDoc，来自 `clipbeam.d.ts`） |
// | `$.` | 同上（`$` 在声明里是 `ClipBeam` 的别名） |
// | `bytes.` | `ArrayBuffer` 的成员（`byteLength` / `slice` …） |
// | 变量名/关键字/局部符号 | 按当前作用域给出 |
//
// 语言服务还没加载完（首次打开脚本页的那一小会儿）时才退回「能力清单」补全，
// 见 [`./autocomplete.ts`](./autocomplete.ts)。

import type { Completion, CompletionContext, CompletionInfo, CompletionResult, CompletionSource } from '@codemirror/autocomplete'
import type { Capability, NamespaceNames } from './autocomplete'
import type { ScriptTarget, TsCompletionEntry } from './language-service'
import { clipbeamCompletion } from './autocomplete'
import { completions, isServiceReady } from './language-service'

/** 脚本内容从哪来（由 `ScriptEditor.vue` 注入，读的是当前 vue ref）。 */
export type ScriptGetter = () => ScriptTarget

/**
 * TypeScript 的补全类别 → CodeMirror 的类型。
 *
 * CodeMirror 用这个决定图标与排序分组；未列出的按 `property` 处理（最接近"成员"）。
 */
function completionType(entry: TsCompletionEntry): string {
  switch (entry.kind) {
    case 'method':
    case 'function':
      return 'method'
    case 'property':
      return 'property'
    case 'variable':
    case 'const':
    case 'let':
      return 'variable'
    case 'class':
    case 'interface':
    case 'type':
      return 'type'
    case 'enum':
    case 'enumMember':
      return 'enum'
    case 'keyword':
      return 'keyword'
    case 'string':
    case 'number':
    case 'boolean':
      return 'constant'
    default:
      return 'property'
  }
}

/** 从补全位置往前看，判断是不是 `.` 之后的成员补全（TypeScript 需要 triggerCharacter）。 */
function triggerCharacterAt(source: string, offset: number): string | undefined {
  for (let index = offset - 1; index >= 0; index--) {
    const char = source[index]
    if (char === '.' || char === '?' || char === '$') {
      return char === '?' ? undefined : char
    }
    // 标识符字符继续往前找；遇到其它字符说明不是成员访问
    if (!/[$\w]/.test(char)) {
      return undefined
    }
  }
  return undefined
}

/**
 * 拼出补全项的详情面板。
 *
 * 用 DOM 而不是 `innerHTML`：签名与 JSDoc 都来自声明文件，属于文本数据，
 * 不应该被当作 HTML 解析。
 */
function renderCompletionInfo(entry: TsCompletionEntry, signature: string, documentation: string): CompletionInfo {
  const root = document.createElement('div')
  root.style.maxWidth = '520px'
  root.style.fontSize = '12px'

  const code = document.createElement('pre')
  code.textContent = signature || entry.name
  code.style.margin = '0'
  code.style.whiteSpace = 'pre-wrap'
  code.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
  root.appendChild(code)

  if (documentation) {
    const doc = document.createElement('div')
    doc.textContent = documentation
    doc.style.marginTop = '6px'
    doc.style.whiteSpace = 'pre-wrap'
    doc.style.color = 'var(--muted-foreground)'
    root.appendChild(doc)
  }

  return root
}

/** 把一条 TypeScript 补全项转成 CodeMirror 的 `Completion`。 */
function toCompletion(entry: TsCompletionEntry): Completion {
  return {
    label: entry.name,
    type: completionType(entry),
    // 列表里先显示短信息（来源），完整签名与 JSDoc 在 `info` 里惰性取
    detail: entry.source ? `来自 ${entry.source}` : undefined,
    info: () => {
      const details = entry.getDetails()
      return renderCompletionInfo(entry, details.signature, details.documentation)
    },
    // 让 TypeScript 的排序（sortText）生效
    boost: 0,
  }
}

/**
 * 建一个补全源。
 *
 * @param getScript 读取当前脚本（名字 + 内容）。
 * @param getNamespace 命名空间的全局名字（后端下发）。
 * @param getCapabilities 能力清单（仅用于语言服务未就绪时的兜底）。
 */
export function clipbeamCompletionSource(
  getScript: ScriptGetter,
  getNamespace: () => NamespaceNames,
  getCapabilities: () => Capability[],
): CompletionSource {
  /**
   * 兜底补全（语言服务未就绪 / 没给结果时）。
   *
   * 包成 `async` 是刻意的：`CompletionSource` 允许返回 Promise，而直接返回同步结果时
   * TypeScript 会在联合类型上挑剔（`CompletionResult | Promise<...> | null` 不能赋给
   * `CompletionResult | null`）。
   */
  const fallback = async (context: CompletionContext): Promise<CompletionResult | null> =>
    await clipbeamCompletion(getNamespace(), getCapabilities())(context)

  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    // 语言服务还没就绪：先用能力清单顶着，别让用户什么都看不到
    if (!isServiceReady()) {
      return fallback(context)
    }

    const script = getScript()
    const offset = context.pos
    const triggerCharacter = triggerCharacterAt(script.source, offset)

    let entries: TsCompletionEntry[] | null = null
    try {
      entries = await completions(script.name, script.source, offset, triggerCharacter)
    }
    catch {
      // 语言服务内部异常不该让补全面板卡住
      entries = null
    }

    if (!entries || entries.length === 0) {
      // 语言服务没给出任何东西时再退兜底（例如刚打开文件、TS 还在首次解析）
      return fallback(context)
    }

    // 用 TypeScript 给的词边界：从光标往前匹配标识符
    const word = context.matchBefore(/[$\w#]*/)
    const from = word ? word.from : offset

    return {
      from,
      options: entries.slice(0, 200).map(toCompletion),
      // 继续输入时用同一批候选过滤（TypeScript 已按前缀给过结果）
      validFor: /^[$\w#]*$/,
    }
  }
}
