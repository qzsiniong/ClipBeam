<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'

interface Progress { got: number; total: number }
interface TaskOutcome { title: string; body: string }
type TaskKind = 'send' | 'recv' | 'deploytype' | null

const busy = ref(false)
const kind = ref<TaskKind>(null)
const got = ref(0)
const total = ref(0)
const lastOutcome = ref<TaskOutcome | null>(null)
const lastError = ref<string | null>(null)

let unlistens: UnlistenFn[] = []

async function startSend() {
  lastError.value = null
  try {
    await invoke('start_send')
  } catch (e) {
    lastError.value = String(e)
  }
}

async function startRecv() {
  lastError.value = null
  try {
    await invoke('start_recv')
  } catch (e) {
    lastError.value = String(e)
  }
}

async function startDeployType() {
  lastError.value = null
  try {
    await invoke('start_deploy_type')
  } catch (e) {
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
  } catch (e) {
    lastError.value = String(e)
  }
}

async function cancelTask() {
  await invoke('cancel_task')
}

async function showWindow() {
  await getCurrentWindow().show()
  await getCurrentWindow().setFocus()
}

onMounted(async () => {
  unlistens.push(await listen<TaskKind>('worker-started', (e) => {
    busy.value = true
    kind.value = e.payload
    got.value = 0
    total.value = 0
  }))
  unlistens.push(await listen<Progress>('worker-progress', (e) => {
    got.value = e.payload.got
    total.value = e.payload.total
  }))
  unlistens.push(await listen<TaskOutcome>('worker-finished', (e) => {
    lastOutcome.value = e.payload
    busy.value = false
    kind.value = null
    got.value = 0
    total.value = 0
  }))
  unlistens.push(await listen<string>('hotkey', async (e) => {
    const id = e.payload
    if (busy.value) {
      // 忙时:stop 或当前任务热键 → 中止
      if (id === 'stop' || id === kind.value) {
        await cancelTask()
      }
    } else {
      // 空闲:send/recv 触发新任务
      if (id === 'send') await startSend()
      else if (id === 'recv') await startRecv()
    }
  }))
})

onUnmounted(() => {
  unlistens.forEach((fn) => fn())
})

const statusText = computed(() => {
  if (!busy.value) return '空闲'
  const labelMap: Record<string, string> = {
    send: '发送中',
    recv: '接收中',
    deploytype: '部署中',
  }
  const label = kind.value ? labelMap[kind.value] : ''
  if (kind.value === 'recv' && total.value > 0) {
    return `${label} ${got.value}/${total.value} 帧`
  }
  return label
})

const statusVariant = computed(() => {
  if (busy.value) return 'secondary'
  return 'success'
})

const progressWidth = computed(() => {
  return total.value > 0 ? `${(got.value / total.value) * 100}%` : '0%'
})
</script>

<template>
    <div class="space-y-4">
      <div class="flex items-center justify-between">
        <div>
          <h1 class="text-2xl font-bold">ClipBeam</h1>
          <p class="text-sm text-muted-foreground">宿主机 ↔ 远程剪贴板桥</p>
        </div>
        <Badge :variant="statusVariant">{{ statusText }}</Badge>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>任务状态</CardTitle>
          <CardDescription>点击下方按钮触发，或使用全局热键</CardDescription>
        </CardHeader>
        <CardContent>
          <div v-if="lastOutcome" class="mb-4 p-3 rounded-lg bg-(--accent)/10 border border-border">
            <div class="font-medium">{{ lastOutcome.title }}</div>
            <div class="text-sm text-muted-foreground mt-1">{{ lastOutcome.body }}</div>
          </div>
          <div v-if="lastError" class="mb-4 p-3 rounded-lg bg-(--destructive)/10 border border-destructive">
            <div class="text-sm text-destructive">{{ lastError }}</div>
          </div>
          <div v-if="busy && kind === 'recv'" class="space-y-1">
            <div class="text-sm text-muted-foreground">接收进度</div>
            <div class="w-full h-2 rounded-full bg-muted overflow-hidden">
              <div
                class="h-full bg-primary transition-all"
                :style="{ width: progressWidth }"
              />
            </div>
            <div class="text-xs text-muted-foreground">{{ got }} / {{ total }} 帧</div>
          </div>
        </CardContent>
        <CardFooter class="flex flex-wrap gap-2">
          <Button :disabled="busy" @click="startSend">发送到远程</Button>
          <Button :disabled="busy" @click="startRecv">截屏接收</Button>
          <Button :disabled="busy" variant="outline" @click="startDeployType">部署接收页(键盘)</Button>
          <Button :disabled="busy" variant="outline" @click="deployCopy">部署接收页(剪贴板)</Button>
          <Button v-if="busy" variant="destructive" @click="cancelTask">中止</Button>
        </CardFooter>
      </Card>

      <Separator />

      <Card>
        <CardHeader>
          <CardTitle>快捷入口</CardTitle>
        </CardHeader>
        <CardContent class="flex flex-wrap gap-2">
          <Button variant="outline" @click="$router.push('/settings')">设置…</Button>
          <Button variant="ghost" @click="showWindow">显示主窗口</Button>
        </CardContent>
      </Card>

      <div class="text-xs text-muted-foreground text-center pt-4">
        远程端接收页请通过 <code class="px-1 py-0.5 rounded bg-muted">web/clipbeam.html</code> 部署
      </div>
    </div>
</template>
