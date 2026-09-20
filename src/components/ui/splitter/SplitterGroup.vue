<script setup lang="ts">
import type { SplitterGroupEmits, SplitterGroupProps } from "reka-ui"
import { SplitterGroup, useForwardPropsEmits } from "reka-ui"

const props = defineProps<SplitterGroupProps>()
const emits = defineEmits<SplitterGroupEmits>()

const forwarded = useForwardPropsEmits(props, emits)
</script>

<template>
  <SplitterGroup
    v-slot="slotProps"
    data-slot="splitter-group"
    v-bind="forwarded"
  >
    <slot v-bind="slotProps" />
  </SplitterGroup>
</template>
