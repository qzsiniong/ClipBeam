<!--
  共享的「任务状态」徽标：监听 worker-* 事件显示空闲/发送中/接收中/部署中/脚本运行中。

  两个侧边栏（主窗口的 AppSidebar、脚本窗口的 ScriptsSidebar）都用它，
  避免状态文案在多处漂移。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Badge } from '@/components/ui/badge'

const busy = ref(false)
const kind = ref<'send' | 'recv' | 'deploytype' | 'script' | null>(null)

const kindLabel: Record<string, string> = {
  send: '发送中',
  recv: '接收中',
  deploytype: '部署中',
  script: '脚本运行中',
}

const statusText = computed(() => {
  if (!busy.value)
    return '空闲'
  return kind.value ? kindLabel[kind.value] : '运行中'
})

const statusVariant = computed(() => (busy.value ? 'secondary' : 'success'))

const unlistens: UnlistenFn[] = []

onMounted(async () => {
  unlistens.push(await listen<'send' | 'recv' | 'deploytype' | 'script'>('worker-started', (e) => {
    busy.value = true
    kind.value = e.payload
  }))
  unlistens.push(await listen('worker-finished', () => {
    busy.value = false
    kind.value = null
  }))
  unlistens.push(await listen('worker-cancelled', () => {
    busy.value = false
    kind.value = null
  }))
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
})
</script>

<template>
  <Badge :variant="statusVariant">
    {{ statusText }}
  </Badge>
</template>
