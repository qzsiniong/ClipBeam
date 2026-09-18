<script setup lang="ts">
import type { ErrorCorrection } from 'qr'
import encodeQR from 'qr'
import { computed } from 'vue'

const props = defineProps<{ text: string }>()

// 与单文件版一致：M 级纠错 + 4 模块 quiet zone，保证宿主机截屏识别率
const ECC: ErrorCorrection = 'medium'
const svg = computed(() =>
  props.text ? encodeQR(props.text, 'svg', { ecc: ECC, border: 4 }) : '',
)
</script>

<template>
  <div class="qr-svg" v-html="svg" />
</template>
