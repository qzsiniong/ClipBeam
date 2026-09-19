<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { LogicalPosition } from '@tauri-apps/api/dpi'
import { listen } from '@tauri-apps/api/event'
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window'
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

const kind = ref<ProgressPayload['kind'] | null>(null)
const got = ref(0)
const total = ref(0)
const startedAt = ref(0)
const now = ref(Date.now())
const finished = ref<TaskOutcome | null>(null)
/** 脚本任务当前输出的片段（进度条下方展示）。 */
const snippet = ref('')
const cancelled = ref(false)
const isBusy = ref(false)

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
  if (hideTimer !== null)
    clearTimeout(hideTimer)
  hideTimer = window.setTimeout(async () => {
    stopTick()
    await getCurrentWindow().hide()
  }, 5000)
}

function clearHide() {
  if (hideTimer !== null) {
    clearTimeout(hideTimer)
    hideTimer = null
  }
}

async function positionWindowRight() {
  const appWindow = getCurrentWindow()
  const monitor = await currentMonitor()
  if (monitor) {
    const scaleFactor = monitor.scaleFactor
    const screenLogicalWidth = monitor.size.width / scaleFactor
    const physicalWinSize = await appWindow.innerSize()
    const winLogicalWidth = physicalWinSize.width / scaleFactor
    const x = screenLogicalWidth - winLogicalWidth - 10
    const y = 100
    await appWindow.setPosition(new LogicalPosition(x, y))
  }
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
  positionWindowRight()
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
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  stopTick()
  clearHide()
})
</script>

<template>
  <!-- 悬浮条布局:紧凑单行 -->
  <div class="flex h-screen items-center gap-3 rounded-xl bg-background px-3 shadow-lg">
    <!-- 左:任务名 + 状态 -->
    <div class="flex shrink-0 flex-col">
      <span class="text-xs font-semibold">{{ titleText || '任务' }}</span>
      <span v-if="isBusy" class="text-[10px] text-primary">进行中</span>
      <span v-else-if="finished" class="text-[10px] text-green-600">完成</span>
      <span v-else-if="cancelled" class="text-[10px] text-red-500">已中止</span>
    </div>

    <!-- 中:进度条 -->
    <div v-if="isBusy && total > 0" class="flex flex-1 flex-col gap-1">
      <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
        <div
          class="h-full bg-primary transition-all duration-700"
          :style="{ width: `${progressPercent}%` }"
        />
      </div>
      <div class="flex items-center justify-between text-[10px] text-muted-foreground">
        <span>{{ progressPercent.toFixed(0) }}%</span>
        <span>{{ got }}/{{ total }}{{ unitLabel }}</span>
        <span v-if="speed > 0">{{ Math.floor(speed) }}{{ speedUnit }}</span>
        <span v-if="remainingMs > 0">剩余{{ fmtDuration(remainingMs) }}</span>
        <span>已用{{ fmtDuration(elapsedMs) }}</span>
      </div>
    </div>

    <!-- 脚本输出片段（脚本任务才有） -->
    <div
      v-if="isBusy && snippet"
      class="max-w-[160px] truncate text-[10px] text-muted-foreground"
      :title="snippet"
    >
      正在输出：{{ snippet }}
    </div>

    <!-- 进行中但未收到进度 -->
    <div v-else-if="isBusy" class="flex-1 text-xs text-muted-foreground">
      初始化…
    </div>

    <!-- 完成 -->
    <div v-else-if="finished" class="flex-1 text-xs text-muted-foreground line-clamp-2 overflow-hidden">
      {{ finished.body }}
    </div>

    <!-- 中止 -->
    <div v-else-if="cancelled" class="flex-1 text-xs text-red-500">
      已中止
    </div>
  </div>
</template>
