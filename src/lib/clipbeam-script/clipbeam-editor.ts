// 脚本编辑器：CodeMirror 6 的扩展组装。
//
// 一个地方把编辑器需要的东西拼齐：基础编辑体验（`basicSetup`）、语言解析、补全、
// 类型诊断 lint、主题、以及「运行 / 保存」快捷键与变更回调。
//
// 分文件的原因：这里只管「装配」，[`autocomplete.ts`](./autocomplete.ts)、
// [`typecheck.ts`](./typecheck.ts)、[`linter.ts`](./linter.ts) 各自负责一件事。

import type { Extension } from '@codemirror/state'
import type { Capability } from './autocomplete'
import { autocompletion } from '@codemirror/autocomplete'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { bracketMatching, defaultHighlightStyle, indentOnInput, syntaxHighlighting } from '@codemirror/language'
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search'
import { Compartment, EditorState, Prec } from '@codemirror/state'

import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from '@codemirror/view'
import { clipbeamCompletion } from './autocomplete'
import { clipbeamLanguage } from './language'
import { clipbeamLinter } from './linter'

/** 编辑器对外暴露的回调。 */
export interface EditorHooks {
  /** 文档变化。 */
  onChange: (value: string) => void
  /** Cmd/Ctrl+Enter。 */
  onRun: () => void
  /** Cmd/Ctrl+S。 */
  onSave: () => void
}

/** 语言切换用的 compartment（改语言不需要重建整个编辑器）。 */
export const languageCompartment = new Compartment()

/** 补全数据切换用的 compartment（能力清单是异步拿到的）。 */
export const completionCompartment = new Compartment()

/**
 * 当前能力清单下的补全扩展。
 *
 * 单独抽出来是为了「拿到能力清单后热替换」：能力列表来自后端，编辑器可能先挂载完成。
 */
export function completionExtension(capabilities: Capability[]): Extension {
  return autocompletion({ activateOnTyping: true, closeOnBlur: true, override: [clipbeamCompletion(capabilities)] })
}

/**
 * 组装完整的编辑器扩展。
 *
 * @param language `'js'` 或 `'ts'`，决定语言解析与类型检查的严格程度。
 * @param getScriptName 脚本名 getter（lint 用它判断 `.ts` / `.js`）。
 * @param capabilities 后端返回的能力清单（决定 `$.` 补全项）。
 * @param hooks 变更 / 运行 / 保存回调。
 */
export function clipbeamExtensions(
  language: 'js' | 'ts',
  getScriptName: () => string,
  capabilities: Capability[],
  hooks: EditorHooks,
): Extension[] {
  return [
    // 基础编辑体验：行号、历史、括号匹配、搜索、当前行高亮…
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightActiveLine(),
    drawSelection(),
    history(),
    indentOnInput(),
    bracketMatching(),
    highlightSelectionMatches(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),

    // 语言与补全（两者都能用 compartment 热替换）
    languageCompartment.of(clipbeamLanguage(language)),
    completionCompartment.of(completionExtension(capabilities)),

    // 类型诊断：TypeScript 编译器 API 算出来的真错误/警告
    clipbeamLinter(getScriptName),

    // 运行 / 保存放在最高优先级，避免被其它键位吃掉
    Prec.highest(
      keymap.of([
        {
          key: 'Mod-Enter',
          run: () => {
            hooks.onRun()
            return true
          },
        },
        {
          key: 'Mod-s',
          run: () => {
            hooks.onSave()
            return true
          },
        },
      ]),
    ),
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),

    EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        hooks.onChange(update.state.doc.toString())
      }
    }),

    // 高度由外层容器控制；字体与配色跟 tailwind 变量走，明暗主题切换后自动生效
    EditorView.theme({
      '&': { height: '100%', fontSize: '13px' },
      '&.cm-focused': { outline: 'none' },
      '.cm-scroller': {
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        overflow: 'auto',
      },
      '.cm-gutters': { backgroundColor: 'transparent', border: 'none' },
      '.cm-content': { paddingBottom: '2rem' },
    }),
  ]
}

/** 用新的补全数据替换编辑器里的补全扩展。 */
export function replaceCompletion(view: EditorView, capabilities: Capability[]) {
  view.dispatch({
    effects: completionCompartment.reconfigure(completionExtension(capabilities)),
  })
}

/** 创建初始状态（组件挂载时用一次）。 */
export function createEditorState(doc: string, extensions: Extension[]): EditorState {
  return EditorState.create({ doc, extensions })
}
