<!--
  插件管理页（主窗口 `/plugins`）。

  这一页只做三件事：让用户**看见**插件（状态 / 权限 / 错误）、**开关**它们（启用 / 停用 /
  重新加载）、以及**看它们的日志**。

  界面上的状态与 Rust 侧 `PluginState` 一一对应（`src-tauri/src/plugin_manager.rs`）：
  关闭 / 运行中 / 异常 / 无效 / 停止超时 —— 每一档都要能解释「为什么」，
  所以卡片上同时显示 `error` 原文（清单问题、入口报错、卡住提示都落在那里）。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { FolderOpen, Loader2, Plug, RefreshCw, RotateCw } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

/** 与 Rust `PluginState` 同名（serde 小写）。 */
type PluginState = 'invalid' | 'disabled' | 'active' | 'error' | 'stoptimeout'

/** 与 Rust `PluginInfo` 同名。 */
interface PluginInfo {
  id: string
  name: string
  version: string
  description: string | null
  author: string | null
  dir: string
  entry: string
  state: PluginState
  permissions: string[]
  menus: [string, string][]
  error: string | null
  usable: boolean
}

/** 与 Rust `ConsoleLine` 同名。 */
interface ConsoleLine { seq: number, level: string, text: string, ts: number }

const plugins = ref<PluginInfo[]>([])
const dir = ref('')
const loading = ref(false)
const busyId = ref<string | null>(null)
/** 每个插件一条错误（命令返回的失败，与卡片上的 `error` 是两回事）。 */
const commandErrors = ref<Record<string, string>>({})

/** 日志：当前选中的插件 + 它的行（只保留最近 500 行，够排查）。 */
const selectedId = ref<string | null>(null)
const logLines = ref<ConsoleLine[]>([])
const unlistens: UnlistenFn[] = []

const MAX_LOG_LINES = 500

const stateLabel: Record<PluginState, string> = {
  invalid: '清单有问题',
  disabled: '已关闭',
  active: '运行中',
  error: '异常',
  stoptimeout: '停止超时',
}

const stateVariant: Record<PluginState, 'secondary' | 'success' | 'warning' | 'error' | 'outline'> = {
  invalid: 'error',
  disabled: 'secondary',
  active: 'success',
  error: 'error',
  stoptimeout: 'warning',
}

const selectedPlugin = computed(
  () => plugins.value.find(plugin => plugin.id === selectedId.value) ?? null,
)

/** 权限名 → 界面文案（与 Rust `Permission::doc` 的含义一致，这里只求短）。 */
const permissionLabel: Record<string, string> = {
  feedback: '应用内提示',
  notification: '系统通知',
  system_dialog: '原生对话框',
  tray: '托盘菜单',
  window: '自定义窗口',
}

async function load(scan = false) {
  loading.value = true
  try {
    plugins.value = await invoke<PluginInfo[]>(scan ? 'refresh_plugins' : 'list_plugins')
    // 选中的插件被删掉/改名了：清掉选择，避免日志区显示一个不存在的插件
    if (selectedId.value && !plugins.value.some(plugin => plugin.id === selectedId.value)) {
      selectedId.value = null
      logLines.value = []
    }
    commandErrors.value = {}
  }
  catch (e) {
    commandErrors.value = { __list__: String(e) }
  }
  finally {
    loading.value = false
  }
}

async function loadDir() {
  try {
    const info = await invoke<{ dir: string }>('plugins_info')
    dir.value = info.dir
  }
  catch {
    // 拿不到目录只是提示少一行，不影响主流程
  }
}

/** 启用 / 停用：后端返回新的列表，直接替换（避免自己猜状态）。 */
async function toggle(plugin: PluginInfo) {
  busyId.value = plugin.id
  commandErrors.value = { ...commandErrors.value, [plugin.id]: '' }
  try {
    const command = plugin.state === 'active' ? 'disable_plugin' : 'enable_plugin'
    plugins.value = await invoke<PluginInfo[]>(command, { id: plugin.id })
  }
  catch (e) {
    commandErrors.value = { ...commandErrors.value, [plugin.id]: String(e) }
  }
  finally {
    busyId.value = null
  }
}

async function reload(plugin: PluginInfo) {
  busyId.value = plugin.id
  commandErrors.value = { ...commandErrors.value, [plugin.id]: '' }
  try {
    plugins.value = await invoke<PluginInfo[]>('reload_plugin', { id: plugin.id })
  }
  catch (e) {
    commandErrors.value = { ...commandErrors.value, [plugin.id]: String(e) }
  }
  finally {
    busyId.value = null
  }
}

async function selectLogs(plugin: PluginInfo) {
  selectedId.value = plugin.id
  try {
    logLines.value = await invoke<ConsoleLine[]>('get_plugin_console', { id: plugin.id })
  }
  catch {
    logLines.value = []
  }
}

async function clearLogs() {
  if (!selectedId.value)
    return
  try {
    await invoke('clear_plugin_console', { id: selectedId.value })
    logLines.value = []
  }
  catch {
    // 清空失败不值得打断用户
  }
}

/** 日志等级配色（与脚本 Console 面板同一套）。 */
function levelClass(level: string): string {
  switch (level) {
    case 'error': return 'text-red-500'
    case 'warn': return 'text-amber-600'
    case 'debug': return 'text-muted-foreground'
    default: return 'text-foreground'
  }
}

onMounted(async () => {
  await loadDir()
  await load()

  // 后端在启停/刷新之后会广播列表；这里跟着更新，界面不会与真实状态脱节
  unlistens.push(
    await listen<PluginInfo[]>('plugins-changed', (event) => {
      plugins.value = event.payload
    }),
  )
  // 插件日志（Rust 侧复用脚本 Console 的事件名，但载荷带 plugin_id）
  unlistens.push(
    await listen<{ plugin_id: string, line: ConsoleLine }>('script-console', (event) => {
      if (event.payload.plugin_id !== selectedId.value)
        return
      logLines.value = [...logLines.value, event.payload.line].slice(-MAX_LOG_LINES)
    }),
  )
  unlistens.push(
    await listen<{ plugin_id: string }>('script-console-clear', () => {
      logLines.value = []
    }),
  )
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
})
</script>

<template>
  <div class="flex flex-col gap-4 pb-10">
    <!-- 顶部：目录 + 刷新 -->
    <div class="flex flex-wrap items-center gap-2">
      <div class="min-w-0 flex-1">
        <div class="text-sm font-medium">
          插件
        </div>
        <div v-if="dir" class="truncate font-mono text-xs text-muted-foreground" :title="dir">
          {{ dir }}
        </div>
      </div>
      <Button variant="outline" size="sm" :disabled="loading" @click="load(true)">
        <RefreshCw class="mr-1 h-3.5 w-3.5" :class="{ 'animate-spin': loading }" />
        刷新
      </Button>
    </div>

    <div v-if="commandErrors.__list__" class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
      {{ commandErrors.__list__ }}
    </div>

    <!-- 空状态：把「怎么装插件」说清楚，而不是只显示"没有数据" -->
    <Card v-if="!plugins.length && !loading">
      <CardHeader>
        <CardTitle class="text-base">
          还没有插件
        </CardTitle>
        <CardDescription>
          插件是 <code class="font-mono">plugins/</code> 下的一个目录：里面要有
          <code class="font-mono">plugin.json</code>（清单）与入口脚本（缺省
          <code class="font-mono">index.js</code>）。把目录拷进来后点「刷新」即可。
        </CardDescription>
      </CardHeader>
      <CardContent class="flex items-center gap-2 text-sm text-muted-foreground">
        <FolderOpen class="h-4 w-4" />
        <span class="font-mono text-xs">{{ dir }}</span>
      </CardContent>
    </Card>

    <!-- 插件列表 -->
    <Card v-for="plugin in plugins" :key="plugin.id">
      <CardHeader class="pb-3">
        <div class="flex items-start gap-2">
          <Plug class="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <CardTitle class="text-base">
                {{ plugin.name }}
              </CardTitle>
              <Badge :variant="stateVariant[plugin.state]">
                {{ stateLabel[plugin.state] }}
              </Badge>
              <span class="text-xs text-muted-foreground">v{{ plugin.version }}</span>
              <code class="text-xs text-muted-foreground">{{ plugin.id }}</code>
            </div>
            <CardDescription v-if="plugin.description" class="mt-1">
              {{ plugin.description }}
            </CardDescription>
          </div>
          <div class="flex shrink-0 items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              :disabled="busyId === plugin.id"
              @click="selectLogs(plugin)"
            >
              日志
            </Button>
            <Button
              variant="outline"
              size="sm"
              :disabled="busyId === plugin.id || !plugin.usable"
              title="改完插件代码后点它生效"
              @click="reload(plugin)"
            >
              <RotateCw class="mr-1 h-3.5 w-3.5" />
              重新加载
            </Button>
            <Button
              :variant="plugin.state === 'active' ? 'secondary' : 'default'"
              size="sm"
              :disabled="busyId === plugin.id || !plugin.usable"
              @click="toggle(plugin)"
            >
              <Loader2 v-if="busyId === plugin.id" class="mr-1 h-3.5 w-3.5 animate-spin" />
              {{ plugin.state === 'active' ? '停用' : '启用' }}
            </Button>
          </div>
        </div>
      </CardHeader>

      <CardContent class="flex flex-col gap-2 pt-0">
        <!-- 权限：插件声明了什么能力，界面上必须看得见 -->
        <div class="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
          <span>权限：</span>
          <template v-if="plugin.permissions.length">
            <Badge v-for="permission in plugin.permissions" :key="permission" variant="outline">
              {{ permissionLabel[permission] ?? permission }}
            </Badge>
          </template>
          <span v-else>无（只能做纯计算）</span>
        </div>

        <!-- 托盘菜单项 -->
        <div v-if="plugin.menus.length" class="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
          <span>托盘菜单：</span>
          <span v-for="[id, label] in plugin.menus" :key="id" class="rounded bg-muted px-1.5 py-px">
            {{ label }}
          </span>
        </div>

        <div class="text-xs text-muted-foreground">
          入口：<code class="font-mono">{{ plugin.entry }}</code>
        </div>

        <!-- 问题 / 错误：清单问题与运行期错误都落到这里，原文照显示 -->
        <div
          v-if="plugin.error || commandErrors[plugin.id]"
          class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 font-mono text-xs whitespace-pre-wrap text-destructive"
        >
          {{ commandErrors[plugin.id] || plugin.error }}
        </div>

        <div v-if="plugin.author" class="text-xs text-muted-foreground">
          作者：{{ plugin.author }}
        </div>
      </CardContent>
    </Card>

    <!-- 日志面板 -->
    <Card v-if="selectedPlugin">
      <CardHeader class="flex-row items-center justify-between pb-3">
        <div>
          <CardTitle class="text-base">
            日志 · {{ selectedPlugin.name }}
          </CardTitle>
          <CardDescription class="text-xs">
            插件里的 <code class="font-mono">console.*</code> 与宿主的动作结果都在这里
          </CardDescription>
        </div>
        <Button variant="outline" size="sm" @click="clearLogs">
          清空
        </Button>
      </CardHeader>
      <CardContent>
        <div class="h-64 overflow-y-auto rounded-md bg-muted/40 p-2 font-mono text-xs">
          <div v-if="!logLines.length" class="text-muted-foreground">
            暂无日志
          </div>
          <div v-for="line in logLines" :key="line.seq" class="whitespace-pre-wrap" :class="levelClass(line.level)">
            {{ line.text }}
          </div>
        </div>
      </CardContent>
    </Card>
  </div>
</template>
