<!--
  侧边栏的「导航区」：总览 / 脚本 / 设置。

  抽成独立组件的原因是两个窗口都要用同一套导航：
  * 主窗口（`main`）：脚本项打开**独立脚本窗口**；
  * 脚本窗口（`scripting`）：总览/设置项打开**主窗口**。

  点「脚本」在脚本窗口里没有意义（已经在脚本窗口了），由父组件决定如何处理。
-->
<script setup lang="ts">
import { ClipboardPaste, FileCode2, Settings as SettingsIcon } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'

const props = withDefaults(defineProps<{
  /** 当前高亮的路径（主窗口用路由 path；脚本窗口只有脚本项）。 */
  current?: string
  /** 是否渲染「脚本」项（脚本窗口里传 false）。 */
  showScripts?: boolean
}>(), {
  current: '',
  showScripts: true,
})

const emit = defineEmits<{
  /** 用户点了某一项：给出它的路径（`/`、`/scripts`、`/settings`）。 */
  (e: 'navigate', path: string): void
}>()

const items = [
  { name: '总览', path: '/', icon: ClipboardPaste },
  { name: '脚本', path: '/scripts', icon: FileCode2 },
  { name: '设置', path: '/settings', icon: SettingsIcon },
]

/** 脚本项在脚本窗口里不显示（已经在脚本窗口了）。 */
const visibleItems = () => props.showScripts ? items : items.filter(item => item.path !== '/scripts')
</script>

<template>
  <nav class="flex flex-col gap-1 p-2">
    <Button
      v-for="item in visibleItems()"
      :key="item.path"
      :variant="item.path === current ? 'secondary' : 'ghost'"
      class="w-full justify-start gap-2 font-normal"
      @click="emit('navigate', item.path)"
    >
      <component :is="item.icon" class="h-4 w-4" />
      {{ item.name }}
    </Button>
  </nav>
</template>
