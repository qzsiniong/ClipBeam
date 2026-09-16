import { createRouter, createWebHashHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/pages/Dashboard.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/pages/Settings.vue') },
    { path: '/progress', name: 'progress', component: () => import('@/pages/Progress.vue') },
  ],
})
