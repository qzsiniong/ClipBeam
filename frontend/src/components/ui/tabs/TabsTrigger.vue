<script setup lang="ts">
import { inject, type Ref } from 'vue'
import { TABS_KEY } from './Tabs.vue'

const props = defineProps<{
  value: string
}>()

const active = inject(TABS_KEY) as Ref<string>

function activate() {
  active.value = props.value
}
</script>

<template>
  <button
    class="inline-flex items-center justify-center whitespace-nowrap rounded-md px-3 py-1 text-sm font-medium transition-all focus-visible:outline-none cursor-pointer"
    :class="active === value
      ? 'bg-[var(--background)] text-[var(--foreground)] shadow'
      : 'text-[var(--muted-foreground)] hover:text-[var(--foreground)]'"
    @click="activate"
  >
    <slot />
  </button>
</template>
