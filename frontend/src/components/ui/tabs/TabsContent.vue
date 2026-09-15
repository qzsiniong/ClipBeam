<script setup lang="ts">
import { inject, computed, type Ref } from 'vue'
import { TABS_KEY } from './Tabs.vue'

const props = defineProps<{
  value: string
}>()

const active = inject(TABS_KEY) as Ref<string>
const show = computed(() => active.value === props.value)
</script>

<template>
  <div v-if="show">
    <slot />
  </div>
</template>
