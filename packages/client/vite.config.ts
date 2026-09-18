import { fileURLToPath, URL } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { minify } from 'html-minifier-terser'
import { defineConfig } from 'vite'
import compress from 'vite-plugin-compression2'
import { viteSingleFile } from 'vite-plugin-singlefile'

export default defineConfig({
  plugins: [
    vue(),
    tailwindcss(),
    viteSingleFile({
      useRecommendedBuildConfig: true,
      removeViteModuleLoader: true,
      deleteInlinedFiles: true,
    }),
    {
      name: 'minify-single-html',
      enforce: 'post',
      async closeBundle() {
        const fs = await import('node:fs/promises')
        const file = 'dist/index.html'
        const html = await fs.readFile(file, 'utf8')
        const out = await minify(html, {
          collapseWhitespace: true,
          removeComments: true,
          minifyCSS: true,
          minifyJS: true,
          processScripts: ['text/javascript'],
        })
        await fs.writeFile(file, out)
      },
    },
    // 对最终 HTML 再做一次 gzip/brotli 预压缩（分发用）
    compress({
      algorithms: ['gzip', 'brotliCompress'],
      deleteOriginalAssets: false,
    }),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@clipbeam/shared': fileURLToPath(new URL('../shared/src/index.ts', import.meta.url)),
    },
  },
  server: {
    port: 5174,
    strictPort: true,
  },
  build: {
    target: 'esnext',
    minify: 'terser',
    cssCodeSplit: false,
    sourcemap: false,
    reportCompressedSize: false,
    assetsInlineLimit: 1 << 30,
    chunkSizeWarningLimit: 1 << 30,
    terserOptions: {
      compress: {
        drop_console: true,
        drop_debugger: true,
        pure_funcs: ['console.log', 'console.warn', 'console.error'],
        passes: 3,
        unsafe: true,
        reduce_vars: true,
      },
      mangle: {
        toplevel: true,
        properties: false, // Vue 内部名不能乱改
      },
      format: {
        comments: false,
        preamble: '',
      },
    },
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
  },
})
