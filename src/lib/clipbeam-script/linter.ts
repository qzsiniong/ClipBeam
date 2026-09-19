// 脚本编辑器：把 TypeScript 诊断接进 CodeMirror 的 lint 面板。
//
// 诊断本身由 typecheck.ts 提供（用 TypeScript 编译器 API 做真类型检查），
// 这里只负责「什么时候算、如何映射、失败怎么办」。

import type { Diagnostic, LintSource } from '@codemirror/lint'
import type { Extension } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import { linter } from '@codemirror/lint'
import { typecheckScript } from './typecheck'

/** 停止输入多久之后开始检查（毫秒）。太长会让标红显得迟钝，太短会频繁计算。 */
const LINT_DELAY = 400

/**
 * 建一个 lint 扩展。
 *
 * `scriptName` 用 getter 传入：脚本名（决定按 `.ts` 还是 `.js` 检查）会随用户切换文件变化，
 * 而扩展只在创建时组装一次。
 */
export function clipbeamLinter(getScriptName: () => string): Extension {
  const source: LintSource = async (view: EditorView): Promise<Diagnostic[]> => {
    const source = view.state.doc.toString()
    try {
      const diagnostics = await typecheckScript(getScriptName(), source)
      return diagnostics.map(diagnostic => ({
        from: diagnostic.from,
        to: diagnostic.to,
        severity: diagnostic.severity,
        message: diagnostic.message,
        // 单字符诊断（例如「缺少分号」）也要能看见
        markClass: undefined,
      }))
    }
    catch (error) {
      // 诊断服务本身失败（例如 TypeScript 加载异常）时不要刷屏，
      // 也不能因为编辑器的问题打断用户写脚本
      console.warn('[clipbeam] 脚本类型检查失败：', error)
      return []
    }
  }

  return linter(source, { delay: LINT_DELAY })
}
