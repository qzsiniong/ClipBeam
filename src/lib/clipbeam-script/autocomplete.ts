// 脚本编辑器：能力命名空间的补全数据源。
//
// 数据来源是后端的 `list_capabilities` 命令 —— 它与运行期注册的能力同源，
// 因此引擎少一个能力时，补全不会还在提示一个不存在的方法。
//
// 命名空间的**名字**（`ClipBeam` / `$` / 以后换成别的）同样由后端下发（`CapabilityList`），
// 前端不写死：改名只需要改 `clipbeam_scripting::NAMESPACE`。
//
// 这里再把引擎固定提供的标准全局补上（`TextDecoder`、`sleep`…），它们不来自能力清单。

import type { Completion, CompletionContext, CompletionResult, CompletionSource } from '@codemirror/autocomplete'

/** 一条能力的签名声明（对应后端 `clipbeam_scripting::Capability`）。 */
export interface Capability {
  /** JS 侧的方法名，例如 `md5`。 */
  name: string
  /** 人类可读签名，例如 `md5(data: ArrayBuffer) -> string`。 */
  signature: string
  /** 一句话说明。 */
  doc: string
  /** 来源标记；目前只有 `clipbeam`（引擎不提供能力）。 */
  source: string
}

/**
 * 能力命名空间的全局名字（后端下发）。
 *
 * `namespace` 一定非空；`alias` 为空串表示没有别名。
 */
export interface NamespaceNames {
  namespace: string
  alias: string
}

/** `list_capabilities` 命令的返回体。 */
export interface CapabilityList extends NamespaceNames {
  capabilities: Capability[]
}

/** 命名空间对象的补全项（名字来自后端）。 */
function namespaceCompletions(names: NamespaceNames): Completion[] {
  const entries: Completion[] = [
    { label: names.namespace, type: 'class', detail: '能力命名空间', info: '脚本可用的全部能力都挂在这个对象上' },
  ]
  if (names.alias) {
    entries.push({ label: names.alias, type: 'variable', detail: `${names.namespace} 的别名`, info: '指向同一个对象，写起来更短' })
  }
  return entries
}

/** 引擎固定提供的标准全局（不来自能力清单）。 */
const ENGINE_GLOBALS: Completion[] = [
  { label: 'sleep', type: 'function', detail: '异步等待', info: 'await sleep(ms)：等待期间被中止会立即返回' },
  {
    label: 'TextDecoder',
    type: 'class',
    detail: '解码字节流',
    info: '支持全部 WHATWG 编码标签：utf-8 / gbk / gb18030 / big5 / shift_jis …',
  },
  { label: 'TextEncoder', type: 'class', detail: 'UTF-8 编码', info: 'encode() 返回 Uint8Array' },
  { label: 'console', type: 'variable', detail: '日志输出', info: 'log / info / debug / warn / error' },
]

/** 把名字转义成能安全放进正则的字面量（`$` 这类字符必须转义）。 */
function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * 建一个补全源。
 *
 * @param names 命名空间的全局名字（后端下发；空名字的列表也能用，只是没有成员补全）。
 * @param capabilities 能力清单（后端下发）。
 */
export function clipbeamCompletion(
  names: NamespaceNames,
  capabilities: Capability[],
): CompletionSource {
  const methodCompletions: Completion[] = capabilities.map(capability => ({
    label: capability.name,
    type: 'method',
    detail: capability.signature,
    info: capability.doc,
    // 让 `md5` 也能被 `md` 前缀命中
    boost: capability.source === 'clipbeam' ? 1 : 0,
  }))

  const globals = [...namespaceCompletions(names), ...ENGINE_GLOBALS]

  // 成员补全：`$.` / `ClipBeam.`（名字来自后端，不写死）
  const memberNames = [names.namespace, names.alias]
    .filter(Boolean)
    .map(escapeRegExp)
  const memberPattern = memberNames.length > 0
    ? new RegExp(`(?:${memberNames.join('|')})\\.[\\w$]*`)
    : null

  return (context: CompletionContext): CompletionResult | null => {
    if (memberPattern) {
      const member = context.matchBefore(memberPattern)
      if (member) {
        return {
          from: member.from + member.text.indexOf('.') + 1,
          options: methodCompletions,
          validFor: /^[\w$]*$/,
        }
      }
    }

    // 全局对象补全：命名空间（含别名）与引擎标准全局
    const word = context.matchBefore(/\w+/)
    if (!word) {
      return null
    }
    if (word.from === word.to && !context.explicit) {
      return null
    }
    return { from: word.from, options: globals, validFor: /^\w*$/ }
  }
}
