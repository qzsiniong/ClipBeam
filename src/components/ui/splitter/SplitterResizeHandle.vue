<script setup lang="ts">
import type { SplitterResizeHandleEmits, SplitterResizeHandleProps } from "reka-ui"
import type { HTMLAttributes } from "vue"
import { reactiveOmit } from "@vueuse/core"
import { SplitterResizeHandle, useForwardPropsEmits } from "reka-ui"
import { cn } from "@/lib/utils"

defineOptions({
  inheritAttrs: false,
})

const props = defineProps<SplitterResizeHandleProps & { class?: HTMLAttributes["class"] }>()
const emits = defineEmits<SplitterResizeHandleEmits>()

const delegatedProps = reactiveOmit(props, "class")

const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <SplitterResizeHandle
    data-slot="splitter-resize-handle"
    v-bind="{ ...$attrs, ...forwarded }"
    :class="cn('relative z-10 h-1.5 shrink-0 cursor-row-resize border-t bg-background transition-colors hover:bg-primary/40 data-[state=drag]:bg-primary', props.class)"
  >
    <slot />
  </SplitterResizeHandle>
</template>
