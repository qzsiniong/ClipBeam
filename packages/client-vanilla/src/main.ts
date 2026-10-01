import type { ReceiverStatusClass } from '@clipbeam/shared'
import {
  buildFeedbackFrame,
  buildFrames,
  clamp,
  createReceiver,
  createTransferSession,
  createTypedDisplay,
  DEBUG,
  readClipboard,
  writeClipboard,
} from '@clipbeam/shared'
import QrCreator from 'qr-creator'

// 一行即可确认调试开关是否生效（开关本身见 @clipbeam/shared 的 debug 模块）
if (DEBUG) {
  // eslint-disable-next-line no-console -- 调试开关的确认行，本身就要求输出
  console.debug('[clipbeam:page] 调试输出已开启（localStorage clipbeam-debug / URL #debug）')
}

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

/* ================= 文件接收（协议 D，与协议 A 共用同一条按键流） ============ */

const fbStatus = $('fbStatus')
const fbBar = $('fbBar')
const fbQrBox = $('fbQr')

function setFb(cls: ReceiverStatusClass, msg: string) {
  fbStatus.className = `status ${cls}`
  fbStatus.textContent = msg
}

/** 反馈二维码：固定在右下角，宿主脚本用 `$.scan_qr()` 读它。 */
function renderFbQr(text: string) {
  fbQrBox.innerHTML = ''
  QrCreator.render({ text, ecLevel: 'M', size: 300, radius: 0 }, fbQrBox)
}

const transfer = createTransferSession({
  onStatus: setFb,
  onProgress: (got, total) => {
    fbBar.style.display = total > 0 ? 'block' : 'none'
    // 进度条用同一个 .bar 结构（内层 <i> 的宽度即进度）
    const inner = fbBar.firstElementChild as HTMLElement | null
    if (inner && total > 0)
      inner.style.width = `${Math.round((got / total) * 100)}%`
  },
  onQr: renderFbQr,
})

// 首屏就渲染一张 UNAUTH，让宿主机一进来就能读到「还没授权」
renderFbQr(buildFeedbackFrame({ type: 'feedback', status: 'unauthorized' }))

$('dirBtn').addEventListener('click', () => {
  void transfer.pickDirectory()
})

/**
 * 同一条 keydown 流分派给两条通道。
 *
 * 先给文件通道（`kbb`/`kbc`），它不认再交给文本通道（`kba`）。两者靠 magic 区分：
 * v2 里两类帧都以 `0` 开头，靠 **magic** 在第 4 个字符分岔：
 * 文本是 `0kba`，文件是 `0kbb`/`0kbc`。所以两边各自校验前缀、各自复位 ——
 * 文件通道认出 `0kba` 就放手，文本通道认出 `0kbb`/`0kbc` 也放手。
 */
window.addEventListener('keydown', (e) => {
  if (e.repeat)
    return
  const target = e.target as HTMLElement | null
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT' || target.isContentEditable))
    return

  // 单字符键才算数据（修饰键 / 方向键的 e.key 是多字符名）
  if (e.key.length === 1) {
    const consumed = transfer.accept(e.key)
    if (consumed)
      e.preventDefault()
    // 无论文件通道是否接手，都继续喂给文本通道：它要靠前 4 个字符
    // （`0kba` vs `0kbb`/`0kbc`）自己判定，认不出就复位，不会误收。
  }
  receiver.handle(e)
}, true)
setInterval(() => {
  receiver.checkTimeout()
  transfer.checkTimeout()
}, 500)

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
