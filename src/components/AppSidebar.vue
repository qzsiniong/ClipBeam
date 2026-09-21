<!--
  主窗口的侧边栏：品牌头 + 导航 + 任务状态。

  导航用共享的 [`AppNav`](./AppNav.vue)；「脚本」项在这里的意思是
  **打开独立的脚本窗口**（脚本页不在主窗口内渲染）。
-->
<script setup lang="ts">
import { getVersion } from '@tauri-apps/api/app'
import { invoke } from '@tauri-apps/api/core'
import { Github } from 'lucide-vue-next'
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppNav from '@/components/AppNav.vue'
import TaskStatusBadge from '@/components/TaskStatusBadge.vue'
import { Button } from '@/components/ui/button'

const route = useRoute()
const router = useRouter()

const currentPath = computed(() => route.path)

/** 应用版本：真源是 `tauri.conf.json`，运行时用 `getVersion()` 读，界面里不硬编码。 */
const version = ref('')
onMounted(async () => {
  try {
    version.value = await getVersion()
  }
  catch {
    // 浏览器 mock / 非 Tauri 环境拿不到版本，留空即可
  }
})

async function navigate(path: string) {
  if (path === '/scripts') {
    // 脚本有自己的大窗口：侧边栏这里只是入口
    await invoke('open_scripts_window')
    return
  }
  await router.push(path)
}
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
        <span v-if="version" class="text-xs text-muted-foreground">v{{ version }}</span>
      </div>
    </div>

    <!-- 中间:导航 -->
    <div class="flex-1">
      <AppNav :current="currentPath" @navigate="navigate" />
    </div>

    <!-- 底部:状态 + GitHub -->
    <div class="shrink-0 border-t p-2 space-y-1">
      <div class="flex items-center justify-between rounded-md px-2 py-1.5">
        <span class="text-xs text-muted-foreground">状态</span>
        <TaskStatusBadge />
      </div>
      <Button variant="ghost" class="w-full justify-start gap-2 text-xs text-muted-foreground" @click="router.push('/')">
        <Github class="h-4 w-4" />
        GitHub
      </Button>
    </div>
  </aside>
</template>
