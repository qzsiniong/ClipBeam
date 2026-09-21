import { createPinia } from 'pinia'
import { createApp } from 'vue'
// 浏览器调试用的 Tauri mock：必须先于 App 装好（App.vue 在 setup 里读窗口 label）。
// 真 Tauri 环境与生产构建下这个调用立刻返回，且 mock 的实现不会进生产包。
import { installTauriMockInBrowser } from '@/dev/tauri-mock'
import App from './App.vue'
import { router } from './router'
import './style.css'

async function start() {
  await installTauriMockInBrowser().catch((error: unknown) => {
    console.warn('[clipbeam] 浏览器 mock 加载失败，页面可能无法正常工作：', error)
  })
  createApp(App).use(createPinia()).use(router).mount('#app')
}

void start()
