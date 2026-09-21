<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { X } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'

interface ProgressPayload {
  kind: 'send' | 'recv' | 'deploytype' | 'script'
  got: number
  total: number
  started_at: number
  ts: number
  /** 脚本任务当前输出的片段(其它任务为空)。 */
  snippet?: string
}

interface TaskOutcome {
  title: string
  body: string
}

/** 任务结束后、用户没碰过窗口时，多久自动收起（碰过就一直留着，由 ✕ 手动关）。 */
const AUTO_HIDE_MS = 5000

const kind = ref<ProgressPayload['kind'] | null>(null)
const got = ref(0)
const total = ref(0)
const startedAt = ref(0)
const now = ref(Date.now())
const finished = ref<TaskOutcome | null>(null)
/** 脚本任务当前输出的片段（第二行右侧展示）。 */
const snippet = ref('')
const cancelled = ref(false)
const isBusy = ref(false)
/** 用户点过/拖过这个窗口 = 关心这次结果 → 结束之后不再自动收起。 */
const userInteracted = ref(false)

const unlistens: UnlistenFn[] = []
let tickTimer: number | null = null
let hideTimer: number | null = null

function startTick() {
  if (tickTimer !== null)
    return
  tickTimer = window.setInterval(() => {
    now.value = Date.now()
  }, 500)
}

function stopTick() {
  if (tickTimer !== null) {
    clearInterval(tickTimer)
    tickTimer = null
  }
}

function scheduleHide() {
  // 任务结束后如果用户碰过窗口（关心结果），就不自动关 —— 让他自己点 ✕
  if (userInteracted.value)
    return
  if (hideTimer !== null)
    clearTimeout(hideTimer)
  hideTimer = window.setTimeout(async () => {
    stopTick()
    await getCurrentWindow().hide()
  }, AUTO_HIDE_MS)
}

function clearHide() {
  if (hideTimer !== null) {
    clearTimeout(hideTimer)
    hideTimer = null
  }
}

/** ✕：立即收起（位置由 Rust 侧记住，下次任务还在原地弹出）。 */
async function hideNow() {
  clearHide()
  stopTick()
  await getCurrentWindow().hide()
}

/**
 * 按住窗口任意空白处拖动（✕ 按钮除外）。
 *
 * 顺便把「用户碰过窗口」记下来：这是「他关心这次结果」的信号；如果此时已经排好了
 * 自动隐藏（任务刚结束），要把它取消掉 —— 否则用户刚点完它就消失了。
 * 浏览器 mock 下没有真实窗口，`startDragging` 会静默失败，忽略即可。
 */
async function onPointerDown(e: PointerEvent) {
  markInterested()
  const target = e.target as HTMLElement | null
  if (target?.closest('[data-no-drag]'))
    return
  try {
    await getCurrentWindow().startDragging()
  }
  catch {
    // dev mock / 无窗口环境下没有 startDragging，忽略
  }
}

/**
 * 标记「用户在关心这次结果」：任务结束后不再自动收起。
 *
 * 两个来源：这里的鼠标事件，以及 Rust 侧发来的 `progress-interacted`
 * （macOS 上点击未激活窗口时鼠标事件可能被系统吞掉，只有窗口焦点是可靠的）。
 */
function markInterested() {
  userInteracted.value = true
  clearHide()
}

const titleText = computed(() => {
  const map: Record<string, string> = {
    send: '发送',
    recv: '接收',
    deploytype: '部署',
    script: '脚本',
  }
  return kind.value ? map[kind.value] : ''
})

const statusText = computed(() => {
  if (isBusy.value)
    return '进行中'
  if (finished.value)
    return '完成'
  if (cancelled.value)
    return '已中止'
  return ''
})

const statusClass = computed(() => {
  if (isBusy.value)
    return 'bg-primary/10 text-primary'
  if (finished.value)
    return 'bg-green-500/10 text-green-600'
  if (cancelled.value)
    return 'bg-red-500/10 text-red-500'
  return 'bg-muted text-muted-foreground'
})

const elapsedMs = computed(() =>
  startedAt.value ? Math.max(0, now.value - startedAt.value) : 0,
)

const speed = computed(() => {
  if (elapsedMs.value < 500 || got.value === 0)
    return 0
  return got.value / (elapsedMs.value / 1000)
})

const remainingMs = computed(() => {
  if (speed.value === 0 || total.value === 0)
    return 0
  return Math.max(0, ((total.value - got.value) / speed.value) * 1000)
})

const progressPercent = computed(() =>
  total.value > 0 ? Math.min(100, (got.value / total.value) * 100) : 0,
)

const unitLabel = computed(() => (kind.value === 'recv' ? '帧' : '字符'))
const speedUnit = computed(() => (kind.value === 'recv' ? '帧/秒' : '字符/秒'))

function fmtDuration(ms: number): string {
  const s = Math.floor(ms / 1000)
  if (s < 60)
    return `${s}s`
  const m = Math.floor(s / 60)
  return `${m}m${s % 60}s`
}

onMounted(async () => {
  unlistens.push(
    await listen<ProgressPayload['kind']>('worker-started', (e) => {
      isBusy.value = true
      kind.value = e.payload
      got.value = 0
      total.value = 0
      startedAt.value = 0
      finished.value = null
      cancelled.value = false
      snippet.value = ''
      // 每个任务重新数：上一个任务里「碰过窗口」不代表这次也关心
      userInteracted.value = false
      clearHide()
      startTick()
    }),
  )
  unlistens.push(
    await listen<ProgressPayload>('worker-progress', (e) => {
      const p = e.payload
      kind.value = p.kind
      got.value = p.got
      total.value = p.total
      startedAt.value = p.started_at
      snippet.value = p.snippet ?? ''
      now.value = Date.now()
    }),
  )
  unlistens.push(
    await listen<TaskOutcome>('worker-finished', (e) => {
      isBusy.value = false
      finished.value = e.payload
      stopTick()
      scheduleHide()
    }),
  )
  unlistens.push(
    await listen<string>('worker-cancelled', () => {
      isBusy.value = false
      cancelled.value = true
      stopTick()
      scheduleHide()
    }),
  )
  // Rust 侧发现窗口被点/拖过（窗口获得焦点）→ 与上面的鼠标事件同一个含义
  unlistens.push(await listen('progress-interacted', markInterested))
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  stopTick()
  clearHide()
})
</script>

<template>
  <!-- 悬浮面板（420×96）：任务/状态；进度条；统计或结果标题；输出片段或结果说明。整块可拖，✕ 除外。 -->
  <div
    class="flex h-screen flex-col justify-center gap-1.5 rounded-xl bg-background px-4 select-none shadow-lg"
    @pointerdown="onPointerDown"
    @mouseenter="markInterested"
  >
    <!-- 第一行：任务名 + 状态 + 百分比 + 关闭 -->
    <div class="flex items-center gap-2">
      <span class="shrink-0 text-sm font-semibold">{{ titleText || '任务' }}</span>
      <span v-if="statusText" class="shrink-0 rounded px-1.5 py-px text-[11px]" :class="statusClass">
        {{ statusText }}
      </span>
      <span class="ml-auto shrink-0 text-xs font-medium tabular-nums text-muted-foreground">
        {{ progressPercent.toFixed(0) }}%
      </span>
      <button
        data-no-drag
        type="button"
        class="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
        title="关闭"
        @click="hideNow"
      >
        <X class="h-4 w-4" />
      </button>
    </div>

    <!-- 第二行：进度条 -->
    <div class="h-2 w-full overflow-hidden rounded-full bg-muted">
      <div
        class="h-full bg-primary transition-all duration-700"
        :style="{ width: `${progressPercent}%` }"
      />
    </div>

    <!-- 第三行：运行中看统计；结束后看结果标题 -->
    <div class="flex items-center gap-2 text-[11px] text-muted-foreground">
      <template v-if="isBusy">
        <span class="shrink-0 tabular-nums">{{ got }}/{{ total }}{{ unitLabel }}</span>
        <span v-if="speed > 0" class="shrink-0 tabular-nums">{{ Math.floor(speed) }} {{ speedUnit }}</span>
        <span v-if="remainingMs > 0" class="shrink-0 tabular-nums">剩余 {{ fmtDuration(remainingMs) }}</span>
        <span class="shrink-0 tabular-nums">已用 {{ fmtDuration(elapsedMs) }}</span>
      </template>
      <span
        v-else-if="finished"
        class="min-w-0 flex-1 truncate font-medium text-foreground"
        :title="finished.title"
      >{{ finished.title }}</span>
      <span v-else-if="cancelled" class="text-red-500">已中止</span>
      <span v-else>初始化…</span>
    </div>

    <!-- 第四行：脚本输出片段 / 结果说明（固定高度，避免整块跳动） -->
    <div class="h-4 min-w-0 text-[11px] text-muted-foreground">
      <span
        v-if="isBusy && snippet"
        class="block truncate font-mono"
        :title="snippet"
      >正在输出：{{ snippet }}</span>
      <span
        v-else-if="finished && finished.body"
        class="block truncate"
        :title="finished.body"
      >{{ finished.body }}</span>
    </div>
  </div>
</template>
