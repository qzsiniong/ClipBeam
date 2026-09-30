/// 
// 01 示例插件：hello-plugin
//
// 这是一个**最小但完整**的插件，演示插件框架的四件事：
//   1. 入口脚本在「插件被启用」时执行一次（允许顶层 await）；
//   2. 用 $plugin.tray.onAction 登记托盘菜单的动作回调（菜单项由 plugin.json 的 menus 声明）；
//   3. 用 $plugin.toast / $plugin.notify / $plugin.alert / $plugin.confirm 做反馈；
//   4. 用 console.* 写日志（出现在「插件」页面的日志面板里）。
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

  // 清单里加了菜单项却忘了在这里处理时，至少要让用户看见
  $plugin.toast(`这个菜单项还没有实现：${action.id}`, { level: 'warning' })
})
