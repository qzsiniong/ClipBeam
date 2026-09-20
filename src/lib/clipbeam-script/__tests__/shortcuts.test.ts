// 快捷键真源的单元测试（纯 TS，不需要 DOM）。
//
// 盯住三件事：
//   1. 平台判断：读不到 / 非 mac 一律回退 Ctrl，不会因为环境缺 navigator 就炸；
//   2. 展示格式化：mac 用符号且不加分隔符，其它平台用 Ctrl/Shift/Alt + `+`；
//   3. **防漂移**：编辑器真绑的键（`editorKeymap`）与面板宣称的键是同一份。

import { describe, expect, it, vi } from 'vitest'
import { editorKeymap } from '../clipbeam-editor'
import { EDITOR_SHORTCUT_GROUPS, formatShortcut, isMac, RUN_KEY, SAVE_KEY } from '../shortcuts'

/** 面板里「运行与保存」那一组（与编辑器绑定对照用）。 */
function runSaveGroup() {
  const group = EDITOR_SHORTCUT_GROUPS.find(item => item.title === '运行与保存')
  if (!group)
    throw new Error('快捷键清单里应当有「运行与保存」分组')
  return group
}

describe('isMac（平台判断）', () => {
  it('识别 macOS 的各种写法', () => {
    expect(isMac('MacIntel')).toBe(true)
    expect(isMac('macOS')).toBe(true)
    expect(isMac('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)')).toBe(true)
  })

  it('其它平台与非预期输入都回退为非 mac', () => {
    expect(isMac('Win32')).toBe(false)
    expect(isMac('Linux x86_64')).toBe(false)
    expect(isMac('')).toBe(false)
  })

  it('读不到 navigator 时不抛错，按非 mac 处理', () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator')
    try {
      // node 环境下 navigator 可能不存在；显式删掉再调用
      Object.defineProperty(globalThis, 'navigator', { value: undefined, configurable: true })
      expect(isMac()).toBe(false)
    }
    finally {
      if (original)
        Object.defineProperty(globalThis, 'navigator', original)
    }
  })
})

describe('formatShortcut（平台化展示）', () => {
  const run = { keys: RUN_KEY, label: '运行脚本' }
  const save = { keys: SAVE_KEY, label: '保存脚本' }
  const redo = { keys: 'Mod-y', macKeys: 'Mod-Shift-z', label: '重做' }
  const complete = { keys: 'Ctrl-Space', macKeys: 'Alt-`', label: '触发补全' }
  const moveLine = { keys: 'Alt-ArrowUp', label: '整行上移' }

  it('macOS：符号化、多键位不加分隔符', () => {
    expect(formatShortcut(run, true)).toBe('⌘⏎')
    expect(formatShortcut(save, true)).toBe('⌘S')
    expect(formatShortcut(redo, true)).toBe('⌘⇧Z')
    expect(formatShortcut(complete, true)).toBe('⌥`')
    expect(formatShortcut(moveLine, true)).toBe('⌥↑')
  })

  it('非 mac 平台：Ctrl、Shift、Alt 用 + 连接', () => {
    expect(formatShortcut(run, false)).toBe('Ctrl+Enter')
    expect(formatShortcut(save, false)).toBe('Ctrl+S')
    expect(formatShortcut(redo, false)).toBe('Ctrl+Y')
    expect(formatShortcut(complete, false)).toBe('Ctrl+Space')
    expect(formatShortcut(moveLine, false)).toBe('Alt+↑')
  })

  it('macKeys 只影响 mac；命名键与非 mac 的 Enter 文案正确', () => {
    expect(formatShortcut({ keys: 'Escape', label: '关闭' }, true)).toBe('Esc')
    expect(formatShortcut({ keys: 'Escape', label: '关闭' }, false)).toBe('Esc')
    expect(formatShortcut({ keys: 'Shift-Mod-k', label: '删除整行' }, false)).toBe('Shift+Ctrl+K')
  })

  it('清单里每一条都能格式化出可读文案（防键位串写错）', () => {
    for (const group of EDITOR_SHORTCUT_GROUPS) {
      for (const entry of group.entries) {
        for (const mac of [true, false]) {
          const text = formatShortcut(entry, mac)
          expect(text, `${group.title} / ${entry.label}（${entry.keys}）`).not.toContain('undefined')
          expect(text.length, `${group.title} / ${entry.label}`).toBeGreaterThan(0)
        }
      }
    }
  })

  it('清单里没有重复的键位串（防复制粘贴漏改）', () => {
    const keys = EDITOR_SHORTCUT_GROUPS.flatMap(group => group.entries.map(entry => entry.keys))
    expect(new Set(keys).size).toBe(keys.length)
  })
})

describe('与编辑器绑定一致（防漂移）', () => {
  it('editorKeymap 绑的就是清单里那两条', () => {
    const hooks = { onChange: vi.fn(), onRun: vi.fn(), onSave: vi.fn() }
    const bound = editorKeymap(hooks).map(binding => binding.key)

    expect(bound).toEqual([RUN_KEY, SAVE_KEY])
    expect(runSaveGroup().entries.map(entry => entry.keys)).toEqual([RUN_KEY, SAVE_KEY])
  })

  it('editorKeymap 的两个回调真的接到 hooks 上', () => {
    const hooks = { onChange: vi.fn(), onRun: vi.fn(), onSave: vi.fn() }
    const [runBinding, saveBinding] = editorKeymap(hooks)

    expect(runBinding.run?.({} as never)).toBe(true)
    expect(saveBinding.run?.({} as never)).toBe(true)
    expect(hooks.onRun).toHaveBeenCalledTimes(1)
    expect(hooks.onSave).toHaveBeenCalledTimes(1)
  })
})
