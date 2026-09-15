import { createRouter, createWebHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/pages/Dashboard.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/pages/Settings.vue') },
  ],
})
