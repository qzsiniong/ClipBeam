<script setup lang="ts">
import type { ReceiverStatusClass } from '@clipbeam/shared'
import {
  buildFrames,
  clamp,
  readClipboard,

} from '@clipbeam/shared'
import { Play, QrCode, Square } from 'lucide-vue-next'
import { computed, onUnmounted, ref } from 'vue'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import QrOverlay from './QrOverlay.vue'

const outCls = ref<ReceiverStatusClass>('')
const outMsg = ref('未生成')
const pasteShow = ref(false)
const pasteText = ref('')
const advShow = ref(false)
const chunkSize = ref<number | string>(800)
const intervalMs = ref<number | string>(300)

const playing = ref(false)
const frames = ref<string[]>([])
const idx = ref(0)
const total = ref(0)
let playTimer: number | null = null

const qrText = computed(() => frames.value[idx.value] ?? '')
const qrTip = computed(() => `第 ${idx.value + 1} / ${total.value} 帧 · 点击任意处关闭`)

const statusColor: Record<ReceiverStatusClass, string> = {
  '': 'text-muted-foreground',
  'run': 'text-sky-400',
  'ok': 'text-emerald-400',
  'err': 'text-red-400',
  'warn': 'text-amber-400',
}

function setOut(cls: ReceiverStatusClass, msg: string) {
  outCls.value = cls
  outMsg.value = msg
}

function startPlay(text: string) {
  stopPlay(true)
  if (!text) {
    setOut('err', '文本为空')
    return
  }
  try {
    const built = buildFrames(text, clamp(chunkSize.value, 200, 1500))
    frames.value = built.frames
    total.value = built.total
  }
  catch (ex) {
    setOut('err', `编码失败: ${(ex as Error).message}`)
    return
  }
  idx.value = 0
  playing.value = true
  setOut('run', `播放中：第 1 / ${total.value} 帧。请在宿主机按接收热键`)
  // 单帧无需轮播
  if (total.value > 1) {
    playTimer = window.setInterval(() => {
      idx.value = (idx.value + 1) % total.value
      setOut('run', `播放中：第 ${idx.value + 1} / ${total.value} 帧。请在宿主机按接收热键`)
    }, clamp(intervalMs.value, 300, 3000))
  }
}

function stopPlay(silent: boolean) {
  if (playTimer !== null) {
    clearInterval(playTimer)
    playTimer = null
  }
  playing.value = false
  if (!silent && total.value)
    setOut('warn', `已停止播放（共 ${total.value} 帧）`)
}

function onReadClipboard() {
  readClipboard().then(startPlay, (ex: unknown) => {
    pasteShow.value = true
    const msg = ex instanceof Error && ex.message.includes('不支持')
      ? '浏览器不支持剪贴板读取，请粘贴文本后用下方按钮生成。'
      : '读取被拒绝（或权限未授予），请粘贴文本后用下方按钮生成。'
    setOut('warn', msg)
  })
}

onUnmounted(() => {
  if (playTimer !== null)
    clearInterval(playTimer)
})
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>
        ② 发送出站（远程 → 宿主机）
      </CardTitle>
      <CardDescription>
        读取远程剪贴板 → 编码为二维码循环播放；在宿主机按接收热键截屏收包。
      </CardDescription>
    </CardHeader>
    <CardContent class="space-y-4">
      <div class="flex flex-wrap gap-2.5">
        <Button @click="onReadClipboard">
          <QrCode />
          读取远程剪贴板并生成二维码
        </Button>
        <Button variant="secondary" :disabled="!playing" @click="stopPlay(false)">
          <Square class="size-3.5" />
          停止播放
        </Button>
      </div>
      <div v-if="pasteShow" class="space-y-2">
        <Textarea
          v-model="pasteText"
          placeholder="若读取被浏览器拒绝，把远程剪贴板内容粘贴到这里，再点下方按钮"
          class="min-h-24 resize-y text-xs"
        />
        <Button size="sm" @click="startPlay(pasteText)">
          <Play class="size-3.5" />
          用上方文本生成二维码
        </Button>
      </div>
      <Label class="cursor-pointer text-muted-foreground">
        <Checkbox v-model="advShow" />
        高级：调整帧长 / 播放间隔
      </Label>
      <div v-if="advShow" class="flex flex-wrap items-center gap-x-6 gap-y-3 text-xs text-muted-foreground">
        <div class="flex items-center gap-2">
          <span>每帧字符数</span>
          <Input
            v-model.number="chunkSize"
            type="number"
            min="200"
            max="1500"
            step="50"
            class="h-8 w-24"
          />
        </div>
        <div class="flex items-center gap-2">
          <span>每帧停留(ms)</span>
          <Input
            v-model.number="intervalMs"
            type="number"
            min="300"
            max="3000"
            step="50"
            class="h-8 w-24"
          />
        </div>
      </div>
      <div
        class="min-h-5 whitespace-pre-wrap break-all rounded-md border bg-muted/40 px-3 py-2 text-sm"
        :class="statusColor[outCls]"
      >
        {{ outMsg }}
      </div>
    </CardContent>
    <QrOverlay v-if="playing" :qr-text="qrText" :tip="qrTip" @close="stopPlay(false)" />
  </Card>
</template>
