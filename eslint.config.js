import antfu from '@antfu/eslint-config'

const ignores = [
  '**/dist/**',
  '**/*.tsbuildinfo',
  'src/components/ui/**',
  'packages/client/src/components/ui/**',
  'src-tauri/src/**',
  'src-tauri/target/**',
  'src-tauri/gen/**',
  '.trae/**',
  'README.md',
  // Rust crate 里的脚本是「内嵌示例」与引擎前置脚本，不走前端 lint：
  // - crates/script-engine/src/prelude.js：引擎前置脚本（与用户脚本同源）
  // - crates/clipbeam-scripting/seed/*：内置示例脚本
  'crates/**',
]

export default antfu({
  ignores,
  formatters: true,
  vue: true,
})
