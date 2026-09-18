/**
 * 打字回显：环形覆盖网格（框架无关）。
 *
 * 快速键盘注入时整段滚动文字根本看不清。改为把字符按到达顺序逐个落入
 * 固定的 cols × rows 网格（行优先），格子位置永不移动；网格填满后新字符
 * 从头循环**原位覆盖**最旧字符，最新落子的格子由视图层做高亮动画：
 *
 *   A → AB → AB → AB → EB → EF
 *              C    CD    CD
 *
 * update() 接收 receiver 推送的「当前文本」（payload 阶段为尾部窗口切片，
 * 长度递增；窗口满后长度不变、内容整体左移一位），内部通过与上次文本 diff
 * 识别本次新到的字符；窗口如何滑动与格子位置无关，格子只按到达序号取模定位。
 *
 * digest 阶段的校验码通过 suffix 透传，显示在网格后方，不参与覆盖。
 */

export interface TypedGridView {
  /** 网格当前内容，长度恒为 cols*rows；空格子为空字符串。 */
  cells: string[]
  /** 列数（视图层据此生成 grid-template-columns）。 */
  cols: number
  /** 最新落子格下标；-1 表示当前无新落子（如仅 suffix 变化）。 */
  freshPos: number
  /** 最新落子序号（每落一字自增），视图层用作 key 重放高亮动画。 */
  freshKey: number
  /** 网格后方的追加文本（digest 阶段为 ` · xyz`，其余为 ''）。 */
  suffix: string
}

export interface TypedGridOptions {
  /** 每行列数，默认 48。 */
  cols?: number
  /** 行数，默认 2（总格子 cols*rows = 96，与接收窗口一致）。 */
  rows?: number
}

export interface TypedDisplay {
  /** 接 receiver 的 onTyped：null=隐藏并清空网格。 */
  update: (text: string | null, suffix?: string) => void
}

export function createTypedDisplay(
  onChange: (view: TypedGridView | null) => void,
  options: TypedGridOptions = {},
): TypedDisplay {
  const cols = options.cols ?? 48
  const rows = options.rows ?? 2
  const total = cols * rows

  let cells: string[] = Array.from<string>({ length: total }).fill('')
  let prev = ''
  let prevSuffix = ''
  let seq = -1
  let freshPos = -1
  let freshKey = 0

  function emit() {
    onChange({ cells: [...cells], cols, freshPos, freshKey, suffix: prevSuffix })
  }

  function clearGrid() {
    cells = Array.from<string>({ length: total }).fill('')
    seq = -1
    freshPos = -1
  }

  function put(ch: string) {
    seq += 1
    freshPos = seq % total
    cells[freshPos] = ch
    freshKey = seq + 1
  }

  return {
    update(text, suffix = '') {
      if (text === null) {
        clearGrid()
        prev = ''
        prevSuffix = ''
        onChange(null)
        return
      }

      let added = ''
      if (text === '' && prev !== '') {
        // 阶段切换（magic → payload）：网格清空重排
        clearGrid()
      }
      else if (text.length > prev.length) {
        // 窗口未填满：新增若干字符（正常每键 1 个）
        added = text.slice(prev.length)
      }
      else if (text.length === prev.length && text !== prev) {
        // 窗口已满整体滑动：最后 1 个字符是本次新到的
        added = text.slice(-1)
      }

      for (const ch of added)
        put(ch)

      const suffixChanged = suffix !== prevSuffix
      prev = text
      prevSuffix = suffix

      if (added || text === '' || suffixChanged)
        emit()
    },
  }
}
