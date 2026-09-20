// 脚本编辑器：悬停提示（hover）。
//
// # 之前为什么没有
//
// 项目里从来没有注册过 hover 扩展，所以悬停 `ClipBeam`、`$.type_str`、任何变量都不会有反应。
//
// # 现在怎么做
//
// 用 TypeScript 语言服务的 `getQuickInfoAtPosition`（同一个服务也在算诊断、补全、签名）：
// 悬停到任何标识符/表达式上都会得到「签名 + JSDoc」，其中能力的文档来自两份 `.d.ts`。
// 例如悬停 `$.type_str` 会显示：
//
// ```text
// (property) type_str: (text: string, delayMs?: number) => void
// 把文本交给宿主输出（GUI 下逐键打进当前焦点窗口）；被中止时抛异常
// ```
//
// 内容用 DOM 拼（**不**用 `innerHTML`）：声明文件里的文本属于用户数据，不该被当成 HTML。

import type { Extension } from '@codemirror/state'
import type { EditorView, Tooltip } from '@codemirror/view'
import type { ScriptGetter } from './completion'
import type { ScriptTarget } from './language-service'
import {

  hoverTooltip,

} from '@codemirror/view'
import { quickInfo } from './language-service'

/** 签名行最长显示这么多字符，避免复杂类型撑出一个巨框。 */
const SIGNATURE_LIMIT = 400

/**
 * 说明文字最多显示这么多字符。
 *
 * TypeScript 会把同一符号的**所有** JSDoc 合并后交给我们 —— 例如悬停全局的 `ClipBeam`
 * 时会同时带上 `interface ClipBeam` 与 `declare const ClipBeam` 两段注释，原样显示就是一个
 * 盖住代码的大方块。所以这里既清理 Markdown 噪声，也截断长度。
 */
const DOC_LIMIT = 260

/** tooltip 的最大宽度：够放一行长签名，又不会横跨整个编辑器。 */
const TOOLTIP_MAX_WIDTH = 460

/** 悬停后等一小会儿再查询，避免鼠标划过时连续触发。 */
const HOVER_DELAY = 120

/**
 * 把 JSDoc 整理成适合塞进 tooltip 的一段话。
 *
 * 声明文件里的注释是 Markdown（代码围栏、反引号、`**强调**`）。tooltip 是按纯文本渲染的，
 * 那些标记只会变成符号噪声，所以在这里去掉（代码围栏里的内容本身保留）。
 * 最后按 [`DOC_LIMIT`] 截断，超出时补一句提示。
 */
export function tidyDocumentation(text: string): string {
  const cleaned = text
    // 去掉代码围栏本身，保留里面的示例
    .replace(/```[a-z]*\n?/gi, '')
    // 去掉行内反引号与强调标记
    .replace(/`/g, '')
    .replace(/\*\*/g, '')
    // 折叠多余空行
    .replace(/\n{3,}/g, '\n\n')
    .trim()

  if (cleaned.length <= DOC_LIMIT) {
    return cleaned
  }
  return `${cleaned.slice(0, DOC_LIMIT)}…（完整说明见声明文件与 README）`
}

/**
 * 组装 tooltip 的 DOM。
 *
 * 分成三块：签名（等宽字体）、说明（次要色）、`@param` 等标签（更弱的颜色）。
 */
function renderHover(view: EditorView, info: { signature: string, documentation: string, tags: string[] }): HTMLElement {
  const root = view.dom.ownerDocument.createElement('div')
  root.className = 'clipbeam-hover cm-tooltip-section'
  root.style.maxWidth = `${TOOLTIP_MAX_WIDTH}px`
  root.style.padding = '6px 8px'
  root.style.fontSize = '12px'
  root.style.lineHeight = '1.6'
  // 文档极长时也别把代码整段盖住：给个高度上限，超出自己滚
  root.style.maxHeight = '240px'
  root.style.overflow = 'auto'

  const signature = info.signature.length > SIGNATURE_LIMIT
    ? `${info.signature.slice(0, SIGNATURE_LIMIT)}…`
    : info.signature
  const signatureEl = view.dom.ownerDocument.createElement('pre')
  signatureEl.textContent = signature
  signatureEl.style.margin = '0'
  signatureEl.style.whiteSpace = 'pre-wrap'
  signatureEl.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
  root.appendChild(signatureEl)

  const documentation = tidyDocumentation(info.documentation)
  if (documentation) {
    const docEl = view.dom.ownerDocument.createElement('div')
    docEl.textContent = documentation
    docEl.style.marginTop = '6px'
    docEl.style.whiteSpace = 'pre-wrap'
    docEl.style.color = 'var(--muted-foreground)'
    root.appendChild(docEl)
  }

  for (const tag of info.tags) {
    const tagEl = view.dom.ownerDocument.createElement('div')
    tagEl.textContent = tag
    tagEl.style.marginTop = '4px'
    tagEl.style.whiteSpace = 'pre-wrap'
    tagEl.style.color = 'var(--muted-foreground)'
    tagEl.style.opacity = '0.85'
    root.appendChild(tagEl)
  }

  return root
}

/**
 * 建一个 hover 扩展。
 *
 * @param getScript 读取当前脚本（名字 + 内容）。
 */
export function clipbeamHover(getScript: ScriptGetter): Extension {
  return hoverTooltip(async (view, pos): Promise<Tooltip | null> => {
    const script: ScriptTarget = getScript()
    // CodeMirror 的文档位置与 TypeScript 的偏移都是 UTF-16 单元，可以直接对应
    const offset = Math.min(Math.max(pos, 0), script.source.length)

    const info = await quickInfo(script.name, script.source, offset)
    if (!info) {
      return null
    }

    return {
      pos: offset,
      // 让高亮范围盖住整个标识符
      end: Math.min(offset + info.length, script.source.length),
      // 默认放光标上方：不影响正在写的那一行；空间不够时 CodeMirror 会自动翻到下方
      above: true,
      // 小箭头指向被悬停的符号，避免「这个框在说哪个标识符」的歧义
      arrow: true,
      create: () => ({ dom: renderHover(view, info) }),
    }
  }, {
    hoverTime: HOVER_DELAY,
    // 光标移到别处就关掉，别一直挂着
    hideOnChange: true,
  })
}
