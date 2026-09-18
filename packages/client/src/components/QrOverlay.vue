<script setup lang="ts">
import QrSvg from './QrSvg.vue'

defineProps<{ qrText: string, tip: string }>()
defineEmits<{ close: [] }>()
</script>

<template>
  <Teleport to="body">
    <div class="qr-overlay" @click="$emit('close')">
      <div class="qr-card">
        <QrSvg :text="qrText" />
      </div>
      <div class="text-[15px] tracking-wide text-[#cdd6ea]">
        {{ tip }}
      </div>
    </div>
  </Teleport>
</template>
