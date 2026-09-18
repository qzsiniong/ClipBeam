<script setup lang="ts">
import type { Config } from '@/stores/config'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onMounted, ref } from 'vue'
import HotkeyCapture from '@/components/HotkeyCapture.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useConfigStore } from '@/stores/config'

const store = useConfigStore()
const draft = ref<Config | null>(null)
const errors = ref<string[]>([])
const savedToast = ref(false)

onMounted(async () => {
  await store.load()
  draft.value = { ...store.config! }
  await listen('config-saved', () => {
    savedToast.value = true
    setTimeout(() => {
      savedToast.value = false
    }, 2000)
  })
})

const canSave = computed(() => {
  if (!draft.value)
    return false
  const cfg = draft.value
  if (!cfg.send_hotkey.trim() || !cfg.recv_hotkey.trim() || !cfg.stop_hotkey.trim())
    return false
  if (cfg.send_hotkey === cfg.recv_hotkey || cfg.send_hotkey === cfg.stop_hotkey || cfg.recv_hotkey === cfg.stop_hotkey)
    return false
  if (cfg.key_delay_ms > 100)
    return false
  if (cfg.settle_ms > 5000)
    return false
  if (cfg.receive_timeout_s < 5 || cfg.receive_timeout_s > 3600)
    return false
  if (cfg.max_text_kb < 1 || cfg.max_text_kb > 10240)
    return false
  return true
})

async function save() {
  if (!draft.value)
    return
  errors.value = []
  try {
    await store.save(draft.value)
    await getCurrentWindow().hide()
  }
  catch (e) {
    errors.value = [String(e)]
  }
}

async function cancel() {
  if (store.config)
    draft.value = { ...store.config }
  errors.value = []
  await getCurrentWindow().hide()
}
</script>

<template>
  <div class="flex flex-col gap-6 pb-20">
    <Tabs v-if="draft" default-value="hotkeys">
      <TabsList class="grid grid-cols-2 w-full h-9">
        <TabsTrigger value="hotkeys" class="text-sm font-medium">
          热键
        </TabsTrigger>
        <TabsTrigger value="timing" class="text-sm font-medium">
          时序与阈值
        </TabsTrigger>
      </TabsList>

      <TabsContent value="hotkeys">
        <Card class="rounded-xl shadow-sm">
          <CardHeader>
            <CardTitle>全局热键</CardTitle>
            <CardDescription>任意焦点下生效，保存后立即重注册</CardDescription>
          </CardHeader>
          <CardContent class="space-y-4">
            <HotkeyCapture
              v-model="draft.send_hotkey"
              label="发送（宿主机→远程）"
            />
            <Separator />
            <HotkeyCapture
              v-model="draft.recv_hotkey"
              label="接收（远程→宿主机）"
            />
            <Separator />
            <HotkeyCapture
              v-model="draft.stop_hotkey"
              label="中止（任务运行时生效）"
            />
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="timing">
        <Card class="rounded-xl shadow-sm">
          <CardHeader>
            <CardTitle>时序与阈值</CardTitle>
            <CardDescription>键盘逐键时序、二维码接收超时与文本大小上限</CardDescription>
          </CardHeader>
          <CardContent>
            <div class="grid grid-cols-2 gap-6">
              <div class="space-y-1.5">
                <Label class="text-sm font-medium">逐键间隔 (ms)</Label>
                <Input
                  v-model="draft.key_delay_ms"
                  type="number"
                  :min="0"
                  :max="100"
                  class="h-9 appearance-none text-right"
                />
              </div>
              <div class="space-y-1.5">
                <Label class="text-sm font-medium">settle 等待 (ms)</Label>
                <Input
                  v-model="draft.settle_ms"
                  type="number"
                  :min="0"
                  :max="5000"
                  class="h-9 appearance-none text-right"
                />
              </div>
              <div class="space-y-1.5">
                <Label class="text-sm font-medium">二维码接收超时 (秒)</Label>
                <Input
                  v-model="draft.receive_timeout_s"
                  type="number"
                  :min="5"
                  :max="3600"
                  class="h-9 appearance-none text-right"
                />
              </div>
              <div class="space-y-1.5">
                <Label class="text-sm font-medium">文本大小上限 (KB)</Label>
                <Input
                  v-model="draft.max_text_kb"
                  type="number"
                  :min="1"
                  :max="10240"
                  class="h-9 appearance-none text-right"
                />
              </div>
            </div>

            <Separator class="my-4" />

            <!-- 压缩开关单独一行 -->
            <div class="flex items-center justify-between rounded-lg border p-3">
              <div class="space-y-0.5">
                <Label class="text-sm font-medium">启用 zstd 压缩发送（协议 A）</Label>
                <p class="text-xs text-muted-foreground">
                  关闭后使用未压缩帧，便于对比传输耗时
                </p>
              </div>
              <input
                v-model="draft.compress"
                type="checkbox"
                class="h-4 w-4 rounded border-border"
              >
            </div>

            <Separator class="my-4" />

            <div class="flex items-center justify-between rounded-lg border p-3">
              <div class="space-y-0.5">
                <Label class="text-sm font-medium">启用按键真实发送</Label>
                <p class="text-xs text-muted-foreground">
                  关闭后按键不发送真实字符
                </p>
              </div>
              <input
                v-model="draft.send_real_keys"
                type="checkbox"
                class="h-4 w-4 rounded border-border"
              >
            </div>

            <Separator class="my-4" />

            <!-- 进度显示方式 -->
            <div class="rounded-lg border p-3 space-y-2">
              <Label class="text-sm font-medium">进度显示方式</Label>
              <div class="flex gap-2">
                <button
                  v-for="opt in [
                    { value: 'floating', label: '悬浮条' },
                    { value: 'tray', label: '托盘图标' },
                    { value: 'both', label: '两者同时' },
                  ]"
                  :key="opt.value"
                  class="px-3 py-1.5 rounded-md text-xs font-medium border transition-colors"
                  :class="draft.progress_display === opt.value
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-border hover:bg-accent'"
                  @click="draft.progress_display = opt.value as any"
                >
                  {{ opt.label }}
                </button>
              </div>
              <p class="text-xs text-muted-foreground">
                悬浮条:右上角不抢焦点的进度条;托盘图标:状态栏图标动态变化
              </p>
            </div>
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>

    <div v-if="errors.length" class="space-y-2">
      <Badge v-for="e in errors" :key="e" variant="destructive">
        {{ e }}
      </Badge>
    </div>
  </div>

  <!-- Sticky footer:保存状态 + 操作按钮 -->
  <div class="fixed bottom-0 left-60 right-0 border-t bg-background/95 backdrop-blur px-6 py-4 flex items-center justify-between gap-3">
    <div>
      <Badge v-if="savedToast" variant="success">
        ✓ 设置已保存，热键已重注册
      </Badge>
    </div>
    <div class="flex gap-2">
      <Button variant="ghost" @click="cancel">
        取消
      </Button>
      <Button :disabled="!canSave" @click="save">
        保存
      </Button>
    </div>
  </div>
</template>
