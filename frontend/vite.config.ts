import path from 'node:path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') },
  },
  // Tauri dev server 端口固定为 5173(对应 tauri.conf.json devUrl)
  server: { port: 5173, strictPort: true },
  clearScreen: false,
})
