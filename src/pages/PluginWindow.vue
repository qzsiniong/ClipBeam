<!--
  插件窗口的宿主页（`index.html#/plugin-window?label=…&plugin=…&page=…`）。

  ## 它为什么存在

  插件窗口加载的不是「插件自己的 HTML」，而是**应用自己的这一页**；这一页再把插件页面放进
  一个 `<iframe sandbox="allow-scripts">` 里。这么绕一圈只有一个目的：

  > 插件页面跑在**沙箱 iframe** 里，拿不到 `window.__TAURI_INTERNALS__`，
  > 因此无法直接调用宿主的 IPC。

  它只能通过 `postMessage` 说话，而这一页是唯一的翻译官。于是「插件能做什么」完全由
  `$plugin` 的能力与权限决定，而不是「它拿到了一个万能对象」。

  ## 两个方向的通信

  | 方向 | 路径 |
  |---|---|
  | 插件 → 应用 | 插件 `window.parent.postMessage(payload)` → 本页校验来源 → `invoke('plugin_window_message')` |
  | 应用 → 插件 | Rust `emit_to(label, 'plugin-window-message')` → 本页 `iframe.contentWindow.postMessage(payload)` |

  插件侧看到的 API（约定，写在 plugin.md 与示例里）：

  ```js
  window.addEventListener('message', (event) => { /* event.data = 载荷 */ })
  window.parent.postMessage({ hello: 'world' }, '*')
  ```

  ## 为什么用 `event.source` 而不是 `event.origin` 校验来源

  沙箱 iframe（不带 `allow-same-origin`）的 origin 是 **`"null"`**，
  所以按 origin 白名单校验在这里行不通 —— 任何一个 `data:`/`blob:` 页面都能声称自己是它。
  比对 `event.source` 与 `iframe.contentWindow` 是同一件事的准确做法。
-->
<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { onMounted, onUnmounted, ref } from 'vue'
import { useRoute } from 'vue-router'

const route = useRoute()

/** 窗口标签：Rust 侧用它路由「这条消息该给哪个插件」。 */
const label = String(route.query.label ?? '')
/** 插件 id（拼协议 URL 用）。 */
const pluginId = String(route.query.plugin ?? '')
/** 插件页面（相对插件目录；查询串里是编码过的）。 */
const page = String(route.query.page ?? 'index.html')

const iframe = ref<HTMLIFrameElement | null>(null)
const unlistens: UnlistenFn[] = []
/** 初始化失败（参数缺失）时的提示。 */
const problem = ref('')

/** 插件页面在自定义协议里的地址。 */
function pluginUrl(): string {
  const encoded = page
    .split('/')
    .map(segment => encodeURIComponent(segment))
    .join('/')
  return `clipbeam-plugin://localhost/p-${encodeURIComponent(pluginId)}/${encoded}`
}

/** 插件 → 应用。 */
async function forwardToPlugin(payload: unknown) {
  try {
    await invoke('plugin_window_message', { label, message: payload })
  }
  catch (error) {
    // 窗口已经不在册（例如插件刚被停用）：忽略即可，这条消息已经无处可去
    console.warn('[clipbeam] 转发插件消息失败：', error)
  }
}

/** 应用 → 插件。 */
function deliverToPage(payload: unknown) {
  const target = iframe.value?.contentWindow
  if (!target)
    return
  // 目标 origin 只能是 `*`：沙箱 iframe 的 origin 是 "null"，写死别的值反而送不到
  target.postMessage(payload, '*')
}

function onWindowMessage(event: MessageEvent) {
  // 只认来自我们自己那个 iframe 的消息（沙箱页面的 origin 是 "null"，不能按 origin 判）
  if (!iframe.value || event.source !== iframe.value.contentWindow)
    return
  void forwardToPlugin(event.data)
}

onMounted(async () => {
  if (!label || !pluginId) {
    problem.value = '窗口参数缺失（label / plugin）：这不是一个由插件打开的窗口。'
    return
  }

  window.addEventListener('message', onWindowMessage)

  // 应用 → 插件
  unlistens.push(
    await listen<unknown>('plugin-window-message', (event) => {
      deliverToPage(event.payload)
    }),
  )

  // 窗口要消失了：告诉 Rust 把登记摘掉（幂等，重复调用没关系）
  window.addEventListener('beforeunload', () => {
    void invoke('plugin_window_closed', { label })
  })
})

onUnmounted(() => {
  window.removeEventListener('message', onWindowMessage)
  unlistens.forEach(fn => fn())
})
</script>

<template>
  <div class="h-screen w-screen overflow-hidden bg-background">
    <!-- 参数不对：说清原因，而不是白屏 -->
    <div v-if="problem" class="flex h-full items-center justify-center p-6 text-center text-sm text-muted-foreground">
      {{ problem }}
    </div>

    <!--
      插件页面。sandbox 只给 allow-scripts：
      * 不给 allow-same-origin → 它碰不到宿主的存储与 IPC（origin 变成 "null"）；
      * 不给 allow-forms / allow-top-navigation → 它不能把整个窗口导航走。
    -->
    <iframe
      v-else
      ref="iframe"
      :src="pluginUrl()"
      class="h-full w-full border-0"
      sandbox="allow-scripts"
      referrerpolicy="no-referrer"
    />
  </div>
</template>
