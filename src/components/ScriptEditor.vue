<!--
  脚本编辑器：CodeMirror 6 的 Vue 包装。

  职责边界：
  * 这里只管「把 EditorView 挂到 DOM 上、同步 modelValue、切换语言、销毁清理」；
  * 高亮 / 补全 / 类型诊断的具体装配在 `@/lib/clipbeam-script/setup` 里。

  与 Vue 的双向绑定刻意做得保守：外部值变化时，只有**与文档内容不同**才 dispatch，
  否则用户每次击键都会把光标顶到行首。
-->
<script setup lang="ts">
import type { Capability } from '@/lib/clipbeam-script/autocomplete'
import { EditorView } from '@codemirror/view'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  clipbeamExtensions,
  createEditorState,
  languageCompartment,
  replaceCompletion,
} from '@/lib/clipbeam-script/clipbeam-editor'
import { clipbeamLanguage } from '@/lib/clipbeam-script/language'

const props = withDefaults(defineProps<{
  /** 当前脚本源码。 */
  modelValue: string
  /** 脚本语言：`js` 或 `ts`。 */
  language?: 'js' | 'ts'
  /** 后端返回的能力清单（`$.` 补全用）。 */
  capabilities?: Capability[]
  /** 脚本名（lint 用它判断 `.ts` / `.js`）。 */
  scriptName?: string
  /** 只读模式（运行中禁用编辑）。 */
  readonly?: boolean
}>(), {
  language: 'js',
  capabilities: () => [],
  scriptName: 'script.js',
  readonly: false,
})

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'run'): void
  (e: 'save'): void
}>()

const host = ref<HTMLDivElement | null>(null)
let view: EditorView | null = null
/** 防止「外部写入 → updateListener → 再 emit」的回环。 */
let syncing = false

onMounted(() => {
  if (!host.value)
    return

  view = new EditorView({
    parent: host.value,
    state: createEditorState(
      props.modelValue,
      clipbeamExtensions(
        props.language,
        () => props.scriptName,
        props.capabilities,
        {
          onChange: (value) => {
            if (syncing)
              return
            emit('update:modelValue', value)
          },
          onRun: () => emit('run'),
          onSave: () => emit('save'),
        },
      ),
    ),
  })
})

onBeforeUnmount(() => {
  view?.destroy()
  view = null
})

// 外部值变化：只在真的不一致时写入，避免打断光标与撤销历史
watch(() => props.modelValue, (value) => {
  if (!view || value === view.state.doc.toString())
    return
  syncing = true
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: value },
  })
  syncing = false
})

// 语言切换：只换语言扩展，保留文档与历史
watch(() => props.language, (language) => {
  view?.dispatch({
    effects: languageCompartment.reconfigure(clipbeamLanguage(language)),
  })
})

// 能力清单晚于编辑器到位时热替换补全
watch(() => props.capabilities, (capabilities) => {
  if (view)
    replaceCompletion(view, capabilities)
}, { deep: true })
</script>

<template>
  <div ref="host" class="cm-host" :class="readonly ? 'opacity-70' : ''" />
</template>

<style scoped>
.cm-host {
  height: 100%;
  overflow: hidden;
  border-radius: 0.5rem;
}
</style>
