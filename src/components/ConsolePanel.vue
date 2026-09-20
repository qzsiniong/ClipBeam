<!--
  脚本 Console 面板：显示 `console.log/info/debug/warn/error` 以及运行状态行。

  数据来源：
  * 挂载时 `get_script_console` 取全量（窗口重开也能恢复本次运行的日志）；
  * 之后增量听 `script-console` 事件，`script-console-clear` 清空。

  后端缓冲上限 1000 行（见 src-tauri/src/console_panel.rs），面板只负责渲染与过滤。

  形态（`view`，由父组件 `ScriptsWindow` 持有并决定外层布局）：
  * `normal`：占 Splitter 下栏，可拖拽调整高度；
  * `collapsed`：只剩标题栏；
  * `maximized`：占满整个编辑区（编辑器仍挂载，只是被压到 0 高，避免丢撤销历史）。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import type { ConsoleView } from '@/components/console-view'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ChevronDown, ChevronUp, Eraser, Maximize2, Minimize2, Terminal } from 'lucide-vue-next'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'

/** 一行输出（对应后端 `ConsoleLine`）。 */
interface ConsoleLine { seq: number, level: string, text: string, ts: number }

const props = withDefaults(defineProps<{ view?: ConsoleView }>(), { view: 'normal' })
const emit = defineEmits<{ 'update:view': [ConsoleView] }>()

const lines = ref<ConsoleLine[]>([])
const filter = ref<'all' | 'log' | 'info' | 'debug' | 'warn' | 'error'>('all')

const collapsed = computed(() => props.view === 'collapsed')
const maximized = computed(() => props.view === 'maximized')

/** 切换形态：折叠/最大化是互斥的，回到 normal 表示恢复。 */
function setView(next: ConsoleView) {
  emit('update:view', props.view === next ? 'normal' : next)
}

const scroller = ref<HTMLDivElement | null>(null)
/**
 * 是否跟随最新输出（默认跟随）。
 *
 * 用户手动往上翻时置 false，滚回底部自动恢复 true —— 判定只认**用户**的滚动，
 * 见 [`onScroll`] 与 [`autoScrolling`]。
 */
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

/**
 * 拉取历史输出并**合并**进当前列表。
 *
 * 不能写成 `lines.value = await invoke(...)`：这个请求是异步的，而脚本可能已经在推
 * `script-console` 事件了 —— 晚到的快照会把刚收到的实时行**整段覆盖掉**（表现为
 * 「面板里没有日志 / 只有前几行」）。所以按 `seq` 去重合并，两边都不丢。
 */
async function loadSnapshot() {
  let snapshot: ConsoleLine[] = []
  try {
    snapshot = await invoke<ConsoleLine[]>('get_script_console')
  }
  catch {
    // 面板是辅助功能：拿不到历史不该打断脚本页
    return
  }

  const bySeq = new Map<number, ConsoleLine>()
  for (const line of snapshot) {
    bySeq.set(line.seq, line)
  }
  for (const line of lines.value) {
    bySeq.set(line.seq, line)
  }
  lines.value = [...bySeq.values()].sort((a, b) => a.seq - b.seq)
}

async function clear() {
  try {
    await invoke('clear_script_console')
  }
  catch {
    // 同上
  }
}

/** 我们主动滚动期间为 true —— 用来把手动滚动与程序滚动区分开。 */
let autoScrolling = false

/**
 * 滚到最新一行。
 *
 * 只在「用户本来就贴着底部」时生效（往上翻看历史时不要把他拽回来）。
 */
async function scrollToBottom() {
  if (!stickToBottom.value || collapsed.value)
    return
  await nextTick()
  const element = scroller.value
  if (!element)
    return

  autoScrolling = true
  // 用最大合法值，兼容浏览器把 scrollTop 钳到 scrollHeight - clientHeight 的行为
  element.scrollTop = element.scrollHeight
  // 等一帧：滚动会异步派发 `scroll` 事件，让它在 autoScrolling 仍为 true 时到达
  requestAnimationFrame(() => {
    autoScrolling = false
    // 已经贴底了，把标记补上（清空后内容变短等情况）
    stickToBottom.value = true
  })
}

function onScroll() {
  // 程序滚动（我们刚把 scrollTop 设到底）不算用户意图：新日志让内容变高时，
  // 浏览器会先派发一次 `scroll`，此时位置已不在底部 —— 若在这里判定，就会误把
  // 「自动跟随」关掉，于是**新行再也不会自动滚到底**（这就是之前的 bug）。
  if (autoScrolling) {
    return
  }

  const element = scroller.value
  if (!element)
    return
  // 距离底部 24px 以内视为「贴底」
  stickToBottom.value = element.scrollHeight - element.scrollTop - element.clientHeight < 24
}

const unlistens: UnlistenFn[] = []

onMounted(async () => {
  await loadSnapshot()
  // 打开面板/重新打开窗口时先贴到底，看到最新一行
  void scrollToBottom()

  unlistens.push(await listen<ConsoleLine>('script-console', (event) => {
    lines.value.push(event.payload)
  }))
  unlistens.push(await listen('script-console-clear', () => {
    lines.value = []
    // 清空后内容变短，重新跟随
    stickToBottom.value = true
    void scrollToBottom()
  }))
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
})

/**
 * 新输出到达后跟随到底部。
 *
 * 这里特意 watch `lines.length`（数字）而**不是** `visibleLines`：
 * 过滤为「全部」时 `visibleLines` 就是 `lines.value` **同一个数组引用**，
 * Vue 做值比较时认为没变 —— watch 永远不触发，于是自动滚动整个失效（踩过）。
 * 数字长度是原始值，变化必然可见；过滤切换另由 `filter` 单独处理。
 */
watch(() => lines.value.length, () => {
  void scrollToBottom()
})

// 切换等级过滤后也跟随到底部（过滤会改变可见行数）
watch(filter, () => {
  stickToBottom.value = true
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
  <section
    class="flex h-full min-h-0 flex-col overflow-hidden"
    :class="view === 'normal' ? '' : 'border-t'"
  >
    <!-- 工具栏 -->
    <div class="flex h-9 shrink-0 items-center gap-2 px-3 text-xs">
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
          v-if="!maximized"
          size="icon-xs"
          variant="ghost"
          :title="collapsed ? '展开' : '折叠'"
          @click="setView('collapsed')"
        >
          <component :is="collapsed ? ChevronUp : ChevronDown" class="h-3.5 w-3.5" />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          :title="maximized ? '还原（回到可拖拽的高度）' : '最大化（占满整个编辑区）'"
          @click="setView('maximized')"
        >
          <component :is="maximized ? Minimize2 : Maximize2" class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>

    <!-- 输出区 -->
    <div
      v-show="!collapsed"
      ref="scroller"
      class="min-h-0 flex-1 overflow-y-auto px-3 pb-2 font-mono text-[11px] leading-5"
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
