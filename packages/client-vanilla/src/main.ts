import type { ReceiverStatusClass } from '@clipbeam/shared'
import {
  buildFrames,
  clamp,
  createReceiver,
  createTypedDisplay,
  readClipboard,
  writeClipboard,
} from '@clipbeam/shared'
import QrCreator from 'qr-creator'

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement
}

/* ================= 入站（宿主机 → 远程） ================= */
const pauseChk = $('pauseChk') as HTMLInputElement
const inStatus = $('inStatus')
const inBar = $('inBar')
const manualRow = $('manualRow')
const manualCopy = $('manualCopy') as HTMLTextAreaElement
const typedBox = $('typed')
const typedGrid = $('typedGrid')
const typedSuffix = $('typedSuffix')
const GRID_COLS = 30
const GRID_ROWS = 10
const cellEls: HTMLSpanElement[] = []
for (let i = 0; i < GRID_COLS * GRID_ROWS; i++) {
  const el = document.createElement('span')
  el.className = 'cell'
  typedGrid.appendChild(el)
  cellEls.push(el)
}

function setIn(cls: ReceiverStatusClass, msg: string) {
  inStatus.className = `status ${cls}`
  inStatus.textContent = msg
}

function fallbackCopy(text: string) {
  manualCopy.value = text
  manualCopy.style.display = 'block'
  manualRow.style.display = 'flex'
  manualCopy.select()
  setIn('warn', `✓ CRC 校验通过（${text.length} 字符），但浏览器拒绝自动写剪贴板，请手动复制上方文本。`)
}

async function writeRemoteClipboard(text: string) {
  const ok = await writeClipboard(text)
  if (ok)
    setIn('ok', `✓ 成功：已写入远程剪贴板（${text.length} 字符，CRC 校验通过）`)
  else
    fallbackCopy(text)
}

// 打字回显：字符落入固定网格原位循环覆盖，最新落子格 pop 高亮
let lastFreshPos = -1
let lastFreshKey = 0
const typedDisplay = createTypedDisplay((view) => {
  if (view === null) {
    typedBox.classList.remove('show')
    typedSuffix.textContent = ''
    for (const el of cellEls) {
      el.textContent = ''
      el.classList.remove('fresh')
    }
    lastFreshPos = -1
    lastFreshKey = 0
    return
  }
  for (let i = 0; i < cellEls.length; i++) {
    if (cellEls[i].textContent !== view.cells[i])
      cellEls[i].textContent = view.cells[i]
  }
  if (lastFreshPos !== view.freshPos || view.freshKey !== lastFreshKey) {
    if (lastFreshPos >= 0)
      cellEls[lastFreshPos]?.classList.remove('fresh')
    if (view.freshPos >= 0) {
      const el = cellEls[view.freshPos]
      // 重放 pop 动画：移除类 → 强制重排 → 加回类
      el.classList.remove('fresh')
      void el.offsetWidth
      el.classList.add('fresh')
    }
    lastFreshPos = view.freshPos
    lastFreshKey = view.freshKey
  }
  typedSuffix.textContent = view.suffix
  typedBox.classList.add('show')
}, {
  cols: GRID_COLS,
  rows: GRID_ROWS,
})

const receiver = createReceiver({
  isPaused: () => pauseChk.checked,
  onStatus: setIn,
  onBar: on => inBar.style.display = on ? 'block' : 'none',
  onTyped: s => typedDisplay.update(s),
  onText: writeRemoteClipboard,
})

window.addEventListener('keydown', e => receiver.handle(e), true)
setInterval(() => receiver.checkTimeout(), 500)

$('copyBtn').addEventListener('click', () => {
  manualCopy.select()
  try {
    document.execCommand('copy')
  }
  catch {
    // 忽略，下方再尝试 Clipboard API
  }
  navigator.clipboard?.writeText(manualCopy.value).catch(() => {})
})

/* ================= 出站（远程 → 宿主机） ================= */
const outStatus = $('outStatus')
const qrBox = $('qrBox')
const qrTip = $('qrTip')
const qrc = $('qrc')
const stopBtn = $('stopBtn') as HTMLButtonElement
const pasteBox = $('pasteBox') as HTMLTextAreaElement
const pasteRow = $('pasteRow')
const chunkIn = $('chunkIn') as HTMLInputElement
const intervalIn = $('intervalIn') as HTMLInputElement

let playTimer: number | null = null
let frames: string[] = []
let idx = 0
let total = 0

function setOut(cls: ReceiverStatusClass, msg: string) {
  outStatus.className = `status ${cls}`
  outStatus.textContent = msg
}

function showPaste() {
  pasteBox.style.display = 'block'
  pasteRow.style.display = 'flex'
}

function renderQr(text: string) {
  qrc.innerHTML = ''
  QrCreator.render({
    text,
    ecLevel: 'M',
    size: 1000,
    radius: 0,
  }, qrc)
}

function renderFrame() {
  renderQr(frames[idx])
  qrTip.textContent = `第 ${idx + 1} / ${total} 帧 · 点击任意处关闭`
  setOut('run', `播放中：第 ${idx + 1} / ${total} 帧。请在宿主机按接收热键`)
  idx = (idx + 1) % total
}

function startPlay(text: string) {
  stopPlay(true)
  if (!text) {
    setOut('err', '文本为空')
    return
  }
  try {
    const built = buildFrames(text, clamp(chunkIn.value, 200, 1500))
    frames = built.frames
    total = built.total
  }
  catch (ex) {
    setOut('err', `编码失败: ${(ex as Error).message}`)
    return
  }
  idx = 0
  stopBtn.disabled = false
  qrBox.classList.add('show')
  renderFrame()

  // 单帧无需轮播
  if (total > 1) {
    playTimer = window.setInterval(renderFrame, clamp(intervalIn.value, 300, 3000))
  }
}

function stopPlay(silent: boolean) {
  if (playTimer !== null) {
    clearInterval(playTimer)
    playTimer = null
  }
  stopBtn.disabled = true
  qrBox.classList.remove('show')
  if (!silent && total)
    setOut('warn', `已停止播放（共 ${total} 帧）`)
}

qrBox.addEventListener('click', () => stopPlay(false))

$('readBtn').addEventListener('click', () => {
  readClipboard().then(startPlay, (ex: unknown) => {
    showPaste()
    const msg = ex instanceof Error && ex.message.includes('不支持')
      ? '浏览器不支持剪贴板读取，请粘贴文本后用下方按钮生成。'
      : '读取被拒绝，请粘贴文本后用下方按钮生成。'
    setOut('warn', msg)
  })
})

$('pasteStartBtn').addEventListener('click', () => startPlay(pasteBox.value))
stopBtn.addEventListener('click', () => stopPlay(false))

;($('advChk') as HTMLInputElement).addEventListener('change', function () {
  $('advBox').style.display = this.checked ? 'flex' : 'none'
})
