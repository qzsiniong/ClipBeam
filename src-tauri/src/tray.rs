//! 系统托盘:Tauri 2 TrayIconBuilder + Menu。
//! - 菜单项用 IconMenuItemBuilder 配 32x32 矢量图标(emoji 已移除)
//! - 菜单项热键用 accelerator 参数,macOS 自动浅色右对齐显示
//! - 忙时禁用执行类条目、状态行动态更新进度
//! - 托盘图标动态绘制进度环(单色 template,自动适配明暗主题)

use crate::config::Config;
use image::{ImageBuffer, Rgba, RgbaImage};
use tauri::{
    App, AppHandle, Manager, Wry, image::Image,
    menu::{IconMenuItem, IconMenuItemBuilder, MenuBuilder},
    tray::TrayIconBuilder,
};

pub const M_STATUS: &str = "status";
pub const M_SEND_RAW: &str = "send_raw";
pub const M_SEND: &str = "send";
pub const M_RECV: &str = "recv";
pub const M_DEPLOY_TYPE: &str = "deploy_type";
pub const M_DEPLOY_COPY: &str = "deploy_copy";
pub const M_SETTINGS: &str = "settings";
pub const M_QUIT: &str = "quit";

/// 托盘菜单项句柄(存入 Tauri State 供 set_busy/set_status_text 使用)。
pub struct TrayItems {
    pub status: IconMenuItem<Wry>,
    pub send: IconMenuItem<Wry>,
    pub recv: IconMenuItem<Wry>,
    pub deploy_type: IconMenuItem<Wry>,
}

/// 构建托盘菜单。cfg 用于设置菜单项 accelerator(热键)。
pub fn build(app: &App, cfg: &Config) -> tauri::Result<()> {
    // 热键作为 accelerator 传入,macOS 自动浅色右对齐显示;图标由 IconMenuItemBuilder 承载
    let send_raw = IconMenuItemBuilder::with_id(M_SEND_RAW, "发送本机剪贴板(原样)")
        .accelerator(cfg.send_raw_hotkey.as_str())
        .icon(icon_send())
        .build(app)?;
    let send = IconMenuItemBuilder::with_id(M_SEND, "发送本机剪贴板 → 远程")
        .accelerator(cfg.send_hotkey.as_str())
        .icon(icon_send())
        .build(app)?;
    let recv = IconMenuItemBuilder::with_id(M_RECV, "截屏接收远程二维码")
        .accelerator(cfg.recv_hotkey.as_str())
        .icon(icon_recv())
        .build(app)?;

    let status = IconMenuItemBuilder::with_id(M_STATUS, "状态:空闲").enabled(false).build(app)?;
    let deploy_type = IconMenuItemBuilder::with_id(
        M_DEPLOY_TYPE,
        "部署接收页到远程(键盘输入,约 3 分钟)",
    )
    .icon(icon_deploy_type())
    .build(app)?;
    let deploy_copy = IconMenuItemBuilder::with_id(
        M_DEPLOY_COPY,
        "部署接收页(复制到宿主机剪贴板)",
    )
    .icon(icon_deploy_copy())
    .build(app)?;
    let settings = IconMenuItemBuilder::with_id(M_SETTINGS, "设置…")
        .icon(icon_settings())
        .build(app)?;
    let quit = IconMenuItemBuilder::with_id(M_QUIT, "退出 ClipBeam")
        .icon(icon_quit())
        .build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&status)
        .separator()
        .item(&send_raw)
        .separator()
        .item(&send)
        .item(&recv)
        .separator()
        .item(&deploy_type)
        .separator()
        .item(&deploy_copy)
        .item(&settings)
        .separator()
        .item(&quit)
        .build()?;

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

    // macOS 27+: 显式要求显示菜单项图像(系统默认全部隐藏)
    #[cfg(target_os = "macos")]
    force_menu_icons_visible(app);

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

/// 显示托盘菜单。
pub fn _show_menu(app: &AppHandle) -> tauri::Result<()> {
    let app_clone = app.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(tray) = app_clone.tray_by_id("main") {
            let _ = tray.with_inner_tray_icon(|inner| {
                inner.show_menu();
            });
        }
    });

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

// ---------------------------------------------------------------------------
// 菜单图标:32x32 RGBA,程序化绘制,彩色方案在 macOS 明暗菜单下均可见
// ---------------------------------------------------------------------------

const MENU_ICON: u32 = 32;

/// 在 32x32 透明画布上执行绘制并转成 Tauri Image。
fn menu_icon(draw: impl Fn(&mut RgbaImage)) -> Image<'static> {
    let mut img: RgbaImage =
        ImageBuffer::from_pixel(MENU_ICON, MENU_ICON, Rgba([0, 0, 0, 0]));
    draw(&mut img);
    Image::new_owned(img.into_raw(), MENU_ICON, MENU_ICON)
}

/// 越界安全的单像素写入。
fn ico_put(img: &mut RgbaImage, x: i32, y: i32, c: Rgba<u8>) {
    if (0..MENU_ICON as i32).contains(&x) && (0..MENU_ICON as i32).contains(&y) {
        img.put_pixel(x as u32, y as u32, c);
    }
}

/// 闭区间填充矩形。
fn ico_rect(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba<u8>) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            ico_put(img, x, y, c);
        }
    }
}

/// 闭区间实心圆角矩形(r 为角半径)。
fn ico_round(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: Rgba<u8>) {
    let r2 = r * r;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let cx = x.clamp(x0 + r, x1 - r);
            let cy = y.clamp(y0 + r, y1 - r);
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 实心圆盘。
fn ico_disc(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: Rgba<u8>) {
    let r2 = r * r;
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 实心三角形(重心坐标判定)。点顺序任意。
fn ico_triangle(
    img: &mut RgbaImage,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    c: Rgba<u8>,
) {
    let minx = p0.0.min(p1.0).min(p2.0).floor() as i32;
    let maxx = p0.0.max(p1.0).max(p2.0).ceil() as i32;
    let miny = p0.1.min(p1.1).min(p2.1).floor() as i32;
    let maxy = p0.1.max(p1.1).max(p2.1).ceil() as i32;
    let sign = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (p.0 - b.0) * (a.1 - b.1) - (a.0 - b.0) * (p.1 - b.1)
    };
    for y in miny..=maxy {
        for x in minx..=maxx {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d1 = sign(p, p0, p1);
            let d2 = sign(p, p1, p2);
            let d3 = sign(p, p2, p0);
            let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            if !(has_neg && has_pos) {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 粗细为 thickness 的线段(像素中心到线段距离 <= thickness/2)。
fn ico_line(
    img: &mut RgbaImage,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thickness: f32,
    c: Rgba<u8>,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    let half = thickness / 2.0;
    let pad = half.ceil() as i32 + 1;
    let minx = x0.min(x1) as i32 - pad;
    let maxx = x0.max(x1) as i32 + pad;
    let miny = y0.min(y1) as i32 - pad;
    let maxy = y0.max(y1) as i32 + pad;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = if len2 > 0.0 {
                (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let qx = x0 + t * dx;
            let qy = y0 + t * dy;
            let ddx = px - qx;
            let ddy = py - qy;
            if ddx * ddx + ddy * ddy <= half * half {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 发送:蓝色向上箭头。
fn icon_send() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([37, 99, 235, 255]); // blue-600
        ico_triangle(img, (16.0, 4.0), (6.0, 15.0), (26.0, 15.0), c);
        ico_rect(img, 13, 15, 18, 27, c);
    })
}

/// 接收:绿色向下箭头。
fn icon_recv() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([22, 163, 74, 255]); // green-600
        ico_rect(img, 13, 5, 18, 17, c);
        ico_triangle(img, (16.0, 28.0), (6.0, 17.0), (26.0, 17.0), c);
    })
}

/// 键盘部署:紫色键盘(外框 + 两排键帽)。
fn icon_deploy_type() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([124, 58, 237, 255]); // violet-600
        let clear = Rgba([0, 0, 0, 0]);
        ico_round(img, 3, 9, 28, 24, 3, c);
        ico_round(img, 6, 12, 25, 21, 2, clear);
        for &y in &[14i32, 18] {
            for &x in &[8i32, 11, 14, 17, 20] {
                ico_rect(img, x, y, x + 1, y + 1, c);
            }
        }
    })
}

/// 复制部署:青色板夹。
fn icon_deploy_copy() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([13, 148, 136, 255]); // teal-600
        let clear = Rgba([0, 0, 0, 0]);
        // 板夹主体(环)
        ico_round(img, 7, 5, 24, 28, 3, c);
        ico_round(img, 9, 8, 22, 26, 2, clear);
        // 顶部夹子(开口朝下的 U)
        ico_round(img, 12, 2, 19, 8, 2, c);
        ico_rect(img, 14, 4, 17, 8, clear);
    })
}

/// 设置:中性灰滑块(三横线 + 圆钮),明暗菜单下均可见。
fn icon_settings() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([107, 114, 128, 255]); // gray-500
        ico_line(img, 4.0, 9.0, 18.0, 9.0, 2.5, c);
        ico_disc(img, 22, 9, 4, c);
        ico_line(img, 14.0, 16.0, 28.0, 16.0, 2.5, c);
        ico_disc(img, 10, 16, 4, c);
        ico_line(img, 4.0, 23.0, 18.0, 23.0, 2.5, c);
        ico_disc(img, 22, 23, 4, c);
    })
}

/// 退出:红色 X。
fn icon_quit() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([220, 38, 38, 255]); // red-600
        ico_line(img, 8.0, 8.0, 24.0, 24.0, 4.0, c);
        ico_line(img, 24.0, 8.0, 8.0, 24.0, 4.0, c);
    })
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

// ---------------------------------------------------------------------------
// macOS 27 兼容:NSMenu 默认隐藏菜单项图像,需逐项设置 preferredImageVisibility
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
pub fn force_menu_icons_visible(app: &App) {
    use objc2::MainThreadMarker;

    /// NSMenuItemImageVisibilityVisible(macOS 27+,旧系统忽略该消息)
    const IMAGE_VISIBILITY_VISIBLE: isize = 1;

    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let _ = tray.with_inner_tray_icon(|inner| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(status_item) = inner.ns_status_item() else {
            return;
        };
        let Some(menu) = status_item.menu(mtm) else {
            return;
        };
        unsafe {
            let n = menu.numberOfItems();
            for i in 0..n {
                if let Some(item) = menu.itemAtIndex(i) {
                    let _: () = objc2::msg_send![
                        &*item,
                        setPreferredImageVisibility: IMAGE_VISIBILITY_VISIBLE
                    ];
                }
            }
        }
    });
}
