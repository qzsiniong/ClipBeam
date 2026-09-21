/* eslint-disable no-console -- 安装横幅、未模拟命令的提示就是要打在 DevTools 里给人看的 */
import type { MockWindowLabel } from './types'
/**
 * **浏览器调试用的 Tauri mock**：真正安装的那一层（`mockWindows` + `mockIPC`）。
 *
 * # 为什么需要
 *
 * Tauri 的 webview 才会注入 `window.__TAURI_INTERNALS__`。用 Chrome 直接打开 Vite 的
 * `http://localhost:5173` 时它不是 undefined 的后果是：`src/App.vue` 在 setup 阶段调
 * `getCurrentWindow().label`（实现读 `__TAURI_INTERNALS__.metadata.currentWindow.label`），
 * 直接 `TypeError` → 白屏；就算绕过，25 个 `invoke` 命令、`listen` 事件、系统弹框也全没有后端。
 *
 * # 做法
 *
 * 用官方 `@tauri-apps/api/mocks`：`mockWindows(label)` 补上窗口元信息，
 * `mockIPC(handler, { shouldMockEvents: true })` 拦 `invoke` 并让 `listen`/`emit` 在本地成环。
 * **不自己手搓 `__TAURI_INTERNALS__`**，也不改任何业务代码。
 *
 * # 守卫
 *
 * 入口 `src/dev/tauri-mock.ts` 负责判断：`import.meta.env.DEV` 且 `!("__TAURI_INTERNALS__" in window)`。
 * 真 Tauri（`pnpm tauri dev` / 打包）下 `__TAURI_INTERNALS__` 存在 → 直接 no-op；
 * 生产构建里 `import.meta.env.DEV` 被常量折叠，且这一层是动态 import 的，整块不进生产包。
 *
 * # 覆盖范围（浏览器里能调什么）
 *
 * 窗口 label：`?window=scripting` 显式指定，否则按 hash 路由推断（见 `resolveWindowLabel`）；
 * 全部自定义命令（`list_scripts` / `read_script` / `get_config` / `list_capabilities` …）
 *   走内存夹具（见 `commands.ts`）；`start_script` 还会模拟一次运行（见 `events.ts`）。
 *
 * **不覆盖**（浏览器里做不到，仍要真机验证）：真实键盘注入、剪贴板、文件系统、
 * Rust 侧 worker 与二维码识别、系统原生弹框。
 */
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import { handleCommand } from './commands'
import { createDevHelpers } from './dev-helpers'

/** 只用一次：同一个 label 重复安装会互相覆盖。 */
let installed = false

/**
 * 从 URL 推断"当前是哪个窗口"。
 *
 * 优先 `?window=<label>`；否则按 hash 路由对应到 Tauri 里同名的窗口
 * （`#/scripting` → scripting、`#/progress` → progress、`#/standby` → standby、其余 → main）。
 */
export function resolveWindowLabel(search: string, hash: string): MockWindowLabel {
  const explicit = new URLSearchParams(search).get('window')
  if (explicit === 'main' || explicit === 'scripting' || explicit === 'progress' || explicit === 'standby')
    return explicit
  if (hash.startsWith('#/scripting'))
    return 'scripting'
  if (hash.startsWith('#/progress'))
    return 'progress'
  if (hash.startsWith('#/standby'))
    return 'standby'
  return 'main'
}

/** 安装 mock；重复调用或真 Tauri 环境下直接返回。 */
export function installTauriMock() {
  if (installed)
    return
  if (!import.meta.env.DEV)
    return
  if (typeof window === 'undefined' || '__TAURI_INTERNALS__' in window)
    return
  installed = true

  const label = resolveWindowLabel(window.location.search, window.location.hash)
  mockWindows(label)
  mockIPC(handleCommand, { shouldMockEvents: true })

  window.__CLIPBEAM_DEV__ = createDevHelpers()

  console.info(
    `[clipbeam] 浏览器调试模式：Tauri API 已被 mock（窗口 label = ${label}）。`
    + '用 window.__CLIPBEAM_DEV__ 可以手动造事件；真机行为不受影响。',
  )
}
