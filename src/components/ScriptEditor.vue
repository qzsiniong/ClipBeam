<!--
  脚本编辑器：CodeMirror 6 的 Vue 包装。

  职责边界：
  * 这里只管「把 EditorView 挂到 DOM 上、同步 modelValue、切换语言、销毁清理」；
  * 高亮 / 补全 / 类型诊断的具体装配在 `@/lib/clipbeam-script/setup` 里。

  与 Vue 的双向绑定刻意做得保守：外部值变化时，只有**与文档内容不同**才 dispatch，
  否则用户每次击键都会把光标顶到行首。
-->
<script setup lang="ts">
import type { Capability, NamespaceNames } from '@/lib/clipbeam-script/autocomplete'
import type { ScriptTarget } from '@/lib/clipbeam-script/language-service'
import { EditorView } from '@codemirror/view'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  clipbeamExtensions,
  createEditorState,
  languageCompartment,
  replaceCompletion,
} from '@/lib/clipbeam-script/clipbeam-editor'
import { clipbeamLanguage } from '@/lib/clipbeam-script/language'
import { warmup } from '@/lib/clipbeam-script/language-service'

const props = withDefaults(defineProps<{
  /** 当前脚本源码。 */
  modelValue: string
  /** 脚本语言：`js` 或 `ts`。 */
  language?: 'js' | 'ts'
  /** 后端返回的能力清单（`$.` 补全用）。 */
  capabilities?: Capability[]
  /** 后端返回的命名空间名字（成员补全与全局候选都用它，前端不写死）。 */
  namespaceNames?: NamespaceNames
  /** 脚本名（lint 用它判断 `.ts` / `.js`）。 */
  scriptName?: string
  /** 只读模式（运行中禁用编辑）。 */
  readonly?: boolean
}>(), {
  language: 'js',
  capabilities: () => [],
  namespaceNames: () => ({ namespace: '', alias: '' }),
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

/**
 * 语义功能（补全/悬停/参数信息/诊断）都靠这个 getter 拿「当前脚本名 + 内容」。
 *
 * 用 getter 而不是快照：扩展在挂载时只装配一次，之后切换文件、继续打字都要看到最新值。
 */
function currentScript(): ScriptTarget {
  return { name: props.scriptName, source: props.modelValue }
}

onMounted(() => {
  if (!host.value)
    return

  // 预加载 TypeScript 语言服务：第一次 hover/补全就不用等它
  void warmup()

  view = new EditorView({
    parent: host.value,
    state: createEditorState(
      props.modelValue,
      clipbeamExtensions(
        currentScript,
        () => props.namespaceNames,
        () => props.capabilities,
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

// 语言切换（按脚本扩展名）：只换语言扩展，保留文档与历史
watch(() => props.scriptName, () => {
  view?.dispatch({
    effects: languageCompartment.reconfigure(clipbeamLanguage(currentScript().name)),
  })
})

// 能力清单 / 命名空间名字晚于编辑器到位时热替换补全
watch([() => props.capabilities, () => props.namespaceNames], () => {
  if (view)
    replaceCompletion(view, currentScript, () => props.namespaceNames, () => props.capabilities)
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
