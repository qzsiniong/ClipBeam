import antfu from '@antfu/eslint-config'

const ignores = [
  '**/dist',
  'src/components/ui/**',
  'src-tauri/src/**',
  'src-tauri/target/**',
  'src-tauri/gen/**',
  'web/clipbeam.html',
  'README.md',
]

export default antfu({
  ignores,
  formatters: true,
  vue: true,
})
