<script setup lang="ts">
import { Keyboard } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import InboundCard from './components/InboundCard.vue'
import OutboundCard from './components/OutboundCard.vue'
</script>

<template>
  <div class="flex min-h-screen flex-col items-center px-4 py-10">
    <div class="w-full max-w-2xl space-y-5">
      <header class="space-y-3">
        <div class="flex items-center gap-2.5">
          <div class="flex size-9 items-center justify-center rounded-lg bg-sky-500/15 text-sky-400">
            <Keyboard class="size-5" />
          </div>
          <h1 class="text-xl font-semibold tracking-tight">
            ClipBeam 剪贴板桥接
          </h1>
          <Badge variant="outline" class="border-amber-500/40 text-amber-400">
            US/QWERTY
          </Badge>
        </div>
        <p class="text-sm leading-6 text-muted-foreground">
          宿主机 ↔ 远程浏览器 · 无网络通道 · 键盘 / 二维码。接收前请<b class="text-foreground">点击本页空白处</b>
          让焦点落在页面上；键盘通道要求远程会话为 US/QWERTY 布局。
        </p>
      </header>
      <InboundCard />
      <OutboundCard />
    </div>
  </div>
</template>
