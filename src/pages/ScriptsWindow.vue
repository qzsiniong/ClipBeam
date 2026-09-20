<!--
  脚本窗口（独立大窗口，label `scripting`，路由 `/scripting`）。

  布局：左侧 aside 侧边栏（脚本列表/新建/跳主窗口） + 右侧「工具栏 + 编辑器 + Console」。

  运行语义（与 src-tauri/src/worker.rs 一致）：
  * 只有**会输出**的脚本才需要待命窗口：静态判定到 `.type_str` 时提前弹，
    真正开始输出前还有一次惰性兜底（见 src-tauri/src/standby.rs）；纯计算脚本直接跑完；
  * `$.confirm` 弹系统原生确认框（tauri-plugin-dialog），不回答就一直等；
  * `console.*` 与运行状态行都进底部 Console 面板（见 src-tauri/src/console_panel.rs）。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import type { CapabilityList, NamespaceNames } from '@/lib/clipbeam-script/autocomplete'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ask } from '@tauri-apps/plugin-dialog'
import { FolderOpen, Pin, PinOff, Play, RefreshCw, Save, Square } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import ConsolePanel from '@/components/ConsolePanel.vue'
import ScriptEditor from '@/components/ScriptEditor.vue'
import ScriptsSidebar from '@/components/ScriptsSidebar.vue'
import ShortcutHelp from '@/components/ShortcutHelp.vue'
import { Button } from '@/components/ui/button'
import { formatShortcut, isMac, RUN_KEY, SAVE_KEY } from '@/lib/clipbeam-script/shortcuts'

interface ScriptMeta { name: string, path: string, language: 'js' | 'ts' }
interface TaskOutcome { title: string, body: string }

const scripts = ref<ScriptMeta[]>([])
const capabilities = ref<CapabilityList['capabilities']>([])
/** 能力命名空间的名字（后端下发，前端不写死）。 */
const namespaceNames = ref<NamespaceNames>({ namespace: '', alias: '' })
const scriptsDir = ref('')
const currentName = ref<string | null>(null)
const source = ref('')
const savedSource = ref('')
const busy = ref(false)
const lastOutcome = ref<TaskOutcome | null>(null)
const lastError = ref<string | null>(null)

/** 窗口是否置顶（脚本窗口需要长时间停留，常和别的窗口并排用）。 */
const alwaysOnTop = ref(false)

const unlistens: UnlistenFn[] = []

const dirty = computed(() => currentName.value !== null && source.value !== savedSource.value)
const currentLanguage = computed<'js' | 'ts'>(() => currentName.value?.endsWith('.ts') ? 'ts' : 'js')

/**
 * 按钮上的快捷键提示（按平台格式化）。
 *
 * 与编辑器真实绑定共用 `shortcuts.ts` 的键位串：`RUN_KEY` / `SAVE_KEY` 既用于
 * `editorKeymap()`，也用在这里和快捷键面板里，改一处不会漏另一处。
 */
const mac = isMac()
const runKeyLabel = formatShortcut({ keys: RUN_KEY, label: '运行脚本' }, mac)
const saveKeyLabel = formatShortcut({ keys: SAVE_KEY, label: '保存脚本' }, mac)

/**
 * 需要用户确认的危险操作（切换/新建/删除会丢改动）用**系统原生**询问框。
 *
 * 为什么不用 `window.confirm`：Tauri 的 webview 把它接管成了插件调用，
 * 而我们只给 `dialog` 插件配置了消息类权限，`window.confirm()` 会直接抛
 * 「dialog.confirm not allowed. Command not found」。这里走插件的 `ask()`
 * （Yes/No 原生弹框），与脚本里的 `$.confirm` 是同一套系统弹框，观感一致。
 *
 * 权限：`src-tauri/capabilities/default.json` 里的 `dialog:allow-ask`
 * （`dialog:default` 只包含 open/save/message，不含消息类弹框）。
 */
async function askUser(message: string): Promise<boolean> {
  return ask(message, { title: 'ClipBeam', kind: 'warning' })
}

async function loadList() {
  try {
    scripts.value = await invoke<ScriptMeta[]>('list_scripts')
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function loadCapabilities() {
  try {
    const list = await invoke<CapabilityList>('list_capabilities')
    capabilities.value = list.capabilities
    namespaceNames.value = { namespace: list.namespace, alias: list.alias }
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function loadInfo() {
  try {
    const info = await invoke<{ dir: string, seeded: number }>('scripts_info')
    scriptsDir.value = info.dir
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function select(name: string) {
  if (name === currentName.value)
    return
  if (dirty.value && !(await askUser('当前脚本有未保存的修改，切换会丢失。继续吗？')))
    return

  lastError.value = null
  try {
    source.value = await invoke<string>('read_script', { name })
    savedSource.value = source.value
    currentName.value = name
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function create(name: string) {
  if (dirty.value && !(await askUser('当前脚本有未保存的修改，新建会丢失。继续吗？')))
    return

  lastError.value = null
  try {
    // 新建先落一个空文件，随后由用户编辑保存；名字非法时后端会拒绝
    await invoke('write_script', { name, source: '// 新建脚本\n' })
    await loadList()
    await select(name)
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function save() {
  if (!currentName.value)
    return false
  lastError.value = null
  try {
    await invoke('write_script', { name: currentName.value, source: source.value })
    savedSource.value = source.value
    await loadList()
    return true
  }
  catch (e) {
    lastError.value = String(e)
    return false
  }
}

async function remove(name: string) {
  if (!(await askUser(`删除脚本 ${name}？此操作不可撤销。`)))
    return
  lastError.value = null
  try {
    await invoke('delete_script', { name })
    if (currentName.value === name) {
      currentName.value = null
      source.value = ''
      savedSource.value = ''
    }
    await loadList()
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function run() {
  // 忙态下按钮是 disabled，键盘路径也要保持一致：否则会打到后端并弹出
  // 「已有任务在运行」这种看不懂的错误条
  if (!currentName.value || busy.value)
    return
  if (dirty.value) {
    const doSave = await askUser('脚本有未保存的修改。先保存再运行吗？（否 = 直接运行已保存的版本）')
    if (doSave && !(await save()))
      return
  }

  lastError.value = null
  lastOutcome.value = null
  try {
    await invoke('start_script', { name: currentName.value })
  }
  catch (e) {
    lastError.value = String(e)
  }
}

async function cancel() {
  await invoke('cancel_task')
}

/**
 * 切换窗口置顶。
 *
 * 用 Tauri 的窗口 API（权限见 `src-tauri/capabilities/default.json` 里的
 * `core:window:allow-set-always-on-top`）。不持久化：下次打开脚本窗口回到
 * `tauri.conf.json` 里的默认值，避免为此引入 `tauri-plugin-window-state`。
 */
async function toggleAlwaysOnTop() {
  lastError.value = null
  try {
    const next = !alwaysOnTop.value
    await getCurrentWindow().setAlwaysOnTop(next)
    alwaysOnTop.value = next
  }
  catch (e) {
    lastError.value = String(e)
  }
}

onMounted(async () => {
  await Promise.all([loadInfo(), loadList(), loadCapabilities()])

  // 置顶初始状态以窗口真实状态为准（别信本地默认值）
  try {
    alwaysOnTop.value = await getCurrentWindow().isAlwaysOnTop()
  }
  catch {
    // 拿不到就按未置顶显示；点一下按钮仍会尝试切换
  }

  // 第一个脚本自动打开，省一次点击
  const first = scripts.value[0]
  if (first)
    await select(first.name)

  unlistens.push(await listen<string>('worker-started', () => {
    busy.value = true
    lastOutcome.value = null
  }))
  unlistens.push(await listen<TaskOutcome>('worker-finished', (e) => {
    busy.value = false
    lastOutcome.value = e.payload
  }))
  unlistens.push(await listen('worker-cancelled', () => {
    busy.value = false
    lastError.value = '任务已中止'
  }))

  window.addEventListener('beforeunload', warnUnsaved)
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  window.removeEventListener('beforeunload', warnUnsaved)
})

function warnUnsaved(event: BeforeUnloadEvent) {
  if (!dirty.value)
    return
  event.preventDefault()
}
</script>

<template>
  <!-- 外层由 App.vue 的 shell 提供 flex 容器与高度，这里只负责横向分栏 -->
  <div class="flex h-full overflow-hidden">
    <ScriptsSidebar
      :scripts="scripts"
      :current="currentName"
      :busy="busy"
      @select="select"
      @create="create"
      @remove="remove"
    />

    <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <!-- 工具栏 -->
      <header class="flex h-16 shrink-0 items-center gap-2 border-b px-4">
        <span class="text-lg font-semibold">脚本</span>
        <span v-if="currentName" class="flex items-center gap-1 text-xs text-muted-foreground">
          <FolderOpen class="h-3.5 w-3.5" />
          {{ currentName }}
        </span>
        <span v-if="dirty" class="text-xs text-amber-600">● 未保存</span>
        <span v-if="busy" class="text-xs text-primary">运行中…</span>

        <div class="ml-auto flex items-center gap-2">
          <Button size="sm" :disabled="busy || !currentName" :title="`运行（${runKeyLabel}）`" @click="run">
            <Play class="h-4 w-4" />
            运行
          </Button>
          <Button v-if="busy" size="sm" variant="destructive" @click="cancel">
            <Square class="h-4 w-4" />
            中止
          </Button>
          <Button
            size="sm"
            variant="outline"
            :disabled="!currentName || !dirty"
            :title="`保存（${saveKeyLabel}）`"
            @click="save"
          >
            <Save class="h-4 w-4" />
            保存
          </Button>
          <ShortcutHelp />
          <Button
            size="sm"
            :variant="alwaysOnTop ? 'secondary' : 'ghost'"
            :title="alwaysOnTop ? '取消置顶' : '窗口置顶（方便和别的窗口并排）'"
            @click="toggleAlwaysOnTop"
          >
            <component :is="alwaysOnTop ? Pin : PinOff" class="h-4 w-4" />
            {{ alwaysOnTop ? '已置顶' : '置顶' }}
          </Button>
          <Button size="sm" variant="ghost" @click="loadList">
            <RefreshCw class="h-4 w-4" />
            刷新
          </Button>
        </div>
      </header>

      <!-- 结果 / 错误条（窄条，不挤占编辑器） -->
      <div v-if="lastOutcome" class="shrink-0 border-b bg-primary/5 px-4 py-2 text-xs">
        <span class="font-medium">{{ lastOutcome.title }}</span>
        <span class="ml-2 text-muted-foreground">{{ lastOutcome.body }}</span>
      </div>
      <div v-if="lastError" class="shrink-0 border-b bg-destructive/5 px-4 py-2 text-xs text-destructive">
        <pre class="whitespace-pre-wrap break-words">{{ lastError }}</pre>
      </div>

      <!-- 编辑器 + Console -->
      <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div class="min-h-0 flex-1 overflow-hidden">
          <ScriptEditor
            v-if="currentName"
            v-model="source"
            :language="currentLanguage"
            :script-name="currentName"
            :capabilities="capabilities"
            :namespace-names="namespaceNames"
            :readonly="busy"
            @run="run"
            @save="save"
          />
          <div v-else class="flex h-full flex-col items-center justify-center gap-2 p-8 text-sm text-muted-foreground">
            <span>左侧选择一个脚本，或新建一个</span>
            <span v-if="scriptsDir" class="text-xs">目录：<code class="rounded bg-muted px-1 py-0.5">{{ scriptsDir }}</code></span>
            <span class="text-xs">
              {{ runKeyLabel }} 运行 · {{ saveKeyLabel }} 保存 · 其它快捷键见右上角 ⌨
            </span>
          </div>
        </div>

        <ConsolePanel />
      </div>
    </div>
  </div>
</template>
