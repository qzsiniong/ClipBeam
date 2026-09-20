// 脚本编辑器：**共享**的 TypeScript 语言服务。
//
// # 为什么需要这一层
//
// 编辑器的三个功能其实问的是同一个问题 —— 「光标处是什么类型」：
//
// | 功能 | 语言服务 API |
// |---|---|
// | 诊断标红/标黄（`linter.ts`） | `getSyntacticDiagnostics` + `getSemanticDiagnostics` |
// | 悬停提示（`hover.ts`） | `getQuickInfoAtPosition` |
// | 代码补全（`completion.ts`） | `getCompletionsAtPosition` + `getCompletionEntryDetails` |
// | 参数信息（`signature.ts`） | `getSignatureHelpItems` |
//
// 所以只建**一个**语言服务、只维护**一份**虚拟文件表，四处共用。之前是每次诊断都
// `createLanguageService()` + `dispose()`，既慢又危险：内容变了但 `getScriptVersion`
// 返回常量 `'0'`，TypeScript 会认定文件没变而复用陈旧快照 —— 现在版本号改成内容哈希。
//
// # 虚拟文件表
//
// | 路径 | 内容 |
// |---|---|
// | `/lib.es5.d.ts` | 核心对象（**`ArrayBuffer` / `Uint8Array` / `Promise` / `Map` 都在这里**） |
// | `/lib.es2022.d.ts` | ES2022 新增（`Object.hasOwn`、`Array.prototype.at` 等） |
// | `/script-engine.d.ts` | `crates/script-engine/src/spec/engine.d.ts`（引擎补齐的标准全局） |
// | `/clipbeam-scripting.d.ts` | `crates/clipbeam-scripting/src/spec/clipbeam.d.ts`（能力命名空间与全部能力） |
// | `/<脚本名>` | 用户脚本 |
//
// 两份 `.d.ts` 直接读 crate 源码，因此与 Rust 侧**同一个真源**：类型提示、
// hover 文档、运行期绑定三者不会漂移。

import type * as TsModule from 'typescript'

/** 当前脚本：名字决定按 `.ts` 还是 `.js` 解析，源码是内容。 */
export interface ScriptTarget {
  name: string
  source: string
}

/** 一条诊断（偏移量已换算成脚本内的绝对字符位置）。 */
export interface ScriptDiagnostic {
  from: number
  to: number
  severity: 'error' | 'warning'
  message: string
}

/** 悬停信息（已拍平成纯文本，由 hover.ts 组装 DOM）。 */
export interface HoverInfo {
  /** 签名行，例如 `(property) type_str: (text: string, delayMs?: number) => void`。 */
  signature: string
  /** JSDoc 正文（可能为空）。 */
  documentation: string
  /** `@param` / `@returns` 等标签行。 */
  tags: string[]
  /** 该类型在源码里覆盖的长度（用于定位 tooltip 的结束位置）。 */
  length: number
}

/** 补全项（语言服务给的原始条目 + 惰性详情）。 */
export interface TsCompletionEntry {
  name: string
  kind: string
  sortText?: string
  /** 来自哪个声明文件（例如能力来自 `clipbeam.d.ts`）。 */
  source?: string
  /** 惰性取详情：签名 + JSDoc。 */
  getDetails: () => { signature: string, documentation: string }
}

/** 一个候选签名。 */
export interface SignatureInfo {
  /** 完整签名文本，例如 `zstd(data: BinaryInput, level?: number): ArrayBuffer`。 */
  label: string
  /** 各参数文本（用于高亮当前参数）。 */
  parameters: string[]
  /** 当前参数下标（`-1` 表示还没进括号）。 */
  activeParameter: number
  /** 第几个重载（0 起）以及总数。 */
  activeSignature: number
  signatureCount: number
  /** 文档（可能为空）。 */
  documentation: string
}

/**
 * 需要的 lib 文件。
 *
 * **两个都要**：`lib.es2022.d.ts` 只声明 ES2022 新增的部分，它引用的
 * `ArrayBuffer` / `Uint8Array` / `Promise` 等核心对象定义在 `lib.es5.d.ts` 里。
 * 只加载前者时，这些名字解析失败 —— 表现为「`$.md5(x)` 里的 `ArrayBuffer` 变成 `any`，
 * 传错类型也不报错」，全靠类型的能力提示同时失效（实测踩过）。
 */
const LIB_NAMES = ['lib.es5.d.ts', 'lib.es2022.d.ts'] as const

const SCRIPT_LIB_FILE = '/script-engine.d.ts'
const EXTENSION_LIB_FILE = '/clipbeam-scripting.d.ts'

/** 虚拟路径 → TypeScript 包内的 lib 文件名。 */
function libPath(name: string): string {
  return `/${name}`
}

/** 虚拟文件表：所有内容都放内存。 */
const files: Record<string, string> = {}
/** 每个文件的版本号：脚本用内容哈希，声明文件恒为 `'1'`（一次加载不再变）。 */
const versions: Record<string, string> = {}

/** 语言服务与 TypeScript 模块（懒加载后缓存）。 */
let tsModule: typeof TsModule | null = null
let service: TsModule.LanguageService | null = null
let currentScriptFile: string | null = null
let loading: Promise<void> | null = null
/** 加载失败只记一次，避免每次击键都刷日志。 */
let loadFailed = false

/** FNV-1a：只需要「内容不同则版本不同」，不需要密码学强度。 */
function contentHash(text: string): string {
  let hash = 0x811C9DC5
  for (let index = 0; index < text.length; index++) {
    hash ^= text.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }
  return (hash >>> 0).toString(16)
}

/** 把语法检查用的选项集中在一处：诊断/补全/悬停/签名都共用它。 */
function compilerOptions(ts: typeof TsModule): TsModule.CompilerOptions {
  return {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    // 脚本里允许顶层 await：引擎按 module + async 求值，这里保持一致
    moduleDetection: ts.ModuleDetectionKind.Force,
    lib: [...LIB_NAMES],
    types: [],
    strict: true,
    // `.js` 也检查（脚本是用户手写的，拼错方法名很常见）
    allowJs: true,
    checkJs: true,
    noEmit: true,
    noUnusedLocals: false,
    noUnusedParameters: false,
    skipLibCheck: true,
    // 让补全/悬停带上 JSDoc，与 IDE 一致
    includeCompletionsWithInsertText: true,
  }
}

/**
 * 懒加载 TypeScript 与两份声明文件，并建立语言服务。
 *
 * 失败时不让编辑器崩：调用方按「未就绪」处理（补全退回能力清单、hover 不弹）。
 */
export function ensureService(): Promise<void> {
  if (service) {
    return Promise.resolve()
  }
  if (!loading) {
    loading = (async () => {
      const [tsImport, engine, extension, es5, es2022] = await Promise.all([
        import('typescript'),
        import('../../../crates/script-engine/src/spec/engine.d.ts?raw'),
        import('../../../crates/clipbeam-scripting/src/spec/clipbeam.d.ts?raw'),
        import('typescript/lib/lib.es5.d.ts?raw'),
        import('typescript/lib/lib.es2022.d.ts?raw'),
      ])

      // `typescript` 是 CJS，动态 import 后可能挂在 default 上
      const ts = ((tsImport as unknown as { default?: typeof TsModule }).default
        ?? (tsImport as unknown as typeof TsModule))

      files[libPath('lib.es5.d.ts')] = es5.default
      files[libPath('lib.es2022.d.ts')] = es2022.default
      files[SCRIPT_LIB_FILE] = engine.default
      files[EXTENSION_LIB_FILE] = extension.default
      for (const path of [libPath('lib.es5.d.ts'), libPath('lib.es2022.d.ts'), SCRIPT_LIB_FILE, EXTENSION_LIB_FILE]) {
        versions[path] = '1'
      }

      const host: TsModule.LanguageServiceHost = {
        getScriptFileNames: () => {
          const list = [libPath('lib.es5.d.ts'), libPath('lib.es2022.d.ts'), SCRIPT_LIB_FILE, EXTENSION_LIB_FILE]
          if (currentScriptFile) {
            list.push(currentScriptFile)
          }
          return list
        },
        // 版本号按内容哈希变化：内容一变，TypeScript 就知道要重新解析
        getScriptVersion: fileName => versions[fileName] ?? '1',
        getScriptSnapshot: (fileName) => {
          const content = files[fileName]
          return content === undefined ? undefined : ts.ScriptSnapshot.fromString(content)
        },
        getCurrentDirectory: () => '/',
        getCompilationSettings: () => compilerOptions(ts),
        // TypeScript 用这个决定「默认 lib」是哪个；其余 lib 由 `lib` 选项带进来
        getDefaultLibFileName: () => libPath(LIB_NAMES[0]),
        fileExists: fileName => files[fileName] !== undefined,
        readFile: fileName => files[fileName],
        readDirectory: () => [],
        directoryExists: () => true,
        getDirectories: () => [],
      }

      tsModule = ts
      service = ts.createLanguageService(host, ts.createDocumentRegistry())
    })().catch((error: unknown) => {
      if (!loadFailed) {
        loadFailed = true
        console.warn('[clipbeam] TypeScript 语言服务加载失败，编辑器将退化为能力清单补全：', error)
      }
    })
  }
  return loading
}

/** 语言服务是否已可用（补全用它决定走 TS 还是兜底）。 */
export function isServiceReady(): boolean {
  return service !== null
}

/** 同步拿到服务（未就绪时返回 `null`，调用方自己兜底）。 */
function takeService(): { ts: typeof TsModule, service: TsModule.LanguageService } | null {
  if (!tsModule || !service) {
    return null
  }
  return { ts: tsModule, service }
}

/** 脚本在虚拟文件系统里的路径。 */
function scriptPath(name: string): string {
  return `/${name}`
}

/**
 * 登记当前脚本（内容 + 版本）。
 *
 * 每次补全/hover/诊断前调用一次即可：内容没变就是空操作，变了才更新版本号。
 */
export async function setScript(target: ScriptTarget): Promise<string | null> {
  await ensureService()
  if (!service) {
    return null
  }

  const path = scriptPath(target.name)
  // 换脚本文件时清掉上一份，避免它继续参与类型推断
  if (currentScriptFile && currentScriptFile !== path) {
    delete files[currentScriptFile]
    delete versions[currentScriptFile]
  }
  currentScriptFile = path
  files[path] = target.source
  versions[path] = contentHash(target.source)
  return path
}

/**
 * 取当前脚本在虚拟文件系统里的路径。
 *
 * `setScript` 之前查询任何东西都没有意义，因此所有查询函数都先调它。
 */
async function targetPath(name: string, source: string): Promise<{ path: string, ts: typeof TsModule, service: TsModule.LanguageService } | null> {
  const path = await setScript({ name, source })
  const taken = takeService()
  if (!path || !taken) {
    return null
  }
  return { path, ...taken }
}

/** 把 TypeScript 的 `displayParts` 拍平成纯文本。 */
export function displayPartsToText(parts: readonly TsModule.SymbolDisplayPart[] | undefined): string {
  return (parts ?? []).map(part => part.text).join('')
}

/** 把 JSDoc 片段拍平成多行文本。 */
function documentationText(parts: readonly TsModule.SymbolDisplayPart[] | undefined): string {
  return displayPartsToText(parts).trim()
}

/** 夹取偏移量到合法范围（TypeScript 偶尔会给出越界值）。 */
function clampOffset(offset: number, source: string): number {
  return Math.min(Math.max(offset, 0), source.length)
}

/**
 * 类型诊断。
 *
 * 只报告**当前脚本**的问题：`lib.d.ts` / 我们自己的声明文件里的问题不该由用户承担。
 */
export async function typecheck(target: ScriptTarget): Promise<ScriptDiagnostic[]> {
  const ready = await targetPath(target.name, target.source)
  if (!ready) {
    return []
  }
  const { ts, service: languageService, path } = ready

  const found = [
    ...languageService.getSyntacticDiagnostics(path),
    ...languageService.getSemanticDiagnostics(path),
  ]

  const length = target.source.length
  const result: ScriptDiagnostic[] = []
  for (const diagnostic of found) {
    if (!diagnostic.file || diagnostic.file.fileName !== path) {
      continue
    }
    const start = Math.min(Math.max(diagnostic.start ?? 0, 0), length)
    const end = Math.min(Math.max(start + (diagnostic.length ?? 0), start), length)
    const text = ts.flattenDiagnosticMessageText(diagnostic.messageText, ' ')
    result.push({
      from: start,
      to: end,
      severity: diagnostic.category === ts.DiagnosticCategory.Error ? 'error' : 'warning',
      message: diagnostic.code ? `TS${diagnostic.code}: ${text}` : text,
    })
  }
  return result
}

/** 悬停信息：类型签名 + JSDoc。 */
export async function quickInfo(name: string, source: string, offset: number): Promise<HoverInfo | null> {
  const ready = await targetPath(name, source)
  if (!ready) {
    return null
  }
  const { service: languageService, path } = ready

  const info = languageService.getQuickInfoAtPosition(path, clampOffset(offset, source))
  if (!info) {
    return null
  }

  const signature = displayPartsToText(info.displayParts).trim()
  if (!signature) {
    return null
  }

  return {
    signature,
    documentation: documentationText(info.documentation),
    tags: (info.tags ?? []).map((tag) => {
      const name = `@${tag.name}`
      const text = documentationText(tag.text)
      return text ? `${name} ${text}` : name
    }),
    length: Math.max(info.textSpan.length, 1),
  }
}

/** 补全候选：条目名 + 类别 + 惰性详情。 */
export async function completions(
  name: string,
  source: string,
  offset: number,
  triggerCharacter?: string,
): Promise<TsCompletionEntry[] | null> {
  const ready = await targetPath(name, source)
  if (!ready) {
    return null
  }
  const { service: languageService, path } = ready

  const info = languageService.getCompletionsAtPosition(path, clampOffset(offset, source), {
    // 关键：带上 triggerCharacter，TypeScript 才知道是 `.` 之后的成员补全
    triggerCharacter: triggerCharacter as TsModule.CompletionsTriggerCharacter | undefined,
    includeCompletionsWithInsertText: true,
    includeAutomaticOptionalChainCompletions: true,
    includeCompletionsForModuleExports: false,
    useLabelDetailsInCompletionEntries: true,
  } as TsModule.GetCompletionsAtPositionOptions)
  if (!info) {
    return null
  }

  return info.entries.map(entry => ({
    name: entry.name,
    kind: entry.kind,
    sortText: entry.sortText,
    source: entry.source,
    getDetails: () => {
      const details = languageService.getCompletionEntryDetails(
        path,
        clampOffset(offset, source),
        entry.name,
        {},
        entry.source,
        undefined,
        entry.data,
      )
      if (!details) {
        return { signature: '', documentation: '' }
      }
      return {
        signature: displayPartsToText(details.displayParts).trim(),
        documentation: documentationText(details.documentation),
      }
    },
  }))
}

/** 参数信息（签名提示）。 */
export async function signatureHelp(
  name: string,
  source: string,
  offset: number,
): Promise<SignatureInfo | null> {
  const ready = await targetPath(name, source)
  if (!ready) {
    return null
  }
  const { service: languageService, path } = ready

  const items = languageService.getSignatureHelpItems(path, clampOffset(offset, source), undefined)
  if (!items || items.items.length === 0) {
    return null
  }

  const index = Math.min(Math.max(items.selectedItemIndex, 0), items.items.length - 1)
  const item = items.items[index]

  // 签名文本 = 前缀 + 参数 + 后缀；参数之间用分隔符（通常是 `, `）
  const prefix = displayPartsToText(item.prefixDisplayParts)
  const suffix = displayPartsToText(item.suffixDisplayParts)
  const separator = displayPartsToText(item.separatorDisplayParts)
  const parameterTexts = item.parameters.map((parameter) => {
    const rest = parameter.isRest ? '...' : ''
    return `${rest}${displayPartsToText(parameter.displayParts)}`
  })

  return {
    label: `${prefix}${parameterTexts.join(separator)}${suffix}`,
    parameters: parameterTexts,
    // 还没输入参数时 TypeScript 会给 -1
    activeParameter: items.argumentIndex,
    activeSignature: index,
    signatureCount: items.items.length,
    documentation: documentationText(item.documentation),
  }
}

/**
 * 预加载（首次打开脚本页时调用，让第一次 hover/补全不用等）。
 *
 * 失败静默：功能会退化为兜底补全。
 */
export async function warmup(): Promise<void> {
  await ensureService()
}

/** 清空脚本登记（切换文件时避免旧内容参与推断）。 */
export function resetScript(): void {
  if (currentScriptFile) {
    delete files[currentScriptFile]
    delete versions[currentScriptFile]
    currentScriptFile = null
  }
}
