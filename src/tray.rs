//! 系统托盘：图标 + 右键（及左键）菜单；菜单项在忙时禁用/改写状态。

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub const M_STATUS: &str = "status";
pub const M_SEND: &str = "send";
pub const M_RECV: &str = "recv";
pub const M_DEPLOY_TYPE: &str = "deploy_type";
pub const M_DEPLOY_COPY: &str = "deploy_copy";
pub const M_SETTINGS: &str = "settings";
pub const M_QUIT: &str = "quit";

/// 64x64 RGBA（build.rs 生成）。
const ICON_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/keybeam_icon.rgba"));

fn load_icon() -> Result<Icon, String> {
    Icon::from_rgba(ICON_RGBA.to_vec(), 64, 64).map_err(|e| format!("托盘图标创建失败: {e}"))
}

/// 给设置窗口用的 winit 图标。
pub fn window_icon() -> winit::window::Icon {
    winit::window::Icon::from_rgba(ICON_RGBA.to_vec(), 64, 64).expect("64x64 RGBA 合法")
}

pub struct TrayMenu {
    // 字段顺序：TrayIcon 必须活到程序退出。
    pub _tray: TrayIcon,
    pub status_item: MenuItem,
    pub send_item: MenuItem,
    pub recv_item: MenuItem,
    pub deploy_type_item: MenuItem,
}

impl TrayMenu {
    /// 切换忙时菜单状态：禁用三个执行类条目，更新状态文本。
    pub fn set_busy(&self, busy: bool, status: &str) {
        self.send_item.set_enabled(!busy);
        self.recv_item.set_enabled(!busy);
        self.deploy_type_item.set_enabled(!busy);
        self.status_item.set_text(status);
    }
}

pub fn build() -> Result<TrayMenu, String> {
    let status_item = MenuItem::with_id(M_STATUS, "状态：空闲", false, None);
    let send_item = MenuItem::with_id(M_SEND, "发送本机剪贴板 → 远程（热键）", true, None);
    let recv_item = MenuItem::with_id(M_RECV, "截屏接收远程二维码（热键）", true, None);
    let deploy_type_item =
        MenuItem::with_id(M_DEPLOY_TYPE, "部署接收页到远程（键盘输入，约 3 分钟）", true, None);
    let deploy_copy_item =
        MenuItem::with_id(M_DEPLOY_COPY, "部署接收页（复制到宿主机剪贴板）", true, None);
    let settings_item = MenuItem::with_id(M_SETTINGS, "设置…", true, None);
    let quit_item = MenuItem::with_id(M_QUIT, "退出 KeyBeam", true, None);

    let menu = Menu::with_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &send_item,
        &recv_item,
        &PredefinedMenuItem::separator(),
        &deploy_type_item,
        &deploy_copy_item,
        &PredefinedMenuItem::separator(),
        &settings_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])
    .map_err(|e| format!("托盘菜单创建失败: {e}"))?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("KeyBeam 剪贴板桥接")
        .with_icon(load_icon()?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()
        .map_err(|e| format!("托盘创建失败: {e}"))?;

    Ok(TrayMenu {
        _tray: tray,
        status_item,
        send_item,
        recv_item,
        deploy_type_item,
    })
}
