<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'

interface TaskOutcome { title: string, body: string }
type TaskKind = 'send' | 'recv' | 'deploytype' | null

const busy = ref(false)
const kind = ref<TaskKind>(null)
const lastOutcome = ref<TaskOutcome | null>(null)
const lastError = ref<string | null>(null)

// 实时进度(改进 2)
const progressGot = ref(0)
const progressTotal = ref(0)
const progressStartedAt = ref(0)
const now = ref(Date.now())
let tickTimer: number | null = null

const unlistens: UnlistenFn[] = []

async function startSendRaw() {
  lastError.value = null
  try {
    await invoke('start_send_raw')
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function startSend() {
  lastError.value = null
  try {
    await invoke('start_send')
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function startRecv() {
  lastError.value = null
  try {
    await invoke('start_recv')
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function startDeployType() {
  lastError.value = null
  try {
    await invoke('start_deploy_type')
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function deployCopy() {
  lastError.value = null
  try {
    const chars = await invoke<number>('deploy_copy')
    lastOutcome.value = {
      title: '✓ 已复制到剪贴板',
      body: `自解压接收页（${chars} 字符）已复制到宿主机剪贴板`,
    }
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function cancelTask() {
  await invoke('cancel_task')
}

/** 脚本有自己的大窗口(见 tauri.conf.json 的 `scripting`)。 */
async function openScripts() {
  await invoke('open_scripts_window')
}

async function showWindow() {
  await getCurrentWindow().show()
  await getCurrentWindow().setFocus()
}

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

function fmtDuration(ms: number): string {
  const s = Math.floor(ms / 1000)
  if (s < 60)
    return `${s}s`
  const m = Math.floor(s / 60)
  return `${m}m${s % 60}s`
}

onMounted(async () => {
  unlistens.push(await listen<TaskKind>('worker-started', (e) => {
    busy.value = true
    kind.value = e.payload
    progressGot.value = 0
    progressTotal.value = 0
    progressStartedAt.value = 0
    startTick()
  }))
  unlistens.push(await listen<{ got: number, total: number, started_at: number }>('worker-progress', (e) => {
    progressGot.value = e.payload.got
    progressTotal.value = e.payload.total
    progressStartedAt.value = e.payload.started_at
    now.value = Date.now()
  }))
  unlistens.push(await listen<TaskOutcome>('worker-finished', (e) => {
    lastOutcome.value = e.payload
    busy.value = false
    kind.value = null
    stopTick()
  }))
  unlistens.push(await listen<string>('worker-cancelled', () => {
    busy.value = false
    kind.value = null
    stopTick()
    lastError.value = '任务已中止'
  }))
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  stopTick()
})

const statusText = computed(() => {
  if (!busy.value)
    return '空闲'
  const labelMap: Record<string, string> = {
    send: '发送中',
    recv: '接收中',
    deploytype: '部署中',
  }
  return kind.value ? labelMap[kind.value] : ''
})

const statusVariant = computed(() => {
  if (busy.value)
    return 'secondary'
  return 'success'
})

const elapsedMs = computed(() =>
  progressStartedAt.value ? Math.max(0, now.value - progressStartedAt.value) : 0,
)

const speed = computed(() => {
  if (elapsedMs.value < 500 || progressGot.value === 0)
    return 0
  return progressGot.value / (elapsedMs.value / 1000)
})

const progressPercent = computed(() =>
  progressTotal.value > 0 ? Math.min(100, (progressGot.value / progressTotal.value) * 100) : 0,
)

const unitLabel = computed(() => (kind.value === 'recv' ? '帧' : '字符'))
const speedUnit = computed(() => (kind.value === 'recv' ? '帧/秒' : '字符/秒'))
</script>

<template>
  <div class="space-y-4">
    <!-- 任务状态卡片 -->
    <Card class="rounded-xl shadow-sm">
      <CardHeader>
        <div class="flex items-center justify-between">
          <div>
            <CardTitle>任务状态</CardTitle>
            <CardDescription>点击下方按钮触发，或使用全局热键</CardDescription>
          </div>
          <Badge :variant="statusVariant">
            {{ statusText }}
          </Badge>
        </div>
      </CardHeader>
      <CardContent>
        <!-- 实时进度(改进 2) -->
        <div v-if="busy && progressTotal > 0" class="space-y-2 mb-4">
          <div class="w-full h-2 rounded-full bg-muted overflow-hidden">
            <div
              class="h-full bg-primary transition-all duration-700"
              :style="{ width: `${progressPercent}%` }"
            />
          </div>
          <div class="text-sm text-muted-foreground">
            {{ progressGot }} / {{ progressTotal }} {{ unitLabel }} · {{ progressPercent.toFixed(0) }}%
          </div>
          <div class="text-xs text-muted-foreground">
            已用 {{ fmtDuration(elapsedMs) }}
            <span v-if="speed > 0">
              · 速度 {{ Math.floor(speed) }} {{ speedUnit }}
            </span>
          </div>
        </div>

        <div v-if="lastOutcome" class="mb-4 p-3 rounded-lg bg-primary/5 border border-primary/30">
          <div class="font-medium">
            {{ lastOutcome.title }}
          </div>
          <div class="text-sm text-muted-foreground mt-1">
            {{ lastOutcome.body }}
          </div>
        </div>
        <div v-if="lastError" class="mb-4 p-3 rounded-lg bg-destructive/5 border border-destructive/30">
          <div class="text-sm text-destructive">
            {{ lastError }}
          </div>
        </div>
      </CardContent>
      <CardFooter class="flex flex-wrap gap-2">
        <Button :disabled="busy" @click="startSendRaw">
          发送到远程(原样)
        </Button>
        <Button :disabled="busy" @click="startSend">
          发送到远程
        </Button>
        <Button :disabled="busy" @click="startRecv">
          截屏接收
        </Button>
        <Button :disabled="busy" variant="outline" @click="startDeployType">
          部署接收页(键盘)
        </Button>
        <Button :disabled="busy" variant="outline" @click="deployCopy">
          部署接收页(剪贴板)
        </Button>
        <Button :disabled="busy" variant="outline" @click="openScripts">
          运行脚本
        </Button>
        <Button v-if="busy" variant="destructive" @click="cancelTask">
          中止
        </Button>
      </CardFooter>
    </Card>

    <Separator />

    <!-- 快捷入口 -->
    <Card class="rounded-xl shadow-sm">
      <CardHeader>
        <CardTitle>快捷入口</CardTitle>
      </CardHeader>
      <CardContent class="flex flex-wrap gap-2">
        <Button variant="outline" @click="$router.push('/settings')">
          设置…
        </Button>
        <Button variant="ghost" @click="showWindow">
          显示主窗口
        </Button>
      </CardContent>
    </Card>

    <div class="text-xs text-muted-foreground text-center pt-4">
      远程端接收页请通过 <code class="px-1 py-0.5 rounded bg-muted">web/clipbeam.html</code> 部署
    </div>
  </div>
</template>
