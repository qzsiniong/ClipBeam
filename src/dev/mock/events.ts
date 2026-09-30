import type { MockConsoleLine, MockOutcome } from './types'
/**
 * mock 的事件编排：Console 行 + 后端的几个 worker-* 事件。
 *
 * mock 下 `emit` 走官方 `mockIPC(..., { shouldMockEvents: true })` 的本地环路，
 * 所以这里造的事件，页面上的 `listen` 真的会收到 —— UI（状态徽标、进度、Console）能完整演练。
 */
import { emit } from '@tauri-apps/api/event'
import { appendConsoleLine, getScript, isRunning, setRunning } from './state'

/** 触发一个后端事件（本机 `listen` 立刻收到）。 */
export function emitEvent(event: string, payload?: unknown): Promise<void> {
  return emit(event, payload)
}

/** 往 Console 缓冲里塞一行并推事件（与 Rust 侧 `ConsoleBuffer::push` 的行为对齐）。 */
export function pushConsole(level: string, text: string) {
  void emitEvent('script-console', appendConsoleLine(level, text))
}

/**
 * 模拟一次脚本运行：`worker-started` → 几行 Console → `worker-finished`。
 *
 * 浏览器里没有真的 worker，但事件与 Console 行都是真的（走 mock 的事件环路），
 * 所以运行状态、进度条、Console 面板这些 UI 都能演练。
 */
export async function simulateRun(name: string) {
  if (isRunning())
    throw new Error('已有任务在运行')
  setRunning(true)
  const startedAt = Date.now()
  await emitEvent('worker-started', 'script')
  await emitEvent('worker-progress', {
    kind: 'script',
    got: 0,
    total: 3,
    started_at: startedAt,
    ts: Date.now(),
    snippet: '',
  })
  pushConsole('info', `▶ 运行 ${name}（浏览器 mock，没有真的执行脚本）`)
  pushConsole('log', `脚本源码 ${getScript(name)?.length ?? 0} 字符`)
  await emitEvent('worker-progress', {
    kind: 'script',
    got: 3,
    total: 3,
    started_at: startedAt,
    ts: Date.now(),
    snippet: name,
  })
  pushConsole('info', '✓ 完成（浏览器 mock）')
  setRunning(false)
  const outcome: MockOutcome = { title: '✓ 脚本执行完成（mock）', body: '浏览器里只演练事件与 Console，不执行真实脚本' }
  await emitEvent('worker-finished', outcome)
}

// ── 插件（日志与 toast）────────────────────────────────────────────────────

/**
 * mock 的插件日志：`插件 id → 行`。
 *
 * 真机是「每个插件一个 ConsoleBuffer」（见 `src-tauri/src/plugin_manager.rs`）；
 * 这里用一张表模拟，事件名与脚本共用（`script-console`），但载荷多带 `plugin_id`。
 */
const PLUGIN_LOGS = new Map<string, MockConsoleLine[]>()
let pluginLogSeq = 1

/** 取一个插件的日志快照。 */
export function pluginConsoleLines(id?: string): MockConsoleLine[] {
  if (id)
    return [...(PLUGIN_LOGS.get(id) ?? [])]
  return [...PLUGIN_LOGS.values()].flat()
}

/** 清空插件日志。 */
export function clearPluginConsole(): void {
  PLUGIN_LOGS.clear()
}

/** 往某个插件的日志里塞一行，并推事件（界面上立刻可见）。 */
export function pushPluginConsole(pluginId: string, level: string, text: string): void {
  const line: MockConsoleLine = { seq: pluginLogSeq++, level, text, ts: Date.now() }
  const lines = PLUGIN_LOGS.get(pluginId) ?? []
  lines.push(line)
  PLUGIN_LOGS.set(pluginId, lines.slice(-500))
  void emitEvent('script-console', { plugin_id: pluginId, line })
}

/** 造一条插件 toast（`$plugin.toast` 落到界面的那条路）。 */
export async function emitPluginToast(
  pluginId = 'hello-plugin',
  pluginName = '示例插件',
  message = '来自插件的提示 👋',
  level = 'info',
): Promise<void> {
  await emitEvent('plugin-toast', {
    plugin_id: pluginId,
    plugin_name: pluginName,
    level,
    message,
    duration_ms: null,
  })
}
