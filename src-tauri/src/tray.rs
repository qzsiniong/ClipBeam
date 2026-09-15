//! 系统托盘:Tauri 2 TrayIconBuilder + Menu。
//! 菜单项与原 tray.rs 等价;忙时禁用三个执行类条目、更新状态文本。

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Wry,
};

pub const M_STATUS: &str = "status";
pub const M_SEND: &str = "send";
pub const M_RECV: &str = "recv";
pub const M_DEPLOY_TYPE: &str = "deploy_type";
pub const M_DEPLOY_COPY: &str = "deploy_copy";
pub const M_SETTINGS: &str = "settings";
pub const M_QUIT: &str = "quit";

/// 托盘菜单项句柄(存入 Tauri State 供 set_busy 使用)。
/// MenuItem 内部用 Arc,Clone 廉价,且 Send+Sync。
#[allow(dead_code)]
pub struct TrayItems {
    pub status: MenuItem<Wry>,
    pub send: MenuItem<Wry>,
    pub recv: MenuItem<Wry>,
    pub deploy_type: MenuItem<Wry>,
}

/// 构建托盘菜单(7 个菜单项 + 4 个分隔符,与原版一致)。
pub fn build(app: &App, _cfg: &crate::config::Config) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, M_STATUS, "状态:空闲", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let send = MenuItem::with_id(
        app,
        M_SEND,
        "发送本机剪贴板 → 远程(热键)",
        true,
        None::<&str>,
    )?;
    let recv = MenuItem::with_id(app, M_RECV, "截屏接收远程二维码(热键)", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let deploy_type = MenuItem::with_id(
        app,
        M_DEPLOY_TYPE,
        "部署接收页到远程(键盘输入,约 3 分钟)",
        true,
        None::<&str>,
    )?;
    let deploy_copy = MenuItem::with_id(
        app,
        M_DEPLOY_COPY,
        "部署接收页(复制到宿主机剪贴板)",
        true,
        None::<&str>,
    )?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let settings = MenuItem::with_id(app, M_SETTINGS, "设置…", true, None::<&str>)?;
    let sep4 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, M_QUIT, "退出 ClipBeam", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &sep1,
            &send,
            &recv,
            &sep2,
            &deploy_type,
            &deploy_copy,
            &sep3,
            &settings,
            &sep4,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("ClipBeam 剪贴板桥接")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;

    app.manage(TrayItems {
        status,
        send,
        recv,
        deploy_type,
    });

    Ok(())
}

/// 托盘图标事件回调(预留:左键双击显示主窗口等)。
pub fn on_tray_event(_app: &AppHandle, _event: TrayIconEvent) {}

/// 忙闲状态切换:禁用三个执行类菜单项,更新状态文本。
#[allow(dead_code)]
pub fn set_busy(app: &AppHandle, busy: bool, status: &str) -> tauri::Result<()> {
    let items = app.state::<TrayItems>();
    items.send.set_enabled(!busy)?;
    items.recv.set_enabled(!busy)?;
    items.deploy_type.set_enabled(!busy)?;
    items.status.set_text(status)?;
    Ok(())
}
