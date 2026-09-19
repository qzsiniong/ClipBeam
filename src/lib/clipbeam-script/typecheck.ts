// 脚本编辑器：类型诊断（供 CodeMirror 的 lint 面板使用）。
//
// 真正的语言服务在 [`language-service.ts`](./language-service.ts)：诊断、悬停、补全、
// 参数信息共用同一个服务与同一份虚拟文件表。这里只做「调一次 + 缓存结果」。
//
// 缓存的必要性：lint 每次击键后（延迟 400ms）都会算一遍，而 TypeScript 的类型检查
// 在大脚本上不是免费的。key 用「脚本名 + 内容」，内容不变就直接返回上次结果。

import type { ScriptDiagnostic } from './language-service'
import { typecheck } from './language-service'

export type { ScriptDiagnostic } from './language-service'

/** 接口版本（改行为时 +1，避免旧的缓存结果被复用）。 */
const CACHE_VERSION = 'v2'

/** 诊断缓存：key 是「脚本名 + 内容」，最多保留 32 条。 */
const cache = new Map<string, ScriptDiagnostic[]>()
const CACHE_LIMIT = 32

/**
 * 对一个脚本做类型检查。
 *
 * @param name 脚本文件名（`.ts` / `.js`），决定按 TS 还是 JS 检查。
 * @param source 源码。
 */
export async function typecheckScript(name: string, source: string): Promise<ScriptDiagnostic[]> {
  const key = `${CACHE_VERSION}\u0000${name}\u0000${source}`
  const cached = cache.get(key)
  if (cached) {
    return cached
  }

  const result = await typecheck({ name, source })

  cache.set(key, result)
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value
    if (oldest) {
      cache.delete(oldest)
    }
  }
  return result
}

/** 清空诊断缓存（测试或切换工作区时用）。 */
export function clearTypecheckCache() {
  cache.clear()
}
