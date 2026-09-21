import type { MockCapability, MockConsoleLine, MockScriptMeta } from './types'
/**
 * mock 的"假后端状态"：初始夹具 + 内存状态 + 访问器。
 *
 * 只有这里能改 mock 的状态；`events.ts` / `commands.ts` 都通过下面导出的访问器读写，
 * 免得状态散到各处。**纯状态**：这里不发事件（发事件在 `events.ts`）。
 */
import { isMac } from '@/lib/clipbeam-script/shortcuts'
import quickStart from '../../../crates/clipbeam-scripting/seed/01-quick-start.js?raw'
import tsDemo from '../../../crates/clipbeam-scripting/seed/02-ts-demo.ts?raw'

/** 造一堆日志，用来看 Console 面板的滚动 / 过滤 / 最大化。 */
const CONSOLE_DEMO = `// 造一堆日志：用来看 Console 面板的自动跟随、等级过滤与最大化
for (let i = 0; i < 60; i++) {
  console.log('第 ' + (i + 1) + ' 行日志（浏览器 mock）')
  if (i % 7 === 0) console.warn('警告示例 ' + i)
  if (i % 11 === 0) console.error('错误示例 ' + i)
}
`

/** mock 的"脚本目录"（页面会把它显示给用户，标注清楚这是假的）。 */
export const MOCK_DIR = '/mock/scripts（浏览器 mock）'

/** 内存里的"脚本目录"：两个真种子（源码来自 crate，保证与内置示例同步）+ 一个造日志的。 */
const scripts = new Map<string, string>([
  ['01-quick-start.js', quickStart],
  ['02-ts-demo.ts', tsDemo],
  ['03-console-demo.js', CONSOLE_DEMO],
])

/** 与 Rust `Config::default()` 对齐（快捷键按平台取 mac / 非 mac 的默认值）。 */
let config: Record<string, unknown> = {
  send_raw_hotkey: isMac() ? 'Cmd+Shift+L' : 'Ctrl+Shift+L',
  send_hotkey: isMac() ? 'Cmd+Shift+K' : 'Ctrl+Shift+K',
  recv_hotkey: isMac() ? 'Cmd+Shift+J' : 'Ctrl+Shift+J',
  stop_hotkey: 'Esc',
  key_delay_ms: 3,
  settle_ms: 200,
  receive_timeout_s: 120,
  max_text_kb: 256,
  compress: true,
  progress_display: 'floating',
  send_real_keys: true,
}

let consoleLines: MockConsoleLine[] = []
let consoleSeq = 0
/** 是否有任务"在运行"（`simulateRun` / `cancel_task` 改写它）。 */
let running = false

/**
 * 能力清单的 **mock 子集**（真值只有 Rust 侧有）。
 *
 * 只影响 `$.` 补全在语言服务未就绪时的兜底；编辑器的主要补全/悬停来自两份 `.d.ts`，
 * 在浏览器里是真实生效的。
 */
const CAPABILITIES: MockCapability[] = [
  ['bytes', 'bytes(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer', '统一成字节'],
  ['str', 'str(data: string | ArrayBuffer | ArrayBufferView, encoding?: string) -> string', '字节 → 文本'],
  ['chunks', 'chunks(data: string, chunkSize?: number) -> string[]; chunks(data: ArrayBuffer | ArrayBufferView, chunkSize?: number) -> ArrayBuffer[]', '按大小分片（字符串按码点、二进制按字节）'],
  ['hex', 'hex(data: string | ArrayBuffer | ArrayBufferView) -> string', '十六进制编码，小写'],
  ['base32_nopad', 'base32_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string', 'RFC4648 Base32（大写无填充）'],
  ['base64_url_nopad', 'base64_url_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string', 'URL-safe Base64（无填充）'],
  ['md5', 'md5(data: string | ArrayBuffer | ArrayBufferView) -> string', '32 位小写十六进制 MD5'],
  ['crc32', 'crc32(data: string | ArrayBuffer | ArrayBufferView, format?: string) -> string', 'CRC-32 校验和'],
  ['zstd', 'zstd(data: string | ArrayBuffer | ArrayBufferView, level?: number) -> ArrayBuffer', 'Zstandard 压缩'],
  ['read_text', 'read_text(path: string, encoding?: string) -> Promise<string>', '读取文件并解码'],
  ['write_text', 'write_text(path: string, text: string) -> Promise<void>', '写入文本（会先询问）'],
  ['exists', 'exists(path: string) -> Promise<boolean>', '路径是否存在'],
  ['type_str', 'type_str(text: string, delayMs?: number) -> void', '把文本交给宿主输出'],
  ['confirm', 'confirm(message: string) -> Promise<boolean>', '向用户提问'],
].map(([name, signature, doc]) => ({ name, signature, doc, source: 'clipbeam' }))

/** 脚本清单（按名字排序，路径前缀是 mock 目录）。 */
export function listScripts(): MockScriptMeta[] {
  return [...scripts.keys()].sort().map(name => ({
    name,
    path: `${MOCK_DIR}/${name}`,
    language: name.endsWith('.ts') ? 'ts' : 'js',
  }))
}

/** 第一个脚本名（按 Map 插入顺序，`runScript()` 不传名字时用它）；空目录返回 `''`。 */
export function firstScriptName(): string {
  return [...scripts.keys()][0] ?? ''
}

/** 读脚本源码；不存在返回 `undefined`（报错文案由调用方决定）。 */
export function getScript(name: string): string | undefined {
  return scripts.get(name)
}

/** 写入/覆盖脚本。 */
export function writeScript(name: string, source: string): void {
  scripts.set(name, source)
}

/** 删除脚本，返回是否真的删掉了。 */
export function deleteScript(name: string): boolean {
  return scripts.delete(name)
}

/** 配置（副本：调用方改了不影响 mock 状态）。 */
export function getConfig(): Record<string, unknown> {
  return { ...config }
}

/** 整份替换配置（对应 `save_config`）。 */
export function setConfig(next: Record<string, unknown>): void {
  config = { ...next }
}

/** 能力清单（mock 子集）。 */
export function listCapabilities(): MockCapability[] {
  return CAPABILITIES
}

/** 追加一行 Console 并返回它（seq 自增、最多留 1000 行，与 Rust 侧缓冲一致）。 */
export function appendConsoleLine(level: string, text: string): MockConsoleLine {
  const line: MockConsoleLine = { seq: ++consoleSeq, level, text, ts: Date.now() }
  consoleLines.push(line)
  if (consoleLines.length > 1000)
    consoleLines = consoleLines.slice(-1000)
  return line
}

/** Console 缓冲快照（副本）。 */
export function consoleLinesSnapshot(): MockConsoleLine[] {
  return [...consoleLines]
}

/** 清空 Console 缓冲。 */
export function clearConsoleLines(): void {
  consoleLines = []
}

/** 是否有任务在运行。 */
export function isRunning(): boolean {
  return running
}

/** 改写"运行中"标志。 */
export function setRunning(value: boolean): void {
  running = value
}
