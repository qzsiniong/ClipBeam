// src/spec/clipbeam.d.ts —— ClipBeam 扩展能力的类型提示（全局声明）
//
// 这个文件是「全局声明」（没有 import/export），与 script-engine 的同名文件一起
// 通过 tsconfig 的 include 生效。
//
// 这里声明的是**本 crate 决定的东西**：能力命名空间的名字（`ClipBeam` / `$`）与挂在它上面的
// 全部能力 —— 引擎不定义任何能力，引擎补齐的标准全局（`sleep` / `TextDecoder` / 定时器 /
// `atob` …）声明在 script-engine 的同名文件里。
//
// 名字的真源是 `clipbeam_scripting::NAMESPACE` / `NAMESPACE_ALIAS`（配置给引擎、下发给前端
// 用的是同一对常量）；这里是**类型层面**的对应声明 —— TypeScript 取不到运行期字符串，
// 所以名字必须在声明文件里再写一次。
//
// 改扩展能力时记得同步这个文件（tests/capabilities.rs 会校验不漂移）。

/** 二进制入参：字符串按 UTF-8 编码，`ArrayBuffer` 与任何视图都按字节处理。 */
type BinaryInput = string | ArrayBuffer | ArrayBufferView

/** `$.stat` 的结果。 */
interface ClipbeamStat {
	/** 路径（原样回显解析后的绝对路径）。 */
	path: string
	/** 字节数。 */
	size: number
	/** 是否是普通文件。 */
	isFile: boolean
	/** 是否是目录。 */
	isDir: boolean
	/** 最后修改时间（epoch 毫秒）；拿不到时为 0。 */
	modifiedMs: number
}

interface ClipBeam {
	// ── 数据形态 ────────────────────────────────────────────────────────────

	/** 统一成字节：字符串按 UTF-8 编码，`ArrayBuffer`/视图原样拷贝。 */
	bytes(data: BinaryInput): ArrayBuffer

	/**
	 * 把字节按 `encoding` 解码成文本。
	 *
	 * 支持全部 WHATWG 编码标签（`utf-8` 默认、`gbk`、`gb18030`、`big5`、
	 * `shift_jis`…）；传入字符串时原样返回。
	 */
	str(data: BinaryInput, encoding?: string): string

	/**
	 * 按 `chunkSize` 分片（默认 1024），返回互相独立的分片，拼接回去与原文一致。
	 *
	 * - 字符串入参：按 **Unicode 码点**切，返回 `string[]`（不会把 emoji 的代理对切半）。
	 * - 二进制入参：按**字节**切，返回 `ArrayBuffer[]`。
	 *
	 * 压缩与切片是分开的：`$.zstd` 只压缩，要分片再调 `$.chunks`。
	 */
	chunks<T extends BinaryInput>(data: T, chunkSize?: number): T extends string ? string[] : ArrayBuffer[]

	// ── 编码 ────────────────────────────────────────────────────────────────

	/** 十六进制编码，小写。 */
	hex(data: BinaryInput): string
	/** 十六进制编码，大写。 */
	hex_upper(data: BinaryInput): string
	/** 十六进制解码（大小写均可）。 */
	hex_decode(text: string): ArrayBuffer

	/** RFC4648 Base32：大写、带 `=` 填充。 */
	base32(data: BinaryInput): string
	/** RFC4648 Base32：大写、无填充。 */
	base32_nopad(data: BinaryInput): string
	/** RFC4648 Base32：小写、带 `=` 填充。 */
	base32_lower(data: BinaryInput): string
	/** RFC4648 Base32：小写、无填充。 */
	base32_lower_nopad(data: BinaryInput): string
	/** Base32 解码：要求大写且带填充（与 `base32` 对应）。 */
	base32_decode(text: string): ArrayBuffer
	/** Base32 解码：要求大写且无填充（与 `base32_nopad` 对应）。 */
	base32_nopad_decode(text: string): ArrayBuffer
	/** Base32 解码：要求小写且带填充（与 `base32_lower` 对应）。 */
	base32_lower_decode(text: string): ArrayBuffer
	/** Base32 解码：要求小写且无填充（与 `base32_lower_nopad` 对应）。 */
	base32_lower_nopad_decode(text: string): ArrayBuffer

	/** 标准 Base64：带 `=` 填充。 */
	base64(data: BinaryInput): string
	/** 标准 Base64：不带填充。 */
	base64_nopad(data: BinaryInput): string
	/** URL-safe Base64（`-` `_`）：带 `=` 填充。 */
	base64_url(data: BinaryInput): string
	/** URL-safe Base64（`-` `_`）：不带填充。 */
	base64_url_nopad(data: BinaryInput): string
	/** 标准 Base64 解码：要求带填充（与 `base64` 对应）。 */
	base64_decode(text: string): ArrayBuffer
	/** 标准 Base64 解码：要求无填充（与 `base64_nopad` 对应）。 */
	base64_nopad_decode(text: string): ArrayBuffer
	/** URL-safe Base64 解码：要求带填充（与 `base64_url` 对应）。 */
	base64_url_decode(text: string): ArrayBuffer
	/** URL-safe Base64 解码：要求无填充（与 `base64_url_nopad` 对应）。 */
	base64_url_nopad_decode(text: string): ArrayBuffer

	// ── 摘要 ────────────────────────────────────────────────────────────────

	/** 32 位小写十六进制 MD5 摘要。 */
	md5(data: BinaryInput): string

	/**
	 * CRC-32 校验和（大端字节序）。
	 *
	 * @param format `hex`（默认，8 位大写）/ `hex_lower` / `base32`（7 位大写）/
	 *   `base32_lower` / `base64`（6 位无填充）
	 */
	crc32(
		data: BinaryInput,
		format?: "hex" | "hex_lower" | "base32" | "base32_lower" | "base64",
	): string

	// ── 压缩 ────────────────────────────────────────────────────────────────

	/** Zstandard 压缩（默认级别 3）。只压缩不分片。 */
	zstd(data: BinaryInput, level?: number): ArrayBuffer
	/** gzip 压缩（默认级别 6，范围 0-9）。 */
	gzip(data: BinaryInput, level?: number): ArrayBuffer
	/** gzip 解压。 */
	gunzip(data: BinaryInput): ArrayBuffer
	/** Brotli 压缩（默认质量 5，范围 0-11）。 */
	brotli(data: BinaryInput, level?: number): ArrayBuffer
	/** Brotli 解压。 */
	unbrotli(data: BinaryInput): ArrayBuffer
	/** xz（LZMA2）压缩，产物兼容 `xz` / `7z` / `tar -J`。 */
	lzma(data: BinaryInput): ArrayBuffer
	/** xz 解压。 */
	unlzma(data: BinaryInput): ArrayBuffer

	// ── 文件系统（路径必须是绝对路径：`~`、`C:/x`、`/d/x`、`/cygdrive/d/x` 都认）──

	/** 读取文件全部字节。 */
	read(path: string): Promise<ArrayBuffer>
	/** 读取文件并按 `encoding` 解码（默认 utf-8）。 */
	read_text(path: string, encoding?: string): Promise<string>
	/** 路径是否存在（文件、目录、符号链接都算）。 */
	exists(path: string): Promise<boolean>
	/** 文件元信息（大小 / 类型 / 修改时间）。 */
	stat(path: string): Promise<ClipbeamStat>
	/** 列出目录下的条目名（已排序）。 */
	list(path: string): Promise<string[]>
	/** 覆盖写入文件（会先询问用户；父目录必须已存在）。 */
	write(path: string, data: BinaryInput): Promise<void>
	/** 以 UTF-8 覆盖写入文本（会先询问用户）。 */
	write_text(path: string, text: string): Promise<void>
	/** 追加写入（会先询问用户；文件不存在时创建）。 */
	append(path: string, data: BinaryInput): Promise<void>
	/** 以 UTF-8 追加写入文本（会先询问用户）。 */
	append_text(path: string, text: string): Promise<void>
	/** 递归创建目录（会先询问用户）。 */
	mkdir(path: string): Promise<void>
	/** 删除文件或目录（目录递归删除；会先询问用户）。 */
	remove(path: string): Promise<void>
	/** 移动 / 改名（会先询问用户）。 */
	rename(from: string, to: string): Promise<void>
	/** 复制文件或目录（目录递归；会先询问用户）。 */
	copy(from: string, to: string): Promise<void>

	// ── 宿主交互 ────────────────────────────────────────────────────────────

	/**
	 * 把文本交给宿主输出。
	 *
	 * GUI 下是逐键打进**当前焦点窗口**，因此运行前会先显示待命窗口；
	 * 命令行下是逐字打印到终端。被中止时抛出异常终止脚本。
	 *
	 * @param delayMs 每个字符之间的延迟（毫秒），省略为 0。
	 */
	type_str(text: string, delayMs?: number): void

	/**
	 * 向用户提问并等待回答。
	 *
	 * @returns 回答「是」为 `true`；「否」为 `false`；用户选择中止时抛异常。
	 */
	confirm(message: string): Promise<boolean>
}

/** 能力命名空间的全局对象（脚本里的 `ClipBeam`）。 */
declare const ClipBeam: ClipBeam

/** `$` 是 `ClipBeam` 的别名，指向同一个对象；写起来更短。 */
declare const $: ClipBeam
