<script setup lang="ts">
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppSidebar from '@/components/AppSidebar.vue'

const router = useRouter()
const route = useRoute()
const currentPath = ref(route.path)

router.afterEach((to) => {
  currentPath.value = to.path
})

// 独立窗口(进度/待命)不显示 sidebar 与托盘监听
const isStandaloneWindow = computed(() =>
  currentPath.value === '/progress' || currentPath.value === '/standby',
)

const pageTitle = computed(() => {
  const map: Record<string, string> = {
    '/': '总览',
    '/settings': '设置',
  }
  return map[currentPath.value] || 'ClipBeam'
})

onMounted(async () => {
  if (getCurrentWindow().label === 'progress' || getCurrentWindow().label === 'standby')
    return
  // 监听托盘「设置」菜单(任务类菜单由 Rust 直接处理)
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
  <!-- 独立窗口:无 sidebar 布局 -->
  <router-view v-if="isStandaloneWindow" />

  <!-- 主窗口:sidebar 布局 -->
  <div v-else class="flex h-screen overflow-hidden">
    <AppSidebar />
    <main class="flex flex-1 flex-col overflow-hidden">
      <header class="flex h-16 shrink-0 items-center gap-2 border-b px-6">
        <span class="text-lg font-semibold">
          {{ pageTitle }}
        </span>
      </header>
      <div class="flex-1 overflow-y-auto p-6">
        <router-view />
      </div>
    </main>
  </div>
</template>
