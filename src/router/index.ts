import { createRouter, createWebHashHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/pages/Dashboard.vue') },
    // 脚本有独立大窗口(label `scripting`)，见 tauri.conf.json 与 pages/ScriptsWindow.vue
    { path: '/scripting', name: 'scripting', component: () => import('@/pages/ScriptsWindow.vue') },
    // 插件管理：常驻插件的启停、状态与日志（插件能力见 plugin.md）
    { path: '/plugins', name: 'plugins', component: () => import('@/pages/Plugins.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/pages/Settings.vue') },
    // 插件窗口的宿主页：它把插件自己的 HTML 装进沙箱 iframe（见该页顶部注释）
    {
      path: '/plugin-window',
      name: 'plugin-window',
      component: () => import('@/pages/PluginWindow.vue'),
    },
    { path: '/progress', name: 'progress', component: () => import('@/pages/Progress.vue') },
    { path: '/standby', name: 'standby', component: () => import('@/pages/Standby.vue') },
  ],
})
