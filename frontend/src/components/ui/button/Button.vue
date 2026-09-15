<script setup lang="ts">
import { computed } from 'vue'
import { buttonVariants, type ButtonVariants } from './index'

const props = withDefaults(defineProps<{
  variant?: ButtonVariants['variant']
  size?: ButtonVariants['size']
  disabled?: boolean
}>(), {
  variant: 'default',
  size: 'default',
  disabled: false,
})

const classes = computed(() => buttonVariants({ variant: props.variant, size: props.size }))
</script>

<template>
  <button :class="classes" :disabled="disabled">
    <slot />
  </button>
</template>
