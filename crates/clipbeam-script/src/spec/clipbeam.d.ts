// src/spec/clipbeam.d.ts —— core 内置能力的类型提示（全局声明）
//
// 这个文件是「全局声明」（没有 import/export）：配合 tsconfig 的 include 即可让
// 脚本直接用上 `Clipbeam` / `$` / `TextDecoder` / `TextEncoder` / `console` 的类型，
// 既不需要 import，也不需要额外配置。
//
// 分层说明：本文件只声明 **core** 提供的能力（`file` / `sleep`）与补齐的标准 API。
// 使用方的业务能力（md5 / zstd / 键盘输出 …）由各自 crate 的 .d.ts 通过
// TypeScript 的接口声明合并（interface declaration merging）追加到 `Clipbeam` 上。
//
// 改 core 能力时记得同步这个文件（tests/spec_sync.rs 会校验不漂移）。

/**
 * 脚本里的能力命名空间。
 *
 * 用 `interface` 而不是对象字面量类型，是为了让使用方（例如 ClipBeam 的
 * clipbeam-scripting）能用**接口声明合并**追加自己的能力：
 *
 * ```ts
 * interface Clipbeam { md5(data: ArrayBuffer): string }
 * ```
 */
interface Clipbeam {
	/**
	 * 读取真实文件，返回文件原始字节。
	 *
	 * 相对路径按**进程工作目录**解析；失败（不存在 / 是目录 / 无权限）时
	 * 返回的 Promise 被拒绝，可以用 try/catch 捕获。
	 */
	file(path: string): Promise<ArrayBuffer>

	/**
	 * 异步等待若干毫秒。
	 *
	 * 等待期间被中止（用户按中止键）会立即返回，不抛异常。
	 */
	sleep(ms: number): Promise<void>
}

/** 能力命名空间的全局对象（core 的 `file` / `sleep`）。 */
declare const Clipbeam: Clipbeam

/** `$` 是 `Clipbeam` 的别名，指向同一个对象。 */
declare const $: Clipbeam

// ── 运行时补齐的标准全局（由 src/prelude.js 提供）───────────────────────────

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
interface ClipbeamConsole {
	log(...args: unknown[]): void
	info(...args: unknown[]): void
	debug(...args: unknown[]): void
	warn(...args: unknown[]): void
	error(...args: unknown[]): void
}

declare const console: ClipbeamConsole
