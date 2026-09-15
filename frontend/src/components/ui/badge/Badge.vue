<script setup lang="ts">
import { computed } from 'vue'
import { badgeVariants, type BadgeVariants } from './index'

const props = withDefaults(defineProps<{
  variant?: BadgeVariants['variant']
}>(), {
  variant: 'default',
})

const classes = computed(() => badgeVariants({ variant: props.variant }))
</script>

<template>
  <div :class="classes">
    <slot />
  </div>
</template>
