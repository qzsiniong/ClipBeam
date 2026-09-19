<!--
  脚本 Console 面板：显示 `console.log/info/debug/warn/error` 以及运行状态行。

  数据来源：
  * 挂载时 `get_script_console` 取全量（窗口重开也能恢复本次运行的日志）；
  * 之后增量听 `script-console` 事件，`script-console-clear` 清空。

  后端缓冲上限 1000 行（见 src-tauri/src/console_panel.rs），面板只负责渲染与过滤。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ChevronDown, ChevronUp, Eraser, Terminal } from 'lucide-vue-next'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'

/** 一行输出（对应后端 `ConsoleLine`）。 */
interface ConsoleLine { seq: number, level: string, text: string, ts: number }

const lines = ref<ConsoleLine[]>([])
const collapsed = ref(false)
const filter = ref<'all' | 'log' | 'info' | 'debug' | 'warn' | 'error'>('all')

const scroller = ref<HTMLDivElement | null>(null)
/** 用户手动向上滚动后暂停自动滚动，滚回底部恢复。 */
const stickToBottom = ref(true)

const FILTERS = ['all', 'log', 'info', 'debug', 'warn', 'error'] as const

const visibleLines = computed(() =>
  filter.value === 'all' ? lines.value : lines.value.filter(line => line.level === filter.value),
)

/** 等级 → 文字颜色（与 Tailwind token 一致，明暗主题都可用）。 */
function levelClass(level: string): string {
  switch (level) {
    case 'error':
      return 'text-destructive'
    case 'warn':
      return 'text-amber-600 dark:text-amber-500'
    case 'debug':
      return 'text-muted-foreground/70'
    default:
      return 'text-foreground/90'
  }
}

function fmtTime(ts: number): string {
  const date = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

async function loadSnapshot() {
  try {
    lines.value = await invoke<ConsoleLine[]>('get_script_console')
  }
  catch {
    // 面板是辅助功能：拿不到日志不该打断脚本页
  }
}

async function clear() {
  try {
    await invoke('clear_script_console')
  }
  catch {
    // 同上
  }
}

/** 贴底时滚到最新一行。 */
async function scrollToBottom() {
  if (!stickToBottom.value || collapsed.value)
    return
  await nextTick()
  const element = scroller.value
  if (element)
    element.scrollTop = element.scrollHeight
}

function onScroll() {
  const element = scroller.value
  if (!element)
    return
  // 距离底部 24px 以内视为「贴底」
  stickToBottom.value = element.scrollHeight - element.scrollTop - element.clientHeight < 24
}

const unlistens: UnlistenFn[] = []

onMounted(async () => {
  await loadSnapshot()

  unlistens.push(await listen<ConsoleLine>('script-console', (event) => {
    lines.value.push(event.payload)
  }))
  unlistens.push(await listen('script-console-clear', () => {
    lines.value = []
  }))
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
})

watch(visibleLines, () => {
  void scrollToBottom()
})

// 展开时把视图拉回底部
watch(collapsed, (value) => {
  if (!value) {
    stickToBottom.value = true
    void scrollToBottom()
  }
})
</script>

<template>
  <section class="shrink-0 border-t" :class="collapsed ? '' : 'h-52'">
    <!-- 工具栏 -->
    <div class="flex h-9 items-center gap-2 px-3 text-xs">
      <Terminal class="h-3.5 w-3.5 text-muted-foreground" />
      <span class="font-medium">Console</span>
      <span class="text-muted-foreground">{{ visibleLines.length }}/{{ lines.length }}</span>

      <select
        v-model="filter"
        class="ml-2 h-6 rounded-md border bg-background px-1 text-[11px] text-muted-foreground"
      >
        <option v-for="item in FILTERS" :key="item" :value="item">
          {{ item === 'all' ? '全部等级' : item }}
        </option>
      </select>

      <div class="ml-auto flex items-center gap-1">
        <Button size="icon-xs" variant="ghost" title="清空" @click="clear">
          <Eraser class="h-3.5 w-3.5" />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          :title="collapsed ? '展开' : '折叠'"
          @click="collapsed = !collapsed"
        >
          <component :is="collapsed ? ChevronUp : ChevronDown" class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>

    <!-- 输出区 -->
    <div
      v-show="!collapsed"
      ref="scroller"
      class="h-[calc(100%-2.25rem)] overflow-y-auto px-3 pb-2 font-mono text-[11px] leading-5"
      @scroll="onScroll"
    >
      <div
        v-for="line in visibleLines"
        :key="line.seq"
        class="flex gap-2 whitespace-pre-wrap break-all"
      >
        <span class="shrink-0 text-muted-foreground/60 select-none">{{ fmtTime(line.ts) }}</span>
        <span :class="levelClass(line.level)">{{ line.text }}</span>
      </div>

      <div v-if="visibleLines.length === 0" class="py-2 text-muted-foreground">
        {{ lines.length === 0 ? '（运行脚本后这里会显示 console 输出）' : '当前过滤条件下没有输出' }}
      </div>
    </div>
  </section>
</template>
