import { fileURLToPath, URL } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  // Tauri dev server 端口固定为 5173(对应 tauri.conf.json devUrl)
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ['src-tauri/**'] },
  },
  clearScreen: false,
})
