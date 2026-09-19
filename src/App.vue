<script setup lang="ts">
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppSidebar from '@/components/AppSidebar.vue'

const router = useRouter()
const route = useRoute()
const currentPath = ref(route.path)

// 按**窗口 label**（而不是路由）决定布局：进度/待命是纯独立窗口；
// 主窗口与脚本窗口都用 aside 侧边栏布局。
const windowLabel = getCurrentWindow().label
const bareWindow = computed(() => windowLabel === 'progress' || windowLabel === 'standby')

router.afterEach((to) => {
  currentPath.value = to.path
})

const pageTitle = computed(() => {
  const map: Record<string, string> = {
    '/': '总览',
    '/scripting': '脚本',
    '/settings': '设置',
  }
  return map[currentPath.value] || 'ClipBeam'
})

onMounted(async () => {
  // 进度/待命窗口不处理托盘事件
  if (bareWindow.value)
    return

  // 托盘「设置…」只由**主窗口**处理：脚本窗口跑的是同一套 Vue 应用（同一个 router），
  // 如果也响应这个事件，它会跟着导航到 /settings —— 表现就是「两个窗口都在显示设置」。
  // Rust 侧已经用 emit_to("main") 只发给主窗口，这里再按 label 兜一道，
  // 避免以后有人改回广播时又踩同一个坑。
  if (windowLabel !== 'main')
    return

  // 「脚本编辑器…」由 Rust 直接打开脚本窗口（见 lib.rs 的 open_scripting_window）
  await listen<string>('tray-menu', async (e) => {
    if (e.payload !== 'settings')
      return
    await getCurrentWindow().show()
    await getCurrentWindow().setFocus()
    router.push('/settings')
  })
})
</script>

<template>
  <!-- 独立窗口(进度/待命):纯页面，无侧边栏 -->
  <router-view v-if="bareWindow" />

  <!-- 主窗口与脚本窗口:aside 侧边栏布局 -->
  <div v-else class="flex h-screen overflow-hidden">
    <!-- 侧边栏由各窗口自己提供:主窗口用 AppSidebar；脚本窗口自带 ScriptsSidebar
         （它还要展示脚本列表），所以这里不能再渲染一个，否则会出现两个 aside。 -->
    <AppSidebar v-if="windowLabel === 'main'" />
    <main class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <!-- 只有主窗口在这里渲染页头:脚本窗口自带工具栏(运行/保存/刷新) -->
      <header v-if="windowLabel === 'main'" class="flex h-16 shrink-0 items-center gap-2 border-b px-6">
        <span class="text-lg font-semibold">
          {{ pageTitle }}
        </span>
      </header>
      <div
        class="min-h-0 flex-1"
        :class="windowLabel === 'main' ? 'overflow-y-auto p-6' : 'overflow-hidden'"
      >
        <router-view />
      </div>
    </main>
  </div>
</template>
