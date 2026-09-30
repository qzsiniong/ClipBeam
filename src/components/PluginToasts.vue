<!--
  插件 toast 宿主：应用内提示的渲染处。

  为什么要宿主组件而不是让插件自己画：插件跑在 JS 引擎里（没有 DOM），
  它只能「请求提示」（`$plugin.toast`），落点由宿主决定 —— 就是这个组件。
  事件由 Rust 侧发（`plugin-toast`，见 `src-tauri/src/plugin_host.rs`）。

  约定：
  * 右下角堆叠，最多同时 5 条（多的按时间淘汰最旧的）；
  * `durationMs` 决定自动消失时间；传 0 表示不自动消失（用户手动点掉）；
  * `level` 决定配色（与脚本 Console 面板同一套等级直觉）。

  只在主窗口与脚本窗口渲染（进度/待命是浮动小窗，见 App.vue 的 `bareWindow`）。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { listen } from '@tauri-apps/api/event'
import { AlertTriangle, CheckCircle2, Info, X, XCircle } from 'lucide-vue-next'
import { onMounted, onUnmounted, ref } from 'vue'

type ToastLevel = 'info' | 'success' | 'warning' | 'error'

/** `plugin-toast` 事件的载荷（镜像 Rust 侧 `PluginToast`）。 */
interface PluginToastPayload {
  plugin_id: string
  plugin_name: string
  level: ToastLevel
  message: string
  duration_ms: number | null
}

interface ActiveToast extends PluginToastPayload {
  /** 本地唯一 id（同一条消息弹两次也要能分开）。 */
  key: number
}

/** 最少能看到多久：插件传了 0 或很小的值时不至于一闪而过。 */
const MIN_DURATION_MS = 1500
/** 没传 durationMs 时的默认停留时间。 */
const DEFAULT_DURATION_MS = 4000
/** 同时最多显示几条。 */
const MAX_VISIBLE = 5

const toasts = ref<ActiveToast[]>([])
const unlistens: UnlistenFn[] = []
const timers = new Map<number, number>()
let nextKey = 1

/** 配色：与脚本 Console 面板的等级配色保持同一套直觉。 */
const styles: Record<ToastLevel, { border: string, icon: unknown, tone: string }> = {
  info: { border: 'border-l-primary', icon: Info, tone: 'text-primary' },
  success: { border: 'border-l-green-500', icon: CheckCircle2, tone: 'text-green-600' },
  warning: { border: 'border-l-amber-500', icon: AlertTriangle, tone: 'text-amber-600' },
  error: { border: 'border-l-red-500', icon: XCircle, tone: 'text-red-600' },
}

function dismiss(key: number) {
  toasts.value = toasts.value.filter(toast => toast.key !== key)
  const timer = timers.get(key)
  if (timer !== undefined) {
    clearTimeout(timer)
    timers.delete(key)
  }
}

function show(payload: PluginToastPayload) {
  const key = nextKey++
  toasts.value = [...toasts.value, { ...payload, key }]

  // 超出上限时淘汰最旧的（它多半已经看不完了）
  while (toasts.value.length > MAX_VISIBLE) {
    dismiss(toasts.value[0].key)
  }

  const duration = payload.duration_ms ?? DEFAULT_DURATION_MS
  if (duration > 0) {
    timers.set(key, window.setTimeout(dismiss, Math.max(duration, MIN_DURATION_MS), key))
  }
}

/** 手动弹一条（浏览器 mock 与调试用：`__CLIPBEAM_DEV__.emitPluginToast()`）。 */
defineExpose({ show })

onMounted(async () => {
  unlistens.push(
    await listen<PluginToastPayload>('plugin-toast', event => show(event.payload)),
  )
})

onUnmounted(() => {
  unlistens.forEach(fn => fn())
  timers.forEach(timer => clearTimeout(timer))
  timers.clear()
})
</script>

<template>
  <div class="pointer-events-none fixed bottom-4 right-4 z-50 flex w-80 flex-col gap-2">
    <div
      v-for="toast in toasts"
      :key="toast.key"
      class="pointer-events-auto flex items-start gap-2 rounded-md border border-l-4 bg-background/95 px-3 py-2 shadow-lg backdrop-blur"
      :class="styles[toast.level].border"
    >
      <component
        :is="styles[toast.level].icon"
        class="mt-0.5 h-4 w-4 shrink-0"
        :class="styles[toast.level].tone"
      />
      <div class="min-w-0 flex-1">
        <div class="text-xs font-medium text-muted-foreground">
          {{ toast.plugin_name }}
        </div>
        <div class="break-words text-sm">
          {{ toast.message }}
        </div>
      </div>
      <button
        type="button"
        class="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
        title="关闭"
        @click="dismiss(toast.key)"
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  </div>
</template>
