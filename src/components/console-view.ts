/**
 * Console 面板的三种形态（由 `ScriptsWindow` 持有，`ConsolePanel` 只消费）。
 *
 * 单独放一个模块而不是从 `.vue` 里 `export type`：SFC 的 `<script setup>` 不允许 export，
 * 而额外加一个普通 `<script>` 块会让 eslint 的 `import/first` 认为后面的 import 不在顶部。
 */
export type ConsoleView = 'normal' | 'collapsed' | 'maximized'
