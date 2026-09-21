import type { ClipBeamDevHelpers } from './types'
/**
 * `window.__CLIPBEAM_DEV__` 的实现（仅 mock 模式挂载）。
 *
 * 后端事件在 mock 下不会自己发生，用它手动造：
 *
 * ```js
 * __CLIPBEAM_DEV__.emit('worker-started', 'send')      // 侧边栏状态徽标变成"发送"
 * __CLIPBEAM_DEV__.consoleLine('error', '手造一行错误') // Console 面板立刻出现红行
 * await __CLIPBEAM_DEV__.runScript('01-quick-start.js') // 完整演练一次"运行脚本"
 * ```
 */
import { emitEvent, pushConsole, simulateRun } from './events'
import { firstScriptName, getConfig, listScripts } from './state'

/** 构造调试助手（挂到 `window` 上的对象）。 */
export function createDevHelpers(): ClipBeamDevHelpers {
  return {
    emit: (event, payload) => emitEvent(event, payload),
    consoleLine: pushConsole,
    runScript: async (name) => {
      await simulateRun(name ?? firstScriptName())
    },
    scripts: listScripts,
    config: () => getConfig(),
  }
}
