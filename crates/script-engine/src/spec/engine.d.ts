// src/spec/engine.d.ts —— 引擎补齐的**标准全局**的类型提示（全局声明）
//
// 这个文件是「全局声明」（没有 import/export）：配合 tsconfig 的 include 即可让脚本
// 直接用上 `sleep` / `TextDecoder` / `TextEncoder` / `console` / 定时器 / `atob` … 的类型，
// 既不需要 import，也不需要额外配置。
//
// 分层说明：**引擎不定义能力命名空间**。这里的名字要么来自规范（WHATWG / ECMAScript
// 宿主扩展），要么是引擎自己承诺的标准全局（目前只有 `sleep`）。能力命名空间
// （`MyTool` / `$` / 使用方配置的任何名字）由**使用方**声明与配置，例如
// 使用方能力集 crate 的 `spec/*.d.ts`。
//
// 改标准全局时记得同步这个文件（tests/spec_sync.rs 会校验不漂移）。

// ── 引擎提供的全局 ───────────────────────────────────────────────────────────

/**
 * 异步等待若干毫秒。
 *
 * **必须 `await`**：它返回 Promise，不 await 只是发起等待、不会真的等。
 * 等待期间被中止（用户按中止键）会立即返回，不抛异常。
 */
declare function sleep(ms: number): Promise<void>

// ── 运行时补齐的标准全局 ─────────────────────────────────────────────────────

/** TextDecoder 的构造选项。 */
interface TextDecoderOptions {
	/** true 时遇到非法字节序列抛 TypeError，而不是替换成 U+FFFD。 */
	fatal?: boolean
	/** true 时保留开头的 BOM（默认会按 BOM 嗅探并去掉）。 */
	ignoreBOM?: boolean
}

/**
 * 把字节流解码成字符串。支持全部 WHATWG 编码标签：
 * `utf-8`（默认）、`utf-16le`、`gbk`、`gb18030`、`big5`、`shift_jis`、`euc-kr`…
 *
 * 注意：不支持 `{ stream: true }` 流式解码。
 */
declare class TextDecoder {
	/** 未知标签会抛 RangeError。 */
	constructor(label?: string, options?: TextDecoderOptions)
	/** 规范名（小写），如 `"utf-8"`、`"gbk"`。 */
	readonly encoding: string
	readonly fatal: boolean
	readonly ignoreBOM: boolean
	/** 无参数时返回空串；传入 detached 的 ArrayBuffer 会抛 TypeError。 */
	decode(input?: ArrayBuffer | ArrayBufferView): string
}

/** 只实现 UTF-8 编码的 TextEncoder。 */
declare class TextEncoder {
	constructor()
	readonly encoding: "utf-8"
	/** 返回 UTF-8 字节；孤立代理项按规范替换成 U+FFFD。 */
	encode(input?: string): Uint8Array
}

/** 运行时提供的精简 console：字符串原样输出，其余值走 JSON.stringify。 */
interface ScriptConsole {
	log(...args: unknown[]): void
	info(...args: unknown[]): void
	debug(...args: unknown[]): void
	warn(...args: unknown[]): void
	error(...args: unknown[]): void
}

declare const console: ScriptConsole

/** 把 base64 解码成二进制字符串（WHATWG 语义；非法输入抛 InvalidCharacterError）。 */
declare function atob(data: string): string
/** 把二进制字符串编码成 base64（WHATWG 语义；非 Latin-1 字符抛 InvalidCharacterError）。 */
declare function btoa(data: string): string

/**
 * 延迟执行一次。返回的 id 可以传给 `clearTimeout`。
 *
 * 与浏览器一致：定时器只属于**本次运行**，脚本结束时会被统一清理。
 */
declare function setTimeout(callback: () => void, delayMs?: number): number
/** 每 delayMs 毫秒执行一次，直到 `clearInterval` 或脚本结束。 */
declare function setInterval(callback: () => void, delayMs?: number): number
/** 取消一个 `setTimeout`；id 不存在或已触发时什么都不做。 */
declare function clearTimeout(id?: number): void
/** 取消一个 `setInterval`；id 不存在或已取消时什么都不做。 */
declare function clearInterval(id?: number): void

/** 单调时钟（毫秒，从运行时创建开始计时）。 */
declare const performance: {
	now(): number
}

/**
 * 深拷贝一个值（JSON 语义）。
 *
 * 只支持对象/数组/原始值；遇到 `Date` / `Map` / 函数 / 循环引用会抛出明确的错误
 * （不做静默降级）。
 */
declare function structuredClone<T>(value: T): T
