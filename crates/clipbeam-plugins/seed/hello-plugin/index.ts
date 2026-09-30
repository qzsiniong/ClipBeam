/// 
// 01 示例插件：hello-plugin
//
// 这是一个**最小但完整**的插件，演示插件框架的五件事：
//   1. 入口脚本在「插件被启用」时执行一次（允许顶层 await）；
//   2. 用 $plugin.tray.onAction 登记托盘菜单的动作回调（菜单项由 plugin.json 的 menus 声明）；
//   3. 用 $plugin.toast / $plugin.notify / $plugin.alert / $plugin.confirm 做反馈；
//   4. 用 $plugin.window.* 开一个自己的窗口，并与页面双向通信；
//   5. 用 console.* 写日志（出现在「插件」页面的日志面板里）。
//
// 插件是什么：<配置目录>/ClipBeam/plugins/<id>/ 下的一个目录，目录名必须等于 plugin.json 里的 id。
// 改完代码后在插件页面点「重新加载」即可生效（当前版本没有热重载）。
//
// $plugin 是插件能力命名空间（正式名 ClipBeamPlugin），类型提示见
// crates/clipbeam-plugins/src/spec/plugins.d.ts。

console.log('hello-plugin 已启用')

// 菜单项的 id 在 plugin.json 的 menus 里声明；这里用一个回调区分它们 ——
// 加菜单项不需要改这份代码。
$plugin.tray.onAction(async (action) => {
  console.log(`收到动作：${action.id}`)

  if (action.id === 'hello') {
    $plugin.toast('你好，我是插件 👋', { level: 'success' })
    $plugin.tray.setTooltip('hello-plugin 刚刚打过招呼')
    return
  }

  if (action.id === 'notify') {
    $plugin.notify('ClipBeam 插件', '这条来自 hello-plugin')
    $plugin.toast('已发系统通知（看通知中心）', { level: 'info' })
    return
  }

  if (action.id === 'confirm') {
    // confirm 的三种回答：主按钮 true、次按钮 false、关闭/超时 null
    const ok = await $plugin.confirm('要弹一条「成功」提示吗？')
    if (ok === true) {
      $plugin.toast('你选了「是」', { level: 'success' })
    } else if (ok === false) {
      $plugin.toast('你选了「否」', { level: 'warning' })
    } else {
      // 关掉对话框或超时：这不是错误，是「没有回答」
      $plugin.toast('没有回答（关闭或超时）', { level: 'info' })
    }
    return
  }

  if (action.id === 'window') {
    openDemoWindow()
    return
  }

  // 清单里加了菜单项却忘了在这里处理时，至少要让用户看见
  $plugin.toast(`这个菜单项还没有实现：${action.id}`, { level: 'warning' })
})

/**
 * 打开演示窗口：页面是插件目录里的 relay.html。
 *
 * 文件名刻意不叫 index.html：那正是应用自己的入口，`pnpm dev` 下经 Vite 会被回退
 * 到 SPA（iframe 里就会莫名其妙地加载整套应用）。
 *
 * 双向通信：页面用 `window.parent.postMessage(...)` 发消息给我，
 * 我用 `$plugin.window.post(win.id, ...)` 发给它。
 *
 * 注意页面跑在**沙箱 iframe** 里（拿不到宿主的 IPC），所以两边只能这样说话 ——
 * 这也是插件页面里没有 `invoke` 可用的原因。
 */
function openDemoWindow() {
  const win = $plugin.window.open({
    title: 'hello-plugin 面板',
    width: 420,
    height: 320,
    page: 'relay.html',
    alwaysOnTop: true,
  })
  console.log(`打开了窗口 ${win.id}（宿主标签 ${win.label}）`)

  // 页面 → 插件
  $plugin.window.onMessage(win.id, (message) => {
    console.log('页面发来：' + JSON.stringify(message))
    // 原样回一条，页面上会显示出来
    $plugin.window.post(win.id, { from: 'plugin', echo: message, at: Date.now() })
  })

  // 窗口关闭是**唯一**能清理回调的时机，所以一定要登记
  $plugin.window.onClosed(win.id, () => {
    console.log(`窗口 ${win.id} 已关闭，清理它的回调`)
    $plugin.toast('演示窗口已关闭', { level: 'info' })
  })
}
