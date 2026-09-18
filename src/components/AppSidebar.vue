<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import { ClipboardPaste, Github, Settings as SettingsIcon } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'

const route = useRoute()
const router = useRouter()

const navItems = [
  { name: '总览', path: '/', icon: ClipboardPaste },
  { name: '设置', path: '/settings', icon: SettingsIcon },
]

const isActive = (path: string) => computed(() => route.path === path).value

const busy = ref(false)
const kind = ref<'send' | 'recv' | 'deploytype' | null>(null)

const kindLabel: Record<string, string> = {
  send: '发送中',
  recv: '接收中',
  deploytype: '部署中',
}

const statusText = computed(() => {
  if (!busy.value)
    return '空闲'
  return kind.value ? kindLabel[kind.value] : '运行中'
})

const statusVariant = computed(() => (busy.value ? 'secondary' : 'success'))

const unlistens: UnlistenFn[] = []

onMounted(async () => {
  unlistens.push(await listen<'send' | 'recv' | 'deploytype'>('worker-started', (e) => {
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
  <aside class="flex h-full w-60 flex-col border-r bg-background/95">
    <!-- 顶部:应用名 -->
    <div class="flex h-16 shrink-0 items-center gap-2 border-b px-4">
      <div class="flex h-8 w-8 items-center justify-center rounded-md bg-primary text-primary-foreground text-sm font-bold">
        C
      </div>
      <div class="flex flex-col">
        <span class="text-sm font-semibold">ClipBeam</span>
        <span class="text-xs text-muted-foreground">v0.1.0</span>
      </div>
    </div>

    <!-- 中间:导航 -->
    <nav class="flex-1 flex flex-col gap-1 p-2">
      <Button
        v-for="item in navItems"
        :key="item.path"
        :variant="isActive(item.path) ? 'secondary' : 'ghost'"
        class="w-full justify-start gap-2 font-normal"
        @click="router.push(item.path)"
      >
        <component :is="item.icon" class="h-4 w-4" />
        {{ item.name }}
      </Button>
    </nav>

    <!-- 底部:状态 + GitHub -->
    <div class="shrink-0 border-t p-2 space-y-1">
      <div class="flex items-center justify-between rounded-md px-2 py-1.5">
        <span class="text-xs text-muted-foreground">状态</span>
        <Badge :variant="statusVariant">
          {{ statusText }}
        </Badge>
      </div>
      <Button variant="ghost" class="w-full justify-start gap-2 text-xs text-muted-foreground" @click="router.push('/')">
        <Github class="h-4 w-4" />
        GitHub
      </Button>
    </div>
  </aside>
</template>
