<script setup lang="ts">
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import ModeToggle from './components/ModeToggle.vue'

const router = useRouter()
const currentPath = ref(router.currentRoute.value.path)

router.afterEach((to) => {
  currentPath.value = to.path
})

onMounted(async () => {
  // 进度窗口不监听托盘菜单,由主窗口处理
  if (getCurrentWindow().label === 'progress')
    return
  // 监听托盘「设置」菜单(任务类菜单由 Rust 直接处理,不再经前端)
  await listen<string>('tray-menu', async (e) => {
    const id = e.payload
    if (id === 'settings') {
      await getCurrentWindow().show()
      await getCurrentWindow().setFocus()
      router.push('/settings')
    }
  })
})
</script>

<template>
  <div class="min-h-screen flex flex-col max-w-3xl mx-auto">
    <nav v-if="currentPath !== '/progress'" class="flex gap-2 px-4 py-3 border-b">
      <a
        class="px-3 py-1.5 rounded hover:bg-accent cursor-pointer"
        :class="{ 'bg-primary text-white': currentPath === '/' }"
        @click="router.push('/')"
      >总览</a>
      <a
        class="px-3 py-1.5 rounded hover:bg-accent cursor-pointer"
        :class="{ 'bg-primary text-white': currentPath === '/settings' }"
        @click="router.push('/settings')"
      >设置</a>

      <ModeToggle />
    </nav>
    <main class="flex-1 p-6">
      <router-view />
    </main>
  </div>
</template>
