// 脚本编辑器：`$` / `Clipbeam` 的补全数据源。
//
// 数据来源是后端能力清单（`list_capabilities` 命令）—— 它与运行期注册的能力同源，
// 因此引擎或少一个能力时，补全不会还在提示一个不存在的方法。
//
// 这里再把「全局对象本身」的补全补上（`$`、`Clipbeam`、`TextDecoder`…），
// 这些不来自能力清单，而是引擎固定提供的全局。

import type { Completion, CompletionContext, CompletionResult, CompletionSource } from '@codemirror/autocomplete'

/** 一条能力的签名声明（对应后端 `clipbeam_scripting::Capability`）。 */
export interface Capability {
  /** JS 侧的方法名，例如 `md5`。 */
  name: string
  /** 人类可读签名，例如 `md5(data: ArrayBuffer) -> string`。 */
  signature: string
  /** 一句话说明。 */
  doc: string
  /** `core`（引擎自带）或 `clipbeam`（ClipBeam 扩展）。 */
  source: string
}

/** 引擎固定提供的全局对象（不来自能力清单）。 */
const GLOBALS: Completion[] = [
  { label: 'Clipbeam', type: 'class', detail: '能力命名空间', info: '脚本可用的全部能力都挂在这个对象上' },
  { label: '$', type: 'variable', detail: 'Clipbeam 的别名', info: '指向同一个对象，写起来更短' },
  {
    label: 'TextDecoder',
    type: 'class',
    detail: '解码字节流',
    info: '支持全部 WHATWG 编码标签：utf-8 / gbk / gb18030 / big5 / shift_jis …',
  },
  { label: 'TextEncoder', type: 'class', detail: 'UTF-8 编码', info: 'encode() 返回 Uint8Array' },
  { label: 'console', type: 'variable', detail: '日志输出', info: 'log / info / debug / warn / error' },
]

/**
 * 建一个补全源。
 *
 * `capabilities` 为空时仍然提供全局对象补全（例如后端还没返回清单）。
 */
export function clipbeamCompletion(capabilities: Capability[]): CompletionSource {
  const methodCompletions: Completion[] = capabilities.map(capability => ({
    label: capability.name,
    type: 'method',
    detail: capability.signature,
    info: capability.doc,
    // 让 `md5` 也能被 `md` 前缀命中
    boost: capability.source === 'clipbeam' ? 1 : 0,
  }))

  return (context: CompletionContext): CompletionResult | null => {
    // 成员补全：`$.` / `Clipbeam.`
    const member = context.matchBefore(/\$\.[\w$]*|Clipbeam\.[\w$]*/)
    if (member) {
      return {
        from: member.from + member.text.indexOf('.') + 1,
        options: methodCompletions,
        validFor: /^[\w$]*$/,
      }
    }

    // 全局对象补全：`$` / `Clipbeam` / 标准 API
    const word = context.matchBefore(/\w+/)
    if (!word) {
      return null
    }
    if (word.from === word.to && !context.explicit) {
      return null
    }
    return { from: word.from, options: GLOBALS, validFor: /^\w*$/ }
  }
}
