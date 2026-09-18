<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { onUnmounted, ref } from 'vue'
import { Button } from '@/components/ui/button'

const _props = defineProps<{
  label: string
  modelValue: string
}>()

const emit = defineEmits<{
  'update:modelValue': [string]
}>()

const capturing = ref(false)

function onKeyDown(e: KeyboardEvent) {
  e.preventDefault()
  e.stopPropagation()
  // 忽略单独的修饰键按下
  if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key))
    return

  const mods = {
    ctrl: e.ctrlKey,
    shift: e.shiftKey,
    alt: e.altKey,
    super_: e.metaKey,
  }
  invoke<string>('capture_hotkey', { code: e.code, mods })
    .then((spec) => {
      emit('update:modelValue', spec)
    })
    .catch(() => {
      // 校验失败,保留原值
    })
    .finally(() => {
      stopCapture()
    })
}

function startCapture() {
  if (capturing.value) {
    stopCapture()
    return
  }
  capturing.value = true
  window.addEventListener('keydown', onKeyDown, true)
}

function stopCapture() {
  capturing.value = false
  window.removeEventListener('keydown', onKeyDown, true)
}

onUnmounted(() => {
  window.removeEventListener('keydown', onKeyDown, true)
})
</script>

<template>
  <div class="flex items-center justify-between gap-4">
    <span class="text-sm font-medium">{{ label }}</span>
    <Button
      :variant="capturing ? 'secondary' : 'outline'"
      class="min-w-55 justify-start font-mono"
      @click="startCapture"
    >
      {{ capturing ? '按下新组合键…（再次点击取消）' : (modelValue || '未设置') }}
    </Button>
  </div>
</template>
