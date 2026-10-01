import { fileURLToPath, URL } from 'node:url'
import { minify } from 'html-minifier-terser'
import { defineConfig } from 'vite'
import compress from 'vite-plugin-compression2'
import { viteSingleFile } from 'vite-plugin-singlefile'

export default defineConfig({
  plugins: [
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
      '@clipbeam/shared': fileURLToPath(new URL('../shared/src/index.ts', import.meta.url)),
    },
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
        // 只丢弃 log/info/warn/error，**专门留下 `console.debug`**。
        //
        // 为什么：部署到远程的正是这份压缩产物（协议 C 内嵌 dist/index.html），
        // 而接收页的调试输出走 `console.debug`（见 packages/shared/src/debug.ts）。
        // 用 `drop_console: true` 会把包括 debug 在内的所有 console 调用一起删掉，
        // 于是「在远程页面上打开调试」这件事根本不可能 —— 加了调用也看不见。
        //
        // 留下的 debug 默认不输出：它由 localStorage/URL 开关控制（默认关），
        // 所以正常使用时控制台依然是干净的。
        drop_console: ['log', 'info', 'warn', 'error'],
        drop_debugger: true,
        pure_funcs: ['console.log', 'console.info', 'console.warn', 'console.error'],
        passes: 3,
        unsafe: true,
        reduce_vars: true,
      },
      mangle: {
        toplevel: true,
        properties: false,
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
