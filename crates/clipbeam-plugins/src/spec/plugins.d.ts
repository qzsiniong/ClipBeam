// src/spec/plugins.d.ts —— 插件能力 `$plugin` 的类型提示（全局声明）
//
// 这个文件是「全局声明」（没有 import/export），通过 tsconfig.scripts.json 的 include
// 生效（与 script-engine 的 engine.d.ts、clipbeam-scripting 的 clipbeam.d.ts 并列）。
//
// 这里声明的是**插件框架决定的东西**：插件能力命名空间的名字（`ClipBeamPlugin` / `$plugin`）
// 与挂在它上面的全部能力 —— 引擎不定义任何能力，引擎补齐的标准全局（`sleep` / 定时器 /
// `TextDecoder` / `atob` …）声明在 script-engine 的同名文件里。
//
// 名字的真源是 `clipbeam_plugins::NAMESPACE` / `NAMESPACE_ALIAS`；这里是**类型层面**的对应
// 声明（TypeScript 取不到运行期字符串，所以名字必须再写一次）。
//
// 改插件能力时记得同步这个文件（tests/spec_sync.rs 会校验不漂移）。

// 依赖引擎声明的标准全局（`sleep` / `console` / `TextDecoder` / 定时器 …）：
// 插件入口用得到它们，而它们不属于能力命名空间。
//
// 下面那条 `reference` 是**仓库内特有**的，应用把这份文件拷到用户数据目录时
// 会把它整段删掉 —— 在平铺的用户目录里声明文件彼此同级，那条相对仓库结构的路径
// 指不到任何东西。
// PORTABLE-BEGIN
/// <reference path="../../../script-engine/src/spec/engine.d.ts" />
// PORTABLE-END

/** toast 等级（决定配色）。 */
type PluginToastLevel = 'info' | 'success' | 'warning' | 'error'

/** `$plugin.toast` 的选项。 */
interface PluginToastOptions {
	/** 等级；缺省 `'info'`。 */
	level?: PluginToastLevel
	/** 自动消失时间（毫秒）；缺省 4000，传 0 表示不自动消失。 */
	durationMs?: number
}

/** `$plugin.alert` / `$plugin.confirm` 的选项。 */
interface PluginDialogOptions {
	/** 对话框标题；缺省由宿主给默认标题。 */
	title?: string
	/**
	 * 按钮组合；缺省 `alert` 是 `'ok'`、`confirm` 是 `'okCancel'`。
	 *
	 * 传对象时 `primary` 必填（否则用户没有「确定」可点）。
	 */
	buttons?: 'ok' | 'okCancel' | { primary: string, secondary?: string, tertiary?: string }
	/** 超时（毫秒）；缺省不超时。超时按「没有回答」处理。 */
	timeoutMs?: number
}

/** 托盘菜单动作的载荷（`action.id` 对应 plugin.json 里的 `menus[].id`）。 */
interface PluginTrayAction {
	id: string
}

/** `$plugin.tray`：托盘能力。 */
interface ClipBeamPluginTray {
	/**
	 * 登记托盘菜单动作的回调。
	 *
	 * 菜单项被点击时调用 `callback`，参数是 `{ id }`。一个插件只有一个回调入口：
	 * 具体是哪个菜单项由 `action.id` 区分 —— 因此加菜单项不需要改 JS。
	 *
	 * 重复调用会覆盖上一次登记。
	 */
	onAction(callback: (action: PluginTrayAction) => void): void

	/** 设置托盘图标的鼠标悬停提示。 */
	setTooltip(text: string): void

	/** 设置托盘徽标文字（macOS 菜单栏支持）；传 `null` 清除。 */
	setBadge(text: string | null): void
}

/** `$plugin.window.open` 的参数（都是「愿望」：尺寸由宿主夹取）。 */
interface PluginWindowOptions {
	/** 标题；缺省是「<插件名> 窗口」。 */
	title?: string
	/** 想要的宽度（逻辑像素）；宿主夹到 200~8000。 */
	width?: number
	/** 想要的高度；同上。 */
	height?: number
	/** 是否可缩放；缺省 `true`。 */
	resizable?: boolean
	/** 是否置顶；缺省 `false`。 */
	alwaysOnTop?: boolean
	/** 是否显示系统标题栏/边框；缺省 `true`。 */
	decorations?: boolean
	/** 是否透明（页面自带背景时用）；缺省 `false`。 */
	transparent?: boolean
	/** 是否居中；缺省 `false`（交给系统摆放）。 */
	center?: boolean
	/** 页面：相对插件目录的路径；缺省 `index.html`。 */
	page?: string
}

/** `$plugin.window.open` 的返回值。 */
interface PluginWindow {
	/** 插件内唯一的窗口 id（`w1` / `w2`…）；后续 post / onMessage / close 都用它。 */
	id: string
	/** 宿主实际使用的窗口标签（诊断用，插件不需要自己拼）。 */
	label: string
	/** 窗口序号（同一个标签里的自增部分）。 */
	seq: number
}

/** `$plugin.window`：自定义窗口能力。 */
interface ClipBeamPluginWindow {
	/**
	 * 打开一个插件窗口，页面来自插件目录（缺省 `index.html`）。
	 *
	 * 页面跑在一个沙箱 `iframe` 里：它拿不到宿主的 IPC，只能用 `window.onmessage` /
	 * `window.parent.postMessage` 与插件通信（见 `plugin.md` §5.4 的时序图）。
	 */
	open(options?: PluginWindowOptions): PluginWindow

	/** 把一个 JSON 值发给窗口页面（页面用 `window.onmessage` 收）。 */
	post(windowId: string, message: unknown): void

	/** 登记某个窗口的页面消息回调；重复登记会覆盖。 */
	onMessage(windowId: string, callback: (message: unknown) => void): void

	/** 登记某个窗口的关闭回调（用户关掉、你自己关掉、页面异常都会触发）。 */
	onClosed(windowId: string, callback: () => void): void

	/** 关掉一个窗口（幂等：已经不在了也不报错）。 */
	close(windowId: string): void
}

interface ClipBeamPlugin {
	// ── 反馈 ────────────────────────────────────────────────────────────────

	/**
	 * 在应用窗口里弹一条提示（应用内 toast）。
	 *
	 * **不阻塞**：返回即继续，不需要 `await`。
	 */
	toast(message: string, options?: PluginToastOptions): void

	/**
	 * 发一条系统通知（macOS 通知中心 / Windows Toast）。
	 *
	 * 与 `toast` 的区别：系统通知在通知中心里，用户切到别的应用也能看到。
	 * 其它平台静默无效果。
	 */
	notify(title: string, body?: string): void

	/**
	 * 弹一个系统原生提示框（只有一个「好」），等用户点掉。
	 */
	alert(message: string, options?: PluginDialogOptions): Promise<void>

	/**
	 * 弹一个系统原生确认框并等用户回答。
	 *
	 * @returns 主按钮 `true`、次按钮 `false`、关闭 / 超时 / 第三个按钮 `null`。
	 */
	confirm(message: string, options?: PluginDialogOptions): Promise<boolean | null>

	// ── 托盘 ────────────────────────────────────────────────────────────────

	/** 托盘能力（动作回调登记、tooltip、徽标）。 */
	tray: ClipBeamPluginTray

	// ── 窗口 ────────────────────────────────────────────────────────────────

	/** 自定义窗口：开窗、往页面发消息、收页面消息、关窗。 */
	window: ClipBeamPluginWindow
}

/** 插件能力命名空间的全局对象（插件入口脚本里的 `ClipBeamPlugin`）。 */
declare const ClipBeamPlugin: ClipBeamPlugin

/** `$plugin` 是 `ClipBeamPlugin` 的别名，指向同一个对象；写起来更短。 */
declare const $plugin: ClipBeamPlugin
