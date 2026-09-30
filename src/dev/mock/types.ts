/**
 * 浏览器 mock 的类型定义（都是 Rust 侧结构的镜像）。
 *
 * 这里只放**类型**：运行时状态在 `state.ts`，命令表在 `commands.ts`。
 */

/** 脚本元信息（对应 Rust `clipbeam_scripting::scripts::ScriptMeta`）。 */
export interface MockScriptMeta { name: string, path: string, language: 'js' | 'ts' }

/** Console 一行（对应 Rust `clipbeam_worker::console_panel::ConsoleLine`）。 */
export interface MockConsoleLine { seq: number, level: string, text: string, ts: number }

/** 能力清单（对应 Rust `clipbeam_scripting::Capability`）。 */
export interface MockCapability { name: string, signature: string, doc: string, source: string }

/** 任务结束时 Console 里的结果行（对应 Rust `TaskOutcome`）。 */
export interface MockOutcome { title: string, body: string }

/** 浏览器 mock 的窗口 label（与 `tauri.conf.json` 里的窗口 label 同名）。 */
export type MockWindowLabel = 'main' | 'scripting' | 'progress' | 'standby'

/** 插件状态（对应 Rust `PluginState`，serde 小写）。 */
export type MockPluginState = 'invalid' | 'disabled' | 'active' | 'error' | 'stoptimeout'

/** 一个插件（对应 Rust `PluginInfo`）。 */
export interface MockPluginInfo {
  id: string
  name: string
  version: string
  description: string | null
  author: string | null
  dir: string
  entry: string
  state: MockPluginState
  permissions: string[]
  menus: [string, string][]
  error: string | null
  usable: boolean
}

/** 浏览器调试助手（仅 mock 模式挂到 `window.__CLIPBEAM_DEV__`）。 */
export interface ClipBeamDevHelpers {
  /** 触发一个后端事件（走被 mock 的 `emit`，本机 `listen` 立刻收到）。 */
  emit: (event: string, payload?: unknown) => Promise<void>
  /** 往 Console 缓冲里塞一行，并推给面板。 */
  consoleLine: (level: string, text: string) => void
  /** 完整演练一次"运行脚本"：worker-started → 几行日志 → worker-finished。 */
  runScript: (name?: string) => Promise<void>
  /** 当前 mock 的脚本清单 / 配置（只读视图）。 */
  scripts: () => MockScriptMeta[]
  config: () => Record<string, unknown>
  /** 当前 mock 的插件清单（只读视图）。 */
  plugins: () => MockPluginInfo[]
  /** 造一条插件 toast（验证提示样式，不必真去点托盘菜单）。 */
  pluginToast: (message?: string, level?: string) => Promise<void>
}

declare global {
  interface Window {
    /** 浏览器 mock 模式的调试入口；真 Tauri 下不存在。 */
    __CLIPBEAM_DEV__?: ClipBeamDevHelpers
  }
}
