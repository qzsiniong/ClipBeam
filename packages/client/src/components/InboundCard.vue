<script setup lang="ts">
import type { ReceiverStatusClass, TypedGridView } from '@clipbeam/shared'
import { createReceiver, createTypedDisplay, writeClipboard } from '@clipbeam/shared'
import { nextTick, onMounted, onUnmounted, ref } from 'vue'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'

/* ================= 入站：键盘状态机（协议逻辑在 @clipbeam/shared） =================
   帧: 0 <magic> 0 <payload> 0 <digest> 1
   magic = "clipbeam1"（v1 未压缩）或 "clipbeam2"（v2 zstd 压缩） */
const paused = ref(false)
const inCls = ref<ReceiverStatusClass>('')
const inMsg = ref('等待接收…')
const barShow = ref(false)
// 打字回显视图：固定网格原位循环覆盖，freshPos 为最新落子格
const typedView = ref<TypedGridView | null>(null)
const manualShow = ref(false)
const manualText = ref('')
const manualWrap = ref<HTMLElement | null>(null)

let idleTimer: number | null = null

const statusColor: Record<ReceiverStatusClass, string> = {
  '': 'text-muted-foreground',
  'run': 'text-sky-400',
  'ok': 'text-emerald-400',
  'err': 'text-red-400',
  'warn': 'text-amber-400',
}

function setIn(cls: ReceiverStatusClass, msg: string) {
  inCls.value = cls
  inMsg.value = msg
}

function fallbackCopy(text: string) {
  manualText.value = text
  manualShow.value = true
  setIn('warn', `✓ CRC 校验通过（${text.length} 字符），但浏览器拒绝自动写剪贴板，请手动复制上方文本。`)
  nextTick(() => manualWrap.value?.querySelector('textarea')?.select())
}

async function onText(text: string) {
  const ok = await writeClipboard(text)
  if (ok)
    setIn('ok', `✓ 成功：已写入远程剪贴板（${text.length} 字符，CRC 校验通过）`)
  else
    fallbackCopy(text)
}

const typedDisplay = createTypedDisplay((view) => {
  typedView.value = view
}, {
  cols: 80,
  rows: 10,
})

const receiver = createReceiver({
  isPaused: () => paused.value,
  onStatus: setIn,
  onBar: on => (barShow.value = on),
  onTyped: s => typedDisplay.update(s),
  onText,
})

function copyManual() {
  manualWrap.value?.querySelector('textarea')?.select()
  try {
    document.execCommand('copy')
  }
  catch {
    // 忽略，下方再尝试 Clipboard API
  }
  navigator.clipboard?.writeText(manualText.value).catch(() => {})
}

onMounted(() => {
  window.addEventListener('keydown', receiver.handle, true)
  idleTimer = window.setInterval(() => receiver.checkTimeout(), 500)
})

onUnmounted(() => {
  window.removeEventListener('keydown', receiver.handle, true)
  if (idleTimer !== null)
    clearInterval(idleTimer)
})
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>
        ① 接收入站（宿主机 → 远程）
      </CardTitle>
      <CardDescription>
        在宿主机按发送热键后，这里会自动接收、CRC 校验并写入远程剪贴板。
      </CardDescription>
    </CardHeader>
    <CardContent class="space-y-4">
      <Label class="cursor-pointer text-muted-foreground">
        <Checkbox v-model="paused" />
        暂停接收（在页面输入框内打字时建议勾选）
      </Label>
      <div v-if="barShow" class="h-1.5 overflow-hidden rounded-full bg-muted">
        <div class="h-full w-1/2 animate-qr-pulse rounded-full bg-sky-500" />
      </div>
      <div
        class="min-h-5 whitespace-pre-wrap break-all rounded-md border bg-muted/40 px-3 py-2 text-sm"
        :class="statusColor[inCls]"
      >
        {{ inMsg }}
      </div>
      <div class="typed" :class="{ show: typedView !== null }" aria-label="键盘输入实时回显">
        <div
          v-if="typedView"
          class="typed-grid"
          :style="{ gridTemplateColumns: `repeat(${typedView.cols}, 1ch)` }"
        >
          <span
            v-for="(ch, i) in typedView.cells"
            :key="i === typedView.freshPos ? `f${i}-${typedView.freshKey}` : `c${i}`"
            class="cell"
            :class="{ fresh: i === typedView.freshPos }"
          >{{ ch }}</span>
        </div>
        <div class="typed-line2">
          <span>{{ typedView?.suffix ?? '' }}</span><span class="cursor" aria-hidden="true">▋</span>
        </div>
      </div>
      <div v-if="manualShow" ref="manualWrap" class="space-y-2">
        <Textarea v-model="manualText" class="min-h-24 resize-y text-xs" />
        <Button variant="secondary" size="sm" @click="copyManual">
          复制上述文本
        </Button>
      </div>
    </CardContent>
  </Card>
</template>
