/**
 * 写剪贴板：成功返回 true；浏览器不支持或用户拒绝时返回 false，
 * 由调用方降级到手动复制 UI。
 */
export async function writeClipboard(text: string): Promise<boolean> {
  if (!navigator.clipboard?.writeText)
    return false
  try {
    await navigator.clipboard.writeText(text)
    return true
  }
  catch {
    return false
  }
}

/**
 * 读剪贴板：浏览器不支持直接 reject；权限被拒绝时 promise 自然 reject，
 * 由调用方降级到粘贴框 UI。
 */
export async function readClipboard(): Promise<string> {
  if (!navigator.clipboard?.readText)
    throw new Error('浏览器不支持剪贴板读取')
  return navigator.clipboard.readText()
}
