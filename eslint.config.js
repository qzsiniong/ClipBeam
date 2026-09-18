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
]

export default antfu({
  ignores,
  formatters: true,
  vue: true,
})
