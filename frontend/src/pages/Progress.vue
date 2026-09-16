<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { LogicalPosition } from '@tauri-apps/api/dpi'
import { listen } from '@tauri-apps/api/event'
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onMounted, onUnmounted, ref } from 'vue'

import { Badge } from '@/components/ui/badge'
import { Card, CardContent } from '@/components/ui/card'

interface ProgressPayload {
  kind: 'send' | 'recv' | 'deploytype'
  got: number
  total: number
  started_at: number
  ts: number
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
  }, 100)
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
  }, 3000)
}

function clearHide() {
  if (hideTimer !== null) {
    clearTimeout(hideTimer)
    hideTimer = null
  }
}

async function positionWindowRight() {
  // 1. 获取当前窗口所在的显示器信息
  // currentMonitor() 返回的是包含物理尺寸和缩放因子的对象
  const appWindow = getCurrentWindow()
  const monitor = await currentMonitor() // 获取当前显示器

  if (monitor) {
    // 2. 获取显示器的逻辑尺寸 (已自动处理缩放)
    // size 是 PhysicalSize, availableSize 也是 PhysicalSize
    // 我们需要通过 scaleFactor 转换为逻辑尺寸，或者直接使用 API 提供的逻辑方法
    const scaleFactor = monitor.scaleFactor
    // 物理尺寸转逻辑尺寸
    const screenLogicalWidth = monitor.size.width / scaleFactor

    // 3. 获取当前窗口的逻辑大小
    // innerSize() 返回的是 PhysicalSize，同样需要转换
    const physicalWinSize = await appWindow.innerSize()
    const winLogicalWidth = physicalWinSize.width / scaleFactor
    // const winLogicalHeight = physicalWinSize.height / scaleFactor

    // 4. 计算靠右的 X 坐标 (逻辑坐标)
    // 如果想要紧贴右边缘，x = 屏幕逻辑宽 - 窗口逻辑宽
    const x = screenLogicalWidth - winLogicalWidth - 10 // 留出 10 像素边距
    const y = 100 // 顶部对齐，可根据需求调整

    // 5. 设置位置
    // setPosition 接受 LogicalPosition 或 PhysicalPosition
    // 传入 LogicalPosition 时，Tauri 会自动根据当前缩放因子转换为物理坐标发送给系统
    await appWindow.setPosition(new LogicalPosition(x, y))
  }
}

const titleText = computed(() => {
  const map: Record<string, string> = {
    send: '发送到远程',
    recv: '截屏接收',
    deploytype: '部署接收页',
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
  <div>
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-lg font-semibold">
        {{ titleText || '任务进度' }}
      </h2>
      <Badge :variant="isBusy ? 'secondary' : finished ? 'success' : 'default'">
        {{ isBusy ? '进行中' : finished ? '完成' : cancelled ? '已中止' : '空闲' }}
      </Badge>
    </div>

    <!-- 运行中:进度条 + 统计 -->
    <div v-if="isBusy && total > 0" class="space-y-3">
      <div class="w-full h-3 rounded-full bg-muted overflow-hidden">
        <div
          class="h-full bg-primary transition-all duration-150"
          :style="{ width: `${progressPercent}%` }"
        />
      </div>
      <div class="text-sm text-muted-foreground">
        {{ got }} / {{ total }} {{ unitLabel }} · {{ progressPercent.toFixed(1) }}%
      </div>
      <div class="text-xs text-muted-foreground">
        已用 {{ fmtDuration(elapsedMs) }}
        <span v-if="speed > 0">
          · 速度 {{ speed.toFixed(1) }} {{ speedUnit }}
          · 剩余 {{ fmtDuration(remainingMs) }}
        </span>
      </div>
    </div>

    <!-- 运行中但还没收到进度(初始化阶段) -->
    <div v-else-if="isBusy" class="text-sm text-muted-foreground">
      正在初始化…
    </div>

    <!-- 结束:结果卡片 -->
    <Card v-else-if="finished" class="border-primary/30 bg-primary/5">
      <CardContent class="pt-4">
        <div class="font-medium">
          {{ finished.title }}
        </div>
        <div class="text-sm text-muted-foreground mt-1">
          {{ finished.body }}
        </div>
      </CardContent>
    </Card>

    <!-- 中止 -->
    <Card v-else-if="cancelled" class="border-destructive/30 bg-destructive/5">
      <CardContent class="pt-4">
        <div class="text-sm text-destructive">
          任务已中止
        </div>
      </CardContent>
    </Card>
  </div>
</template>
