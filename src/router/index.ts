import { createRouter, createWebHashHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/pages/Dashboard.vue') },
    // 脚本有独立大窗口(label `scripting`)，见 tauri.conf.json 与 pages/ScriptsWindow.vue
    { path: '/scripting', name: 'scripting', component: () => import('@/pages/ScriptsWindow.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/pages/Settings.vue') },
    { path: '/progress', name: 'progress', component: () => import('@/pages/Progress.vue') },
    { path: '/standby', name: 'standby', component: () => import('@/pages/Standby.vue') },
  ],
})
