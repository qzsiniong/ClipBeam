<script setup lang="ts">
withDefaults(defineProps<{ orientation?: 'horizontal' | 'vertical' }>(), {
  orientation: 'horizontal',
})
</script>

<template>
  <div
    :class="[
      'shrink-0 bg-[var(--border)]',
      orientation === 'horizontal' ? 'h-px w-full' : 'h-full w-px',
    ]"
    role="separator"
  />
</template>
