<script setup lang="ts">
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { MousePointerClick } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Button } from '@/components/ui/button'

const kind = ref<'send' | 'recv' | 'deploytype' | 'script' | null>(null)
const hotkey = ref('')
const countdown = ref(10)
const cancelled = ref(false)
/** 脚本用 `$.request_focus("提示")` 带来的自定义提示语；空 = 用默认文案。 */
const hint = ref<string | null>(null)
/** 暂停失焦检测：用户需要好几次切换焦点时用（后端会同时冻结倒计时）。 */
const paused = ref(false)
let countdownTimer: number | null = null

const unlistens: (() => void)[] = []

const kindLabel = computed(() => {
  const map: Record<string, string> = {
    send: '发送到远程',
    recv: '截屏接收',
    deploytype: '部署接收页',
    script: '运行脚本',
  }
  return kind.value ? map[kind.value] : ''
})

onMounted(async () => {
  const win = getCurrentWindow()
  // 待命事件只属于待命窗口：其它窗口跑的是同一套 Vue 应用，按 label 兜一道，
  // 避免以后事件改成广播时误触发（见 lib.rs / standby.rs 里的 emit_to 修复）。
  if (win.label !== 'standby')
    return

  unlistens.push(await listen<{ kind: 'send' | 'recv' | 'deploytype' | 'script', hotkey: string, hint?: string | null }>('standby-config', (e) => {
    kind.value = e.payload.kind
    hotkey.value = e.payload.hotkey
    hint.value = e.payload.hint ?? null
    // 每一轮待命都从「未暂停」开始（后端 arm 时同样会复位）
    paused.value = false
    countdown.value = 10
    cancelled.value = false
    startCountdown()
  }))

  unlistens.push(await listen('standby-start', () => {
    stopCountdown()
    win.hide()
  }))

  unlistens.push(await listen('standby-cancel', () => {
    cancelled.value = true
    stopCountdown()
    setTimeout(() => win.hide(), 1000)
  }))
})

function startCountdown() {
  stopCountdown()
  countdownTimer = window.setInterval(() => {
    countdown.value -= 1
    if (countdown.value <= 0)
      stopCountdown()
  }, 1000)
}

function stopCountdown() {
  if (countdownTimer !== null) {
    clearInterval(countdownTimer)
    countdownTimer = null
  }
}

async function manualCancel() {
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke('cancel_task')
}

/**
 * 暂停 / 恢复失焦检测。
 *
 * 暂停只抑制「失焦 = 已确认」这一个信号并冻结倒计时；取消与热键照常可用。
 * 恢复时倒计时重新给满 —— 用户是点我们这个按钮恢复的，所以焦点此刻在待命窗口上，
 * 还需要再点一次目标窗口才会确认（回到原来的逻辑）。
 *
 * 命令返回的是**生效后的「是否暂停」**；若状态没变（例如这一轮待命已经结束），
 * 就什么都不做，免得把倒计时搅乱。
 */
async function togglePause() {
  const { invoke } = await import('@tauri-apps/api/core')
  const effective = await invoke<boolean>('set_standby_paused', { paused: !paused.value })
  if (effective === paused.value)
    return

  paused.value = effective
  if (effective) {
    stopCountdown()
  }
  else {
    countdown.value = 10
    startCountdown()
  }
}

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  stopCountdown()
})
</script>

<template>
  <div class="flex h-full flex-col items-center justify-center gap-2 p-4 text-center">
    <div class="flex h-10 w-10 items-center justify-center rounded-full bg-primary/10">
      <MousePointerClick class="h-5 w-5 text-primary" />
    </div>

    <div v-if="!cancelled">
      <h2 class="text-sm font-semibold">
        {{ kindLabel }}
      </h2>
      <p class="mt-0.5 text-xs text-muted-foreground">
        {{ hint ?? '请点击目标远程窗口开始' }}
      </p>
      <p v-if="hotkey" class="mt-0.5 text-[10px] text-muted-foreground">
        触发热键: {{ hotkey }}
      </p>
      <div v-if="!paused" class="mt-2 text-xl font-bold text-primary">
        {{ countdown }}s
      </div>
      <p v-else class="mt-2 text-xs font-medium text-primary">
        已暂停检测：准备好后点「恢复检测」
      </p>
      <div class="mt-2 flex items-center justify-center gap-1">
        <Button variant="ghost" size="sm" class="h-7 text-xs" @click="togglePause">
          {{ paused ? '恢复检测' : '暂停检测' }}
        </Button>
        <Button variant="ghost" size="sm" class="h-7 text-xs" @click="manualCancel">
          取消
        </Button>
      </div>
    </div>

    <div v-else>
      <h2 class="text-sm font-semibold text-muted-foreground">
        已取消
      </h2>
    </div>
  </div>
</template>
