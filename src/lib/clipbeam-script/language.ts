// 脚本编辑器：CodeMirror 6 的源码解析（TypeScript → JavaScript）。
//
// 为什么要在浏览器里再跑一遍 TypeScript：
//   * 语法高亮与补全：`@codemirror/lang-javascript` 自带 JS/TS 的解析（lezer 语法树）；
//   * 类型诊断：用 TypeScript 编译器 API 对脚本做真类型检查（见 typecheck.ts）。
//
// 这里只做「源码 → AST」的第一步，供补全/诊断复用。

import type { Extension } from '@codemirror/state'
import { javascript } from '@codemirror/lang-javascript'

/**
 * 按脚本语言返回 CodeMirror 的语言扩展。
 *
 * `jsx: false`：脚本运行时不支持 JSX/TSX（引擎会明确报错），
 * 打开它只会让编辑器接受跑不了的语法，所以关掉。
 */
export function clipbeamLanguage(language: 'js' | 'ts'): Extension {
  return javascript({ typescript: language === 'ts', jsx: false })
}
