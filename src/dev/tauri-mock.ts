/**
 * **浏览器调试用的 Tauri mock —— 入口**（只在 dev + 真浏览器里生效）。
 *
 * Tauri 的 webview 才会注入 `window.__TAURI_INTERNALS__`；用 Chrome 直接打开 Vite 的
 * `http://localhost:5173` 时它不存在的后果是：`src/App.vue` 在 setup 阶段调
 * `getCurrentWindow().label` 直接 `TypeError` → 白屏。
 *
 * 这个文件只做两件事：**判断该不该装**，然后**动态 import** 真正的实现。
 * 动态 import 是必要的：mock 的实现分在 `src/dev/mock/` 几个模块里，模块级夹具
 * （`?raw` 引入的种子源码、命令表、内存 Map）在静态 import 下会被 Rollup 判定为有副作用
 * 而留在生产包里；放在 `import.meta.env.DEV` 守卫之后的动态 import 里，整块不进生产包。
 *
 * 实现（`src/dev/mock/`）：
 * - `types.ts` 类型（Rust 侧结构的镜像）
 * - `state.ts` 夹具 + 内存状态 + 访问器
 * - `events.ts` 事件编排（Console 行、worker-*、模拟运行）
 * - `commands.ts` 命令表（`invoke` → 夹具）
 * - `dev-helpers.ts` `window.__CLIPBEAM_DEV__`
 * - `install.ts` 安装（`mockWindows` + `mockIPC`）与窗口 label 推断
 *
 * 覆盖范围、缺口与手动造事件的方法见 `mock/install.ts`、`mock/dev-helpers.ts` 顶部注释。
 */

/** 在浏览器里装上 Tauri mock；真 Tauri / 生产构建下立刻返回（不加载实现）。 */
export async function installTauriMockInBrowser(): Promise<void> {
  if (!import.meta.env.DEV)
    return
  if (typeof window === 'undefined' || '__TAURI_INTERNALS__' in window)
    return
  const { installTauriMock } = await import('./mock/install')
  installTauriMock()
}
