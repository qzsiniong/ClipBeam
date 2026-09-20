// 脚本编辑器：CodeMirror 6 的扩展组装。
//
// 一个地方把编辑器需要的东西拼齐：基础编辑体验、语言解析（高亮）、**TypeScript 驱动的**
// 补全 / 悬停 / 参数信息 / 类型诊断，以及「运行 / 保存」快捷键与变更回调。
//
// 分工：
// * [`language.ts`](./language.ts)      —— lezer 语法（只负责高亮与缩进）
// * [`language-service.ts`](./language-service.ts) —— TypeScript 语言服务（语义：类型）
// * [`completion.ts`](./completion.ts)  —— 补全（含语言服务未就绪时的兜底）
// * [`hover.ts`](./hover.ts)            —— 悬停类型 + JSDoc
// * [`signature.ts`](./signature.ts)    —— 参数信息面板
// * [`linter.ts`](./linter.ts)          —— 诊断标红/标黄
//
// 语义相关的一切都走**同一个**语言服务实例，所以「诊断说没问题、hover 说旧类型」这种
// 不一致不会发生（见 language-service.ts 里的版本号说明）。

import type { Extension } from '@codemirror/state'
import type { KeyBinding } from '@codemirror/view'
import type { Capability, NamespaceNames } from './autocomplete'
import type { ScriptGetter } from './completion'
import { autocompletion, closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
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
import { clipbeamCompletionSource } from './completion'
import { clipbeamHover } from './hover'
import { clipbeamLanguage } from './language'
import { clipbeamLinter } from './linter'
import { RUN_KEY, SAVE_KEY } from './shortcuts'
import { clipbeamSignatureHelp } from './signature'

/** 编辑器对外暴露的回调。 */
export interface EditorHooks {
  /** 文档变化。 */
  onChange: (value: string) => void
  /** [`RUN_KEY`]（`Mod-Enter`）。 */
  onRun: () => void
  /** [`SAVE_KEY`]（`Mod-s`）。 */
  onSave: () => void
}

/**
 * 本项目自己绑的快捷键（**最高优先级**，避免被默认键位吃掉）。
 *
 * 单独抽成函数有两个原因：`clipbeamExtensions` 要用它；单测也要能读到这份绑定，
 * 从而断言「帮助面板里写的键」与「编辑器真绑的键」是同一份（见 `shortcuts.ts`）。
 */
export function editorKeymap(hooks: EditorHooks): KeyBinding[] {
  return [
    {
      key: RUN_KEY,
      run: () => {
        hooks.onRun()
        return true
      },
    },
    {
      key: SAVE_KEY,
      run: () => {
        hooks.onSave()
        return true
      },
    },
  ]
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
export function completionExtension(
  getScript: ScriptGetter,
  getNamespace: () => NamespaceNames,
  getCapabilities: () => Capability[],
): Extension {
  return autocompletion({
    activateOnTyping: true,
    closeOnBlur: true,
    // 只注册**一个** source：它内部按语言服务是否就绪决定走 TS 还是能力清单兜底。
    // 之前用 override 直接顶掉了语言自带补全，导致变量完全没有补全。
    override: [clipbeamCompletionSource(getScript, getNamespace, getCapabilities)],
  })
}

/**
 * 组装完整的编辑器扩展。
 *
 * @param getScript 读取当前脚本（名字 + 内容），语义功能都靠它。
 * @param getNamespace 命名空间的全局名字（后端下发）。
 * @param getCapabilities 能力清单（仅用于语言服务未就绪时的兜底补全）。
 * @param hooks 变更 / 运行 / 保存回调。
 */
export function clipbeamExtensions(
  getScript: ScriptGetter,
  getNamespace: () => NamespaceNames,
  getCapabilities: () => Capability[],
  hooks: EditorHooks,
): Extension[] {
  return [
    // 基础编辑体验：行号、历史、括号匹配/自动闭合、搜索、当前行高亮…
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightActiveLine(),
    drawSelection(),
    history(),
    indentOnInput(),
    bracketMatching(),
    closeBrackets(),
    highlightSelectionMatches(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),

    // 语言（高亮/缩进）与补全
    languageCompartment.of(clipbeamLanguage(getScript().name)),
    completionCompartment.of(completionExtension(getScript, getNamespace, getCapabilities)),

    // 语义能力：悬停、参数信息、类型诊断由同一个 TypeScript 语言服务提供
    clipbeamHover(getScript),
    clipbeamSignatureHelp(getScript),
    clipbeamLinter(() => getScript().name),

    // 运行 / 保存放在最高优先级，避免被其它键位吃掉
    // （注意 `Mod-Enter` 在 CodeMirror 默认键位里是 insertBlankLine，这里被我们遮蔽）
    Prec.highest(keymap.of(editorKeymap(hooks))),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),

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
      '.cm-tooltip': { fontSize: '12px' },
    }),
  ]
}

/** 用新的补全数据替换编辑器里的补全扩展。 */
export function replaceCompletion(
  view: EditorView,
  getScript: ScriptGetter,
  getNamespace: () => NamespaceNames,
  getCapabilities: () => Capability[],
) {
  view.dispatch({
    effects: completionCompartment.reconfigure(
      completionExtension(getScript, getNamespace, getCapabilities),
    ),
  })
}

/** 创建初始状态（组件挂载时用一次）。 */
export function createEditorState(doc: string, extensions: Extension[]): EditorState {
  return EditorState.create({ doc, extensions })
}
