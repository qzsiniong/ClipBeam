import antfu from '@antfu/eslint-config'

const ignores = [
  '**/dist',
  'src/components/ui/**',
]

export default antfu({
  ignores,
  formatters: true,
  vue: true,
})
