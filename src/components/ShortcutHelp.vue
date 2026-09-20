<!--
  脚本编辑器的快捷键面板。

  为什么要它：编辑器绑了一堆键（运行/保存是项目自己绑的，撤销/查找/补全是 CodeMirror 的），
  但界面上原先没有任何地方能查 —— 只有 README 里提了两条。这里把清单放到窗口里：

  * 头部一个 ⌨ 按钮（icon-only，避免最小宽度 760px 时挤爆工具栏）；
  * 面板内容来自 `@/lib/clipbeam-script/shortcuts`，与编辑器真实绑定共用同一份键位串；
  * 键位按平台格式化（macOS 显示 ⌘⇧⌥，其它平台显示 Ctrl/Shift/Alt）。
-->
<script setup lang="ts">
import { Keyboard } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { EDITOR_SHORTCUT_GROUPS, formatShortcut, isMac } from '@/lib/clipbeam-script/shortcuts'

/** 平台只判断一次：窗口生命周期内不会变。 */
const mac = isMac()
</script>

<template>
  <Popover>
    <PopoverTrigger as-child>
      <Button size="sm" variant="ghost" title="快捷键" aria-label="快捷键">
        <Keyboard class="h-4 w-4" />
      </Button>
    </PopoverTrigger>

    <PopoverContent align="end" :side-offset="6" class="max-h-[60vh] w-80 overflow-y-auto p-3">
      <div class="mb-2 text-sm font-medium">
        常用快捷键
      </div>

      <div v-for="group in EDITOR_SHORTCUT_GROUPS" :key="group.title" class="mb-2 last:mb-0">
        <div class="mb-1 text-xs font-medium text-muted-foreground">
          {{ group.title }}
        </div>
        <div
          v-for="entry in group.entries"
          :key="entry.keys"
          class="flex items-center justify-between gap-4 py-1 text-sm"
        >
          <span>{{ entry.label }}</span>
          <kbd class="shrink-0 rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
            {{ formatShortcut(entry, mac) }}
          </kbd>
        </div>
      </div>

      <p class="mt-2 border-t pt-2 text-xs text-muted-foreground">
        提示：鼠标悬停在标识符上可以查看类型与说明。
      </p>
    </PopoverContent>
  </Popover>
</template>
