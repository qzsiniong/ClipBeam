// 脚本编辑器的**快捷键真源**：绑定用的键位串与展示用的文案都从这里取。
//
// # 为什么要有这个文件
//
// 快捷键有两个消费者：
//   1. `clipbeam-editor.ts` 把 `RUN_KEY` / `SAVE_KEY` 绑到编辑器上；
//   2. `ShortcutHelp.vue` 把这份清单画给用户看。
// 两者共用同一份键位串（并由单测钉住一致），所以不会出现
// 「面板写着 ⌘↵、编辑器其实绑的是别的键」。
//
// # 这份清单镜像了哪些键位表
//
// 前两组是本 crate 自己绑的；其余来自 `clipbeam-editor.ts` 里组装的 CodeMirror 键位表：
//
// | 来源 | 键位 |
// |---|---|
// | 本项目（`Prec.highest`） | `Mod-Enter` 运行、`Mod-s` 保存 |
// | `@codemirror/commands` `historyKeymap` | `Mod-z` 撤销、`Mod-y`（mac `Mod-Shift-z`）重做 |
// | `@codemirror/commands` `defaultKeymap` | `Mod-/` 注释、`Mod-[`/`Mod-]` 缩进、`Alt-ArrowUp/Down` 整行移动、`Shift-Mod-k` 删除整行、`Mod-a` 全选 |
// | `@codemirror/search` `searchKeymap` | `Mod-f` / `Mod-g` / `Mod-d` / `Mod-Alt-g` |
// | `@codemirror/autocomplete` `completionKeymap` | `Ctrl-Space` 触发、`Enter` 接受、`Escape` 关闭 |
//
// 注意一个已知的**遮蔽**：`defaultKeymap` 里 `Mod-Enter` 原本是 `insertBlankLine`，
// 被我们的运行键（`Prec.highest`）顶掉了 —— 面板不展示这条被遮蔽的键。
//
// 升级 CodeMirror 大版本时，请对照 `node_modules` 里的 keymap 源码复核本文件。

/** 运行脚本的键位（与编辑器绑定共用）。 */
export const RUN_KEY = 'Mod-Enter'

/** 保存脚本的键位（与编辑器绑定共用）。 */
export const SAVE_KEY = 'Mod-s'

/** 一条快捷键：`keys` 是 CodeMirror 键位串，`macKeys` 用于 macOS 上不同的写法。 */
export interface ShortcutEntry {
  /** CodeMirror 风格的键位串，例如 `Mod-Enter`、`Alt-ArrowUp`。 */
  keys: string
  /** macOS 上的替代键位串（省略时直接用 `keys`）。 */
  macKeys?: string
  /** 面向用户的动作说明。 */
  label: string
}

/** 一组快捷键（面板里一个小标题 + 若干行）。 */
export interface ShortcutGroup {
  title: string
  entries: ShortcutEntry[]
}

/** 面板展示的全部快捷键（有意只列常用的，不是全量键位表）。 */
export const EDITOR_SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    title: '运行与保存',
    entries: [
      { keys: RUN_KEY, label: '运行脚本' },
      { keys: SAVE_KEY, label: '保存脚本' },
    ],
  },
  {
    title: '编辑',
    entries: [
      { keys: 'Mod-z', label: '撤销' },
      { keys: 'Mod-y', macKeys: 'Mod-Shift-z', label: '重做' },
      { keys: 'Mod-/', label: '注释 / 取消注释' },
      { keys: 'Mod-]', label: '增加缩进' },
      { keys: 'Mod-[', label: '减少缩进' },
      { keys: 'Alt-ArrowUp', label: '整行上移' },
      { keys: 'Alt-ArrowDown', label: '整行下移' },
      { keys: 'Shift-Mod-k', label: '删除整行' },
      { keys: 'Mod-a', label: '全选' },
    ],
  },
  {
    title: '查找',
    entries: [
      { keys: 'Mod-f', label: '查找' },
      { keys: 'Mod-g', label: '下一个匹配' },
      { keys: 'Mod-Shift-g', label: '上一个匹配' },
      { keys: 'Mod-d', label: '选中下一个相同词' },
      { keys: 'Mod-Alt-g', label: '跳到行' },
    ],
  },
  {
    title: '补全与提示',
    entries: [
      // macOS 上 Ctrl-Space 常被输入法占用，CodeMirror 另外绑了 Alt-`，面板一并给出
      { keys: 'Ctrl-Space', macKeys: 'Alt-`', label: '触发补全' },
      { keys: 'Enter', label: '接受补全候选' },
      { keys: 'Escape', label: '关闭补全 / 查找框' },
    ],
  },
]

/** 非 mac 平台（Windows / Linux）上 CodeMirror 的 `Mod` 是 Ctrl。 */
const MAC_MOD = '⌘'
const OTHER_MOD = 'Ctrl'

/** 单独出现的修饰键与常用命名键 → 展示文案。 */
const MAC_NAMED_KEYS: Record<string, string> = {
  Shift: '⇧',
  Alt: '⌥',
  Ctrl: '⌃',
  Enter: '⏎',
  Backspace: '⌫',
  Delete: '⌦',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
  Escape: 'Esc',
  Space: 'Space',
  Tab: '⇥',
}

const OTHER_NAMED_KEYS: Record<string, string> = {
  // 方向键/回退这类符号两边都能看懂，保留；修饰键必须换回文字，
  // 否则 Windows/Linux 上会出现 `⇧+Ctrl+K` 这种混搭
  ...MAC_NAMED_KEYS,
  Shift: 'Shift',
  Alt: 'Alt',
  Ctrl: 'Ctrl',
  Enter: 'Enter',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Tab: 'Tab',
}

/**
 * 当前是不是 macOS。
 *
 * 默认从浏览器环境读：`userAgentData.platform`（新 API）→ `navigator.platform` → UA。
 * 参数可注入，便于单测；**读不到一律按非 mac 处理**（回退显示 Ctrl），不会抛错。
 */
export function isMac(platform?: string): boolean {
  let value = platform
  if (value === undefined) {
    try {
      const nav = globalThis.navigator as
        | (Navigator & { userAgentData?: { platform?: string } })
        | undefined
      value = nav?.userAgentData?.platform ?? nav?.platform ?? nav?.userAgent ?? ''
    }
    catch {
      value = ''
    }
  }
  return /mac/i.test(value ?? '')
}

/**
 * 把一条快捷键格式化成展示文案。
 *
 * - macOS：`⌘⇧Z`、`⌥↑`、`⌃Space`（多键位不加分隔符）
 * - 其它平台：`Ctrl+Shift+Z`、`Alt+↑`、`Ctrl+Space`
 */
export function formatShortcut(entry: ShortcutEntry, mac: boolean): string {
  const spec = mac && entry.macKeys ? entry.macKeys : entry.keys
  const mod = mac ? MAC_MOD : OTHER_MOD
  const named = mac ? MAC_NAMED_KEYS : OTHER_NAMED_KEYS

  const parts = spec.split('-').map((part) => {
    if (part === 'Mod')
      return mod
    if (part === 'Cmd' || part === 'Meta')
      return mac ? MAC_MOD : 'Super'
    if (named[part])
      return named[part]
    // 单字母统一大写，方便和其它键位对齐
    return part.length === 1 ? part.toUpperCase() : part
  })

  return mac ? parts.join('') : parts.join('+')
}
