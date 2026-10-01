/**
 * 接收页的调试输出开关与打印器。
 *
 * # 为什么默认关
 *
 * 接收页跑在**远程机器**的浏览器里，键盘通道的速率约 300 字符/秒 —— 文件传输时
 * 「每帧一行」也是成百上千行。默认打开会把控制台淹掉，也没有意义地拖慢接收。
 * 所以开关默认**关**，需要时在远程页面现开，不用重新构建、也不用重新部署。
 *
 * # 怎么打开（任选其一，改完刷新页面）
 *
 * ```js
 * localStorage.setItem('clipbeam-debug', '1')   // 开着
 * localStorage.removeItem('clipbeam-debug')     // 清掉，回到 URL 判定
 * ```
 *
 * 或者在 URL 末尾加 `#debug`（也认 `?debug=1`）。**localStorage 优先于 URL**，
 * 所以 `localStorage.setItem('clipbeam-debug', '0')` 可以临时压掉 URL 里的 `#debug`。
 *
 * # 为什么必须配合构建配置
 *
 * 部署到远程的是 `client-vanilla` 的**压缩产物**（协议 C 内嵌 `dist/index.html`）。
 * 那里若开着 terser 的 `drop_console: true`，**包括 `debug` 在内的所有** `console.*`
 * 调用都会被删掉，这些日志在远程页面上一条都不会存在 —— 所以
 * `packages/client-vanilla/vite.config.ts` 里改成只丢 `log`/`info`/`warn`/`error`，
 * 专门把 `debug` 留下来（见该文件里的注释）。
 *
 * # 用法
 *
 * ```ts
 * import { debug } from './debug'
 * debug('transfer', '收到分片', seq, slice.byteLength)
 * ```
 *
 * 关掉时是空操作，调用点不必自己写 `if`。
 */

/** localStorage 里的开关键。 */
const KEY = 'clipbeam-debug'
/** 前缀让日志在一堆无关输出里可 grep。 */
const PREFIX = '[clipbeam'

function truthy(raw: string | null | undefined): boolean {
  if (raw == null)
    return false
  const v = raw.trim().toLowerCase()
  return v === '1' || v === 'true' || v === 'on' || v === 'yes'
}

/** `#debug` / `#debug=1` / `?debug=1` / `...&debug` 都算打开。 */
function fromUrl(): boolean {
  if (typeof location === 'undefined')
    return false
  const raw = `${location.search}${location.hash}`.toLowerCase()
  // 要求 `debug` 后面是结尾或另一个分隔符，避免把 `#debugging` 也算进来
  return /[?&#]debug(?:=(?:1|true|on|yes))?(?:&|$)/.test(raw)
}

function detect(): boolean {
  try {
    if (typeof localStorage !== 'undefined') {
      const stored = localStorage.getItem(KEY)
      // 显式设置优先（包括用它显式关掉 URL 打开的调试）
      if (stored !== null)
        return truthy(stored)
    }
  }
  catch {
    // 隐私模式 / 禁用存储时 localStorage 会抛错，退回 URL 判定
  }
  return fromUrl()
}

/**
 * 调试开关（模块加载时判定一次：改完 localStorage 需要刷新页面）。
 *
 * 导出它是为了让页面启动时能打印一行「调试已开启」，否则用户没法确认开关生效了
 * —— 一个看不见的开关等于没有开关。
 */
export const DEBUG: boolean = detect()

/** 按需输出一行调试信息；开关关掉时什么都不做。 */
export function debug(scope: string, ...args: unknown[]): void {
  if (!DEBUG)
    return
  // 这一行必须能被构建保留：`client-vanilla` 的 terser 配置只丢弃
  // log/info/warn/error，专门留下 debug（见 vite.config.ts）。
  // eslint-disable-next-line no-console -- 本模块存在的意义就是输出 console.debug
  console.debug(`${PREFIX}:${scope}]`, ...args)
}
