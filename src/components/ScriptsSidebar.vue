<!--
  脚本窗口的侧边栏：与主窗口**同一套 aside 布局**（同样的宽度、品牌头、任务状态），
  只是把导航区换成了「脚本列表 + 新建」。

  底部的「总览 / 设置」跳回主窗口（脚本窗口是独立窗口，不能靠路由切过去）。
-->
<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { FilePlus2, LayoutDashboard, Settings as SettingsIcon, Trash2 } from 'lucide-vue-next'
import { ref } from 'vue'
import TaskStatusBadge from '@/components/TaskStatusBadge.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

interface ScriptMeta { name: string, path: string, language: 'js' | 'ts' }

const props = defineProps<{
  /** 脚本列表。 */
  scripts: ScriptMeta[]
  /** 当前打开的脚本名。 */
  current: string | null
  /** 任务运行中时禁用编辑类操作。 */
  busy: boolean
}>()

const emit = defineEmits<{
  (e: 'select', name: string): void
  (e: 'create', name: string): void
  (e: 'remove', name: string): void
}>()

const newName = ref('')

function create() {
  const name = newName.value.trim()
  if (!name)
    return
  emit('create', name)
  newName.value = ''
}

/** 主窗口是独立窗口：先把它显示出来，再让它在对应路由上落位。 */
async function openMain() {
  await invoke('open_main_window')
}
</script>

<template>
  <aside class="flex h-full w-60 shrink-0 flex-col border-r bg-background/95">
    <!-- 顶部:应用名 -->
    <div class="flex h-16 shrink-0 items-center gap-2 border-b px-4">
      <div class="flex h-8 w-8 items-center justify-center rounded-md bg-primary text-primary-foreground text-sm font-bold">
        C
      </div>
      <div class="flex flex-col">
        <span class="text-sm font-semibold">ClipBeam 脚本</span>
        <span class="text-xs text-muted-foreground">v0.1.0</span>
      </div>
    </div>

    <!-- 中间:脚本列表 + 新建 -->
    <div class="flex-1 overflow-hidden flex flex-col gap-2 p-2">
      <div class="flex items-center gap-1">
        <Input
          v-model="newName"
          placeholder="new-script.js"
          class="h-8 text-xs"
          @keydown.enter="create"
        />
        <Button size="icon-sm" variant="outline" title="新建脚本" :disabled="props.busy" @click="create">
          <FilePlus2 class="h-4 w-4" />
        </Button>
      </div>

      <div class="min-h-0 flex-1 space-y-1 overflow-y-auto pr-1">
        <div
          v-for="script in props.scripts"
          :key="script.name"
          class="flex items-center gap-1 rounded-md px-1"
          :class="script.name === props.current ? 'bg-secondary' : 'hover:bg-accent/50'"
        >
          <button
            class="flex-1 truncate py-1.5 text-left text-xs"
            :title="script.path"
            @click="emit('select', script.name)"
          >
            {{ script.name }}
          </button>
          <Badge variant="outline" class="shrink-0 text-[10px]">
            {{ script.language }}
          </Badge>
          <Button
            size="icon-xs"
            variant="ghost"
            title="删除"
            :disabled="props.busy"
            @click="emit('remove', script.name)"
          >
            <Trash2 class="h-3 w-3" />
          </Button>
        </div>

        <div
          v-if="props.scripts.length === 0"
          class="rounded-md border border-dashed p-3 text-xs text-muted-foreground"
        >
          还没有脚本。上面输入文件名（如 <code>demo.js</code>）后按回车新建。
        </div>
      </div>
    </div>

    <!-- 底部:跳回主窗口 + 任务状态 -->
    <div class="shrink-0 border-t p-2 space-y-1">
      <Button
        variant="ghost"
        class="w-full justify-start gap-2 font-normal"
        @click="openMain"
      >
        <LayoutDashboard class="h-4 w-4" />
        总览
      </Button>
      <Button
        variant="ghost"
        class="w-full justify-start gap-2 font-normal"
        @click="openMain"
      >
        <SettingsIcon class="h-4 w-4" />
        设置
      </Button>
      <div class="flex items-center justify-between rounded-md px-2 py-1.5">
        <span class="text-xs text-muted-foreground">状态</span>
        <TaskStatusBadge />
      </div>
    </div>
  </aside>
</template>
