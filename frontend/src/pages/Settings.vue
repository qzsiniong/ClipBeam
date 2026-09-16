<script setup lang="ts">
import type { Config } from '@/stores/config'
import { listen } from '@tauri-apps/api/event'
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
  }
  catch (e) {
    errors.value = [String(e)]
  }
}

function cancel() {
  if (store.config)
    draft.value = { ...store.config }
  errors.value = []
}
</script>

<template>
  <div>
    <div class="">
      <div class="mb-6">
        <h1 class="text-2xl font-bold mb-1">
          ClipBeam 设置
        </h1>
        <p class="text-sm text-muted-foreground">
          热键与传输参数配置
        </p>
      </div>

      <div v-if="!draft" class="text-center py-12 text-muted-foreground">
        加载中…
      </div>

      <div v-else>
        <Tabs default-value="hotkeys">
          <TabsList class="grid grid-cols-2 w-full">
            <TabsTrigger value="hotkeys">
              热键
            </TabsTrigger>
            <TabsTrigger value="timing">
              时序与阈值
            </TabsTrigger>
          </TabsList>

          <TabsContent value="hotkeys">
            <Card>
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
            <Card>
              <CardHeader>
                <CardTitle>时序与阈值</CardTitle>
                <CardDescription>键盘逐键时序、二维码接收超时与文本大小上限</CardDescription>
              </CardHeader>
              <CardContent>
                <div class="grid grid-cols-2 gap-6">
                  <div class="space-y-2">
                    <Label>逐键间隔 (ms)</Label>
                    <Input
                      v-model="draft.key_delay_ms"
                      type="number"
                      :min="0"
                      :max="100"
                    />
                  </div>
                  <div class="space-y-2">
                    <Label>settle 等待 (ms)</Label>
                    <Input
                      v-model="draft.settle_ms"
                      type="number"
                      :min="0"
                      :max="5000"
                    />
                  </div>
                  <div class="space-y-2">
                    <Label>二维码接收超时 (秒)</Label>
                    <Input
                      v-model="draft.receive_timeout_s"
                      type="number"
                      :min="5"
                      :max="3600"
                    />
                  </div>
                  <div class="space-y-2">
                    <Label>文本大小上限 (KB)</Label>
                    <Input
                      v-model="draft.max_text_kb"
                      type="number"
                      :min="1"
                      :max="10240"
                    />
                  </div>
                </div>
              </CardContent>
            </Card>
          </TabsContent>
        </Tabs>

        <div v-if="errors.length" class="mt-4 space-y-2">
          <Badge v-for="e in errors" :key="e" variant="destructive">
            {{ e }}
          </Badge>
        </div>

        <div v-if="savedToast" class="mt-4">
          <Badge variant="success">
            ✓ 设置已保存，热键已重注册
          </Badge>
        </div>

        <div class="mt-6 flex justify-end gap-3">
          <Button variant="outline" @click="cancel">
            取消
          </Button>
          <Button :disabled="!canSave" @click="save">
            保存
          </Button>
        </div>
      </div>
    </div>
  </div>
</template>
