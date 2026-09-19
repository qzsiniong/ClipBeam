// src/spec/clipbeam.d.ts —— ClipBeam 扩展能力的类型提示（全局声明）
//
// 这个文件是「全局声明」（没有 import/export），与 clipbeam-script 的同名文件一起
// 通过 tsconfig 的 include 生效。
//
// 这里**只声明 clipbeam-scripting 注入的能力**；core 的 `file` / `sleep` 与
// `TextDecoder` / `TextEncoder` / `console` 在 clipbeam-script 的 spec 里声明。
// TypeScript 的接口声明合并会把两份 `Clipbeam` 合成一个。
//
// 改扩展能力时记得同步这个文件（tests/capabilities.rs 会校验不漂移）。

interface Clipbeam {
	/** 32 位小写十六进制 MD5 摘要。 */
	md5(data: ArrayBuffer): string

	/** RFC4648 Base32 编码：无 `=` 填充、输出小写。 */
	base32(data: ArrayBuffer): string

	/**
	 * Zstandard 压缩（级别 3）并按 `chunkSize` 切片。
	 *
	 * @param chunkSize 每个分片的最大字节数，省略时为 1024；传 0 或负数按 1 处理。
	 */
	zstd(data: ArrayBuffer, chunkSize?: number): ArrayBuffer[]

	/**
	 * 把文本交给宿主输出。
	 *
	 * GUI 下是逐键打进**当前焦点窗口**，因此运行前会先显示待命窗口；
	 * 命令行下是逐字打印到终端。被中止时抛出异常终止脚本。
	 *
	 * @param delayMs 每个字符之间的延迟（毫秒），省略为 0。
	 */
	typeStr(text: string, delayMs?: number): void

	/**
	 * 向用户提问并等待回答。
	 *
	 * @returns 回答「是」为 `true`；回答「否」或等待超时为 `false`；用户选择中止时抛异常。
	 */
	confirm(message: string): Promise<boolean>
}
