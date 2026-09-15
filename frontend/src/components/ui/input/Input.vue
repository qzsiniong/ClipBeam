<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  modelValue?: string | number
  type?: string
  min?: number | string
  max?: number | string
  step?: number | string
  placeholder?: string
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [string | number]
}>()

const value = computed({
  get: () => props.modelValue,
  set: (v) => emit('update:modelValue', v as string | number),
})
</script>

<template>
  <input
    v-model="value"
    :type="type || 'text'"
    :min="min"
    :max="max"
    :step="step"
    :placeholder="placeholder"
    :disabled="disabled"
    class="flex h-9 w-full rounded-md border border-[var(--input)] bg-transparent px-3 py-1 text-sm shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-[var(--muted-foreground)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--ring)] disabled:cursor-not-allowed disabled:opacity-50"
  />
</template>
