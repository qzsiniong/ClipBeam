/* eslint-disable no-console -- 未模拟的命令、只有真机能做的命令，都要在 DevTools 里留一行提示 */
/**
 * mock 的命令表：`invoke(cmd, payload)` → 夹具。
 *
 * 加一条命令 = 在 `COMMANDS` 里加一行（不再是一大段 `switch`）。
 * 未知命令只警告不抛错，方便发现"还没模拟哪条"。
 */
import type { InvokeArgs } from '@tauri-apps/api/core'
import { emitEvent, simulateRun } from './events'
import {
  clearConsoleLines,
  consoleLinesSnapshot,
  deleteScript,
  getConfig,
  getScript,
  listCapabilities,
  listScripts,
  MOCK_DIR,
  setConfig,
  setRunning,
  writeScript,
} from './state'

/** 构造"命令失败"的返回值：mock 的 `invoke` 是 async 包装，抛出即 reject。 */
function fail(message: string): never {
  throw new Error(message)
}

/** 参数取值（`InvokeArgs` 可能是数组/二进制，这里只处理对象形态）。 */
function arg(payload: InvokeArgs | undefined, key: string): unknown {
  if (!payload || Array.isArray(payload) || payload instanceof ArrayBuffer || ArrayBuffer.isView(payload))
    return undefined
  return (payload as Record<string, unknown>)[key]
}

/** 与 Rust `hotkey::spec_from_frontend` 同序：Ctrl → Super → Alt → Shift → 键名。 */
function hotkeySpec(code: string, mods: { ctrl?: boolean, shift?: boolean, alt?: boolean, super_?: boolean }): string {
  const parts: string[] = []
  if (mods.ctrl)
    parts.push('Ctrl')
  if (mods.super_)
    parts.push('Super')
  if (mods.alt)
    parts.push('Alt')
  if (mods.shift)
    parts.push('Shift')
  parts.push(code)
  return parts.join('+')
}

/** 只有真机才能做的命令（键盘 / 剪贴板 / 接收页）。 */
function realMachineOnly(cmd: string): null {
  console.info(`[clipbeam-mock] ${cmd}：只有真机才能做（键盘/剪贴板/接收页）`)
  return null
}

/** 命令 → 夹具。 */
const COMMANDS = new Map<string, (payload: InvokeArgs | undefined) => unknown>([
  ['get_config', () => getConfig()],
  ['save_config', (payload) => {
    setConfig({ ...(arg(payload, 'config') as Record<string, unknown>) })
    return null
  }],
  ['get_autostart', () => false],
  ['set_autostart', () => null],
  ['capture_hotkey', (payload) => {
    const code = String(arg(payload, 'code') ?? '')
    if (!code)
      fail('无法识别该组合键')
    return hotkeySpec(code, (arg(payload, 'mods') ?? {}) as Record<string, boolean>)
  }],
  ['list_scripts', () => listScripts()],
  ['read_script', (payload) => {
    const name = String(arg(payload, 'name') ?? '')
    return getScript(name) ?? fail(`读取脚本失败：浏览器 mock 里没有 ${name}`)
  }],
  ['write_script', (payload) => {
    const name = String(arg(payload, 'name') ?? '')
    if (!/^[\w.-]+\.(?:js|ts|mjs|cjs|mts|cts)$/.test(name))
      fail(`非法脚本文件名 ${name}`)
    writeScript(name, String(arg(payload, 'source') ?? ''))
    return null
  }],
  ['delete_script', (payload) => {
    const name = String(arg(payload, 'name') ?? '')
    if (!deleteScript(name))
      fail(`删除脚本失败：浏览器 mock 里没有 ${name}`)
    return null
  }],
  ['scripts_info', () => ({ dir: MOCK_DIR, seeded: 0 })],
  // 名字与别名与 Rust 侧 `clipbeam_scripting::NAMESPACE` / `NAMESPACE_ALIAS` 保持一致
  ['list_capabilities', () => ({ namespace: 'ClipBeam', alias: '$', capabilities: listCapabilities() })],
  ['get_script_console', () => consoleLinesSnapshot()],
  ['clear_script_console', () => {
    clearConsoleLines()
    void emitEvent('script-console-clear')
    return null
  }],
  ['start_script', (payload) => {
    const name = String(arg(payload, 'name') ?? '')
    void simulateRun(name).catch((error: unknown) => console.warn('[clipbeam-mock] 模拟运行失败：', error))
    return null
  }],
  ['cancel_task', () => {
    setRunning(false)
    void emitEvent('worker-cancelled')
    return null
  }],
  // 浏览器里没有真的待命窗口：把请求的状态原样回传，让待命页的暂停/恢复按钮能正常切换
  ['set_standby_paused', payload => Boolean(arg(payload, 'paused'))],
  ['open_scripts_window', () => {
    console.info('[clipbeam-mock] 「打开脚本窗口」在浏览器里对应的做法是另开一个 tab：/?window=scripting#/scripting')
    return null
  }],
  ['open_main_window', () => {
    console.info('[clipbeam-mock] 「回到主窗口」在浏览器里对应的做法是切到 /?window=main#/ 那个 tab')
    return null
  }],
  ['start_send', () => realMachineOnly('start_send')],
  ['start_send_raw', () => realMachineOnly('start_send_raw')],
  ['start_recv', () => realMachineOnly('start_recv')],
  ['start_deploy_type', () => realMachineOnly('start_deploy_type')],
  ['deploy_copy', () => realMachineOnly('deploy_copy')],
])

/**
 * 窗口/对话框插件的默认返回；`undefined` 表示"这不是 window/dialog 插件命令"。
 *
 * 给默认值是为了不让 UI 因为 `undefined` 炸掉：`plugin:window|*` 是 no-op（`show`/`hide`…），
 * `plugin:dialog|ask` 恒 `false`（等价"用户取消"，避免 mock 下误触发删除/写文件这类流程）。
 */
function pluginDefault(cmd: string, payload: InvokeArgs | undefined): unknown {
  if (cmd.startsWith('plugin:window|')) {
    switch (cmd) {
      case 'plugin:window|is_always_on_top':
        return false
      case 'plugin:window|is_visible':
      case 'plugin:window|is_focused':
        return true
      case 'plugin:window|inner_size':
        return { width: 420, height: 80 }
      case 'plugin:window|scale_factor':
        return 1
      case 'plugin:window|current_monitor':
        return null
      default:
        return null
    }
  }
  if (cmd.startsWith('plugin:dialog|'))
    return cmd === 'plugin:dialog|ask' ? false : null
  // 侧边栏用 Tauri 的 getVersion() 显示版本；浏览器里读不到 tauri.conf，
  // 回一个**明确标注是 mock** 的串，免得看起来像真实版本号
  if (cmd === 'plugin:app|version')
    return '0.1.0-mock'
  if (cmd.startsWith('plugin:')) {
    console.warn('[clipbeam-mock] 未模拟的插件命令：', cmd, payload)
    return null
  }
  return undefined
}

/** mock 的 `invoke` 入口。 */
export function handleCommand(cmd: string, payload?: InvokeArgs): unknown {
  const fromPlugin = pluginDefault(cmd, payload)
  if (fromPlugin !== undefined)
    return fromPlugin

  const handler = COMMANDS.get(cmd)
  if (handler)
    return handler(payload)

  console.warn('[clipbeam-mock] 未模拟的命令：', cmd, payload)
  return null
}
