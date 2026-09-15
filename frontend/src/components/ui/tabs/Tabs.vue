<script lang="ts">
import { type InjectionKey, type Ref } from 'vue'

export const TABS_KEY: InjectionKey<Ref<string>> = Symbol('tabs')
</script>

<script setup lang="ts">
import { ref, provide } from 'vue'

const props = defineProps<{
  defaultValue?: string
}>()

const active = ref<string>(props.defaultValue || '')
provide(TABS_KEY, active)
</script>

<template>
  <div class="space-y-4">
    <slot />
  </div>
</template>
