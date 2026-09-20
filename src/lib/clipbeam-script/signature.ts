// 脚本编辑器：参数信息（签名提示）。
//
// 输入 `$.zstd(` 或参数之间按 `,` 时，在编辑区上方显示当前函数的签名，并高亮当前参数：
//
// ```text
// zstd(data: BinaryInput, level?: number): ArrayBuffer       参数 1/2
//      ~~~~~~~~~~~~~
// ```
//
// 用 `StateField` + `showPanel`：签名放在编辑器状态里（可随事务更新），面板贴在编辑区
// 上方 —— 不用 tooltip 是因为 tooltip 会跟着光标跑，容易盖住正在写的代码。

import type { Extension } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import type { ScriptGetter } from './completion'
import type { SignatureInfo } from './language-service'
import { StateEffect, StateField } from '@codemirror/state'
import { showPanel, ViewPlugin } from '@codemirror/view'
import { signatureHelp } from './language-service'

/** 把异步查询到的签名写进编辑器状态。 */
const setSignature = StateEffect.define<SignatureInfo | null>()

/** 当前签名提示；`null` 表示不显示。 */
const signatureField = StateField.define<SignatureInfo | null>({
  create: () => null,
  update: (value, transaction) => {
    for (const effect of transaction.effects) {
      if (effect.is(setSignature)) {
        return effect.value
      }
    }
    return value
  },
})

/**
 * 光标是否位于某个函数调用的参数表里。
 *
 * 从光标往前扫：先遇到 `(` 就认为在调用里；未进括号时遇到 `;` / `{` / `}` 就认为不在。
 * 字符串或注释里的括号可能被误判 —— 代价只是多算一次查询，结果由语言服务兜底（会返回 null）。
 */
export function insideCall(source: string, offset: number): boolean {
  let depth = 0
  for (let index = offset - 1; index >= 0; index--) {
    const char = source[index]
    if (char === ')') {
      depth++
    }
    else if (char === '(') {
      if (depth === 0) {
        return true
      }
      depth--
    }
    else if (depth === 0 && (char === ';' || char === '{' || char === '}')) {
      return false
    }
  }
  return false
}

/** 渲染签名面板的 DOM：签名文本 + 当前参数高亮 + 「参数 i/n」。 */
function renderPanel(view: EditorView, info: SignatureInfo): HTMLElement {
  const doc = view.dom.ownerDocument
  const root = doc.createElement('div')
  root.className = 'cm-clipbeam-signature'
  root.style.display = 'flex'
  root.style.alignItems = 'baseline'
  root.style.gap = '8px'
  root.style.padding = '2px 8px'
  root.style.fontSize = '11px'
  root.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
  root.style.color = 'var(--muted-foreground)'

  // 在签名文本里**按顺序**定位每个参数，得到 [start, end) 区间后逐段渲染。
  // 不靠 `indexOf(parameters[0])` 猜前缀：参数名可能在前缀里也出现（如 `f(f: number)`）。
  const spans: { start: number, end: number }[] = []
  let cursor = 0
  for (const parameter of info.parameters) {
    const found = info.label.indexOf(parameter, cursor)
    if (found < 0) {
      // 语言服务给的参数文本与拼出来的签名不完全一致时放弃高亮，只显示整行
      spans.length = 0
      break
    }
    spans.push({ start: found, end: found + parameter.length })
    cursor = found + parameter.length
  }

  const label = doc.createElement('span')
  label.style.whiteSpace = 'pre-wrap'

  if (spans.length === 0) {
    label.textContent = info.label
  }
  else {
    let position = 0
    for (const [index, span] of spans.entries()) {
      if (span.start > position) {
        label.appendChild(doc.createTextNode(info.label.slice(position, span.start)))
      }
      const active = index === info.activeParameter
      if (active) {
        const em = doc.createElement('span')
        em.textContent = info.label.slice(span.start, span.end)
        em.style.color = 'var(--foreground)'
        em.style.textDecoration = 'underline'
        label.appendChild(em)
      }
      else {
        label.appendChild(doc.createTextNode(info.label.slice(span.start, span.end)))
      }
      position = span.end
    }
    if (position < info.label.length) {
      label.appendChild(doc.createTextNode(info.label.slice(position)))
    }
  }
  root.appendChild(label)

  const counter = doc.createElement('span')
  const total = info.parameters.length
  const current = Math.min(Math.max(info.activeParameter + 1, 1), Math.max(total, 1))
  counter.textContent = total > 0 ? `参数 ${current}/${total}` : ''
  if (info.signatureCount > 1) {
    counter.textContent += ` · 重载 ${info.activeSignature + 1}/${info.signatureCount}`
  }
  counter.style.marginLeft = 'auto'
  root.appendChild(counter)

  return root
}

/**
 * 建一个签名提示扩展。
 *
 * @param getScript 读取当前脚本（名字 + 内容）。
 */
export function clipbeamSignatureHelp(getScript: ScriptGetter): Extension {
  /** 请求序号：异步结果回来时若已过期就丢弃，避免面板显示上一个位置的签名。 */
  let latestRequest = 0

  const plugin = ViewPlugin.fromClass(class {
    private readonly view: EditorView

    constructor(view: EditorView) {
      this.view = view
      this.refresh()
    }

    update(update: { docChanged: boolean, selectionSet: boolean, view: EditorView }) {
      if (update.docChanged || update.selectionSet) {
        this.refresh()
      }
    }

    /** 查一次签名并按结果更新状态。 */
    refresh() {
      const view = this.view
      const script = getScript()
      const offset = view.state.selection.main.head
      const source = view.state.doc.toString()

      // 不在调用里就直接清空，省掉一次查询
      if (!insideCall(source, offset)) {
        latestRequest++
        if (view.state.field(signatureField, false)) {
          view.dispatch({ effects: setSignature.of(null) })
        }
        return
      }

      const request = ++latestRequest
      void signatureHelp(script.name, source, offset).then((info) => {
        // 过期结果或视图已销毁：丢弃
        if (request !== latestRequest || !view.dom.isConnected) {
          return
        }
        view.dispatch({ effects: setSignature.of(info) })
      })
    }
  })

  // `showPanel` 的构造器类型不允许返回 null（虽然文档说会忽略），所以这里**总是**返回面板，
  // 没有签名时用 `display: none` 收起 —— 否则它会占掉一条空白横条。
  const panel = showPanel.of((view) => {
    const info = view.state.field(signatureField, false)
    const dom = info
      ? renderPanel(view, info)
      : view.dom.ownerDocument.createElement('div')
    dom.style.display = info ? '' : 'none'
    return { top: true, dom }
  })

  return [signatureField, plugin, panel]
}
