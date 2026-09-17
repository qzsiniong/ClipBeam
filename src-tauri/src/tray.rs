//! 系统托盘:Tauri 2 TrayIconBuilder + Menu。
//! - 菜单项热键用 accelerator 参数,macOS 自动浅色右对齐显示
//! - label 前加 emoji 图标
//! - 忙时禁用执行类条目、状态行动态更新进度
//! - 托盘图标动态绘制进度环(单色 template,自动适配明暗主题)

use crate::config::Config;
use image::{ImageBuffer, Rgba, RgbaImage};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Manager, Wry,
};

pub const M_STATUS: &str = "status";
pub const M_SEND: &str = "send";
pub const M_RECV: &str = "recv";
pub const M_DEPLOY_TYPE: &str = "deploy_type";
pub const M_DEPLOY_COPY: &str = "deploy_copy";
pub const M_SETTINGS: &str = "settings";
pub const M_QUIT: &str = "quit";

/// 托盘菜单项句柄(存入 Tauri State 供 set_busy/set_status_text 使用)。
pub struct TrayItems {
    pub status: MenuItem<Wry>,
    pub send: MenuItem<Wry>,
    pub recv: MenuItem<Wry>,
    pub deploy_type: MenuItem<Wry>,
}

/// 构建托盘菜单。cfg 用于设置菜单项 accelerator(热键)。
pub fn build(app: &App, cfg: &Config) -> tauri::Result<()> {
    // 热键作为 accelerator 传入,macOS 自动浅色右对齐显示;label 只保留功能描述 + emoji
    let send = MenuItem::with_id(
        app, M_SEND, "📤 发送本机剪贴板 → 远程", true,
        Some(cfg.send_hotkey.clone()),
    )?;
    let recv = MenuItem::with_id(
        app, M_RECV, "📥 截屏接收远程二维码", true,
        Some(cfg.recv_hotkey.clone()),
    )?;

    let status = MenuItem::with_id(app, M_STATUS, "状态:空闲", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let deploy_type = MenuItem::with_id(
        app,
        M_DEPLOY_TYPE,
        "⌨️ 部署接收页到远程(键盘输入,约 3 分钟)",
        true,
        None::<&str>,
    )?;
    let deploy_copy = MenuItem::with_id(
        app,
        M_DEPLOY_COPY,
        "📋 部署接收页(复制到宿主机剪贴板)",
        true,
        None::<&str>,
    )?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let settings = MenuItem::with_id(app, M_SETTINGS, "⚙️ 设置…", true, None::<&str>)?;
    let sep4 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, M_QUIT, "❌ 退出 ClipBeam", true, None::<&str>)?;

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

    let icon = app
        .default_window_icon()
        .map(|i| i.clone())
        .unwrap_or_else(|| Image::new(&[], 1, 1));

    TrayIconBuilder::with_id("main")
        .icon(icon)
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

/// 忙闲状态切换:禁用三个执行类菜单项,更新状态文本。
pub fn set_busy(app: &AppHandle, busy: bool, status: &str) -> tauri::Result<()> {
    let items = app.state::<TrayItems>();
    items.send.set_enabled(!busy)?;
    items.recv.set_enabled(!busy)?;
    items.deploy_type.set_enabled(!busy)?;
    items.status.set_text(status)?;
    // 恢复默认图标(空闲)
    if !busy {
        if let Some(tray) = app.tray_by_id("main") {
            if let Some(icon) = app.default_window_icon() {
                let _ = tray.set_icon(Some(icon.clone()));
            }
        }
    }
    Ok(())
}

/// 更新状态行进度文本(供 worker send_progress 调用)。
pub fn set_status_text(app: &AppHandle, text: &str) -> tauri::Result<()> {
    let items = app.state::<TrayItems>();
    items.status.set_text(text)
}

/// 更新托盘图标为带进度环的单色图标(macOS template,自动适配明暗主题)。
pub fn set_tray_progress(app: &AppHandle, percent: u8) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main") else {
        return Ok(());
    };
    let img = draw_progress_icon(percent);
    let w = img.width();
    let h = img.height();
    let rgba = img.into_raw();
    let tauri_img = Image::new_owned(rgba, w, h);
    tray.set_icon(Some(tauri_img))?;
    // 设为 template image,macOS 自动适配明暗主题
    tray.set_icon_as_template(true)?;
    Ok(())
}

/// 绘制 128x128 单色进度环 PNG(透明背景 + 环)。
/// 环内已完成部分 alpha=255,未完成部分 alpha=60(半透明背景环)。
fn draw_progress_icon(percent: u8) -> RgbaImage {
    let size = 128;
    let cx = 64.0;
    let cy = 64.0;
    let outer = 56.0;
    let inner = 44.0;
    let percent = percent.min(100) as f64 / 100.0;
    // 进度弧:从顶部(-90°)顺时针扫过
    let start_angle = -std::f64::consts::FRAC_PI_2;

    let mut img: RgbaImage = ImageBuffer::from_pixel(size, size, Rgba([0, 0, 0, 0]));

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < inner || dist > outer {
                continue;
            }
            // 计算像素角度(atan2,从顶部顺时针)
            let angle = dy.atan2(dx);
            // 归一化到 [-PI, PI],判断是否在进度弧内
            let in_progress = if percent >= 1.0 {
                true
            } else {
                // 把 angle 转换到 [start_angle, start_angle + TAU) 区间
                let mut a = angle - start_angle;
                while a < 0.0 {
                    a += std::f64::consts::TAU;
                }
                while a >= std::f64::consts::TAU {
                    a -= std::f64::consts::TAU;
                }
                a <= percent * std::f64::consts::TAU
            };
            let alpha = if in_progress { 255u8 } else { 60u8 };
            img.put_pixel(x, y, Rgba([0, 0, 0, alpha]));
        }
    }
    img
}

/// 托盘图标事件回调(预留)。
pub fn on_tray_event(_app: &AppHandle, _event: tauri::tray::TrayIconEvent) {}
