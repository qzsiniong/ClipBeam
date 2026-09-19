// 脚本编辑器：用 TypeScript 编译器 API 做**真类型诊断**。
//
// 为什么值得引入 TypeScript 本身（它已经是 devDependency）：
//   * 能报出「运行期才炸」的错误，例如 `$.file(123)`、`$.md5("not a buffer")`、
//     写了运行时不存在的 API、`await` 用错地方；
//   * 诊断位置与用户看到的一致（`getSyntacticDiagnostics` + `getSemanticDiagnostics`）。
//
// 实现要点：
//   * 虚拟文件系统：内置 lib.es2022.d.ts + 两个 crate 的 bindings.d.ts + 当前脚本；
//   * TypeScript 体积不小，用动态 `import()` 懒加载，首屏不受影响；
//   * 同一份内容只算一次（内容缓存），避免每次击键都全量重算。

import type * as TsModule from 'typescript'

/** 一条诊断（已转成 CodeMirror 需要的**绝对字符偏移**）。 */
export interface ScriptDiagnostic {
  /** 起始偏移（字符）。 */
  from: number
  /** 结束偏移（字符）。 */
  to: number
  /** `error` 在编辑器里标红，`warning` 标黄。 */
  severity: 'error' | 'warning'
  /** 单行展示信息。 */
  message: string
}

/** 接口版本（`typescript` 的版本或我们改声明文件时手动 +1）。 */
const CACHE_VERSION = 'v1'

/** 诊断缓存：key 是「脚本名 + 内容」，最多保留 32 条。 */
const cache = new Map<string, ScriptDiagnostic[]>()
const CACHE_LIMIT = 32

interface TypeScriptBundle {
  ts: typeof TsModule
  coreBindings: string
  extensionBindings: string
  /** TypeScript 自带的默认 lib（`lib.es2022.d.ts`）内容。 */
  defaultLib: string
}

/** 默认 lib 在 TypeScript 包内的相对路径（`lib` 配置项 + `.d.ts`）。 */
const DEFAULT_LIB_NAME = 'lib.es2022.d.ts'

let bundlePromise: Promise<TypeScriptBundle> | null = null

/**
 * 懒加载 TypeScript 与两份声明文件。
 *
 * 两个 `.d.ts` 直接从 crate 源码读（`?raw`），因此**与 Rust 侧同一个真源**，
 * 不会出现「编辑器里有、运行期没有」的漂移。
 *
 * 默认 lib 走 `typescript` 包自己的 `?raw` 资源，而不是 `node:fs` —— 编辑器跑在
 * WebView 里，不能读磁盘。
 */
function loadBundle(): Promise<TypeScriptBundle> {
  if (!bundlePromise) {
    bundlePromise = (async () => {
      const [tsModule, core, extension, lib] = await Promise.all([
        import('typescript'),
        import('../../../crates/clipbeam-script/src/spec/clipbeam.d.ts?raw'),
        import('../../../crates/clipbeam-scripting/src/spec/clipbeam.d.ts?raw'),
        import('typescript/lib/lib.es2022.d.ts?raw'),
      ])

      // `typescript` 是 CJS，动态 import 后可能挂在 default 上
      const ts = ((tsModule as unknown as { default?: typeof TsModule }).default
        ?? (tsModule as unknown as typeof TsModule))

      return {
        ts,
        coreBindings: core.default,
        extensionBindings: extension.default,
        defaultLib: lib.default,
      }
    })()
  }
  return bundlePromise
}

/** 把 TypeScript 的诊断分类映射到我们的严重级别。 */
function severityOf(ts: typeof TsModule, category: TsModule.DiagnosticCategory): 'error' | 'warning' {
  return category === ts.DiagnosticCategory.Error ? 'error' : 'warning'
}

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

  const { ts, coreBindings, extensionBindings, defaultLib } = await loadBundle()

  const scriptFile = `/${name}`
  const coreFile = '/clipbeam-script.d.ts'
  const extensionFile = '/clipbeam-scripting.d.ts'
  const libFile = `/${DEFAULT_LIB_NAME}`

  const files: Record<string, string> = {
    [scriptFile]: source,
    [coreFile]: coreBindings,
    [extensionFile]: extensionBindings,
    [libFile]: defaultLib,
  }

  const options: TsModule.CompilerOptions = {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    // 脚本里允许顶层 await：引擎按 module + async 求值，这里保持一致
    moduleDetection: ts.ModuleDetectionKind.Force,
    lib: [DEFAULT_LIB_NAME],
    types: [],
    strict: true,
    // `.js` 也检查（脚本是用户手写的，拼错方法名很常见）
    allowJs: true,
    checkJs: true,
    noEmit: true,
    noUnusedLocals: false,
    noUnusedParameters: false,
    skipLibCheck: true,
  }

  const host: TsModule.LanguageServiceHost = {
    getScriptFileNames: () => [scriptFile, coreFile, extensionFile],
    getScriptVersion: () => '0',
    getScriptSnapshot: (fileName) => {
      const content = files[fileName]
      return content === undefined ? undefined : ts.ScriptSnapshot.fromString(content)
    },
    getCurrentDirectory: () => '/',
    getCompilationSettings: () => options,
    getDefaultLibFileName: () => libFile,
    fileExists: fileName => files[fileName] !== undefined,
    readFile: fileName => files[fileName],
    readDirectory: () => [],
    directoryExists: () => true,
    getDirectories: () => [],
  }

  const service = ts.createLanguageService(host, ts.createDocumentRegistry())
  const found = [
    ...service.getSyntacticDiagnostics(scriptFile),
    ...service.getSemanticDiagnostics(scriptFile),
  ]
  service.dispose()

  // 把偏移量夹到源码范围内（TypeScript 偶尔会给出越界的长度）
  const length = source.length
  const result: ScriptDiagnostic[] = []
  for (const diagnostic of found) {
    // 只报告当前脚本的诊断：声明文件/内建库里的问题不该由用户承担
    if (!diagnostic.file || diagnostic.file.fileName !== scriptFile) {
      continue
    }
    const start = Math.min(Math.max(diagnostic.start ?? 0, 0), length)
    const end = Math.min(Math.max(start + (diagnostic.length ?? 0), start), length)
    const text = ts.flattenDiagnosticMessageText(diagnostic.messageText, ' ')
    result.push({
      from: start,
      to: end,
      severity: severityOf(ts, diagnostic.category),
      message: diagnostic.code ? `TS${diagnostic.code}: ${text}` : text,
    })
  }

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

/** 是否已加载 TypeScript（首次打开脚本页时会触发，供 UI 显示「类型检查已就绪」）。 */
export function isReady(): boolean {
  return bundlePromise !== null
}
