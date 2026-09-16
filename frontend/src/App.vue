<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import ModeToggle from './components/ModeToggle.vue'

const router = useRouter()
const currentPath = ref(router.currentRoute.value.path)

router.afterEach((to) => { currentPath.value = to.path })

onMounted(async () => {
  // 监听托盘菜单事件 → 触发对应任务或打开设置
  await listen<string>('tray-menu', async (e) => {
    const id = e.payload
    if (id === 'settings') {
      await getCurrentWindow().show()
      await getCurrentWindow().setFocus()
      router.push('/settings')
    } else if (id === 'quit') {
      await getCurrentWindow().destroy()
    } else if (id === 'send') {
      await invoke('start_send')
    } else if (id === 'recv') {
      await invoke('start_recv')
    } else if (id === 'deploy_type') {
      await invoke('start_deploy_type')
    } else if (id === 'deploy_copy') {
      await invoke('deploy_copy')
    }
  })
})
</script>

<template>
  <div class="min-h-screen flex flex-col max-w-3xl mx-auto">
    <nav class="flex gap-2 px-4 py-3 border-b">
      <a
        class="px-3 py-1.5 rounded hover:bg-[var(--accent)] cursor-pointer"
        :class="{ 'bg-[var(--primary)] text-white': currentPath === '/' }"
        @click="router.push('/')"
      >总览</a>
      <a
        class="px-3 py-1.5 rounded hover:bg-[var(--accent)] cursor-pointer"
        :class="{ 'bg-[var(--primary)] text-white': currentPath === '/settings' }"
        @click="router.push('/settings')"
      >设置</a>

      <ModeToggle />
    </nav>
    <main class="flex-1 p-6">
      <router-view />
    </main>
  </div>
</template>
