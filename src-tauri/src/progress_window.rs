//! 进度窗口的摆放：默认落在**鼠标所在显示器**的右上角，用户拖动过就记住（本次运行内），
//! 并在显示器拔掉 / 分辨率变化后把位置夹回可见区域。
//!
//! # 为什么放在 Rust
//!
//! 窗口是启动时创建的隐藏窗口，`show()` 由 worker 触发。在这里「先定位再 show」可以避免
//! 窗口在旧位置闪一下；前端因此不需要碰显示器信息，只管内容。
//!
//! # 位置策略
//!
//! * **没拖动过**：包含鼠标光标的显示器的 `work_area` 右上角（`work_area` 已排除 macOS
//!   菜单栏与 Windows 任务栏），边距 [`MARGIN`]；光标读不到就退到窗口当前显示器 → 主显示器。
//! * **拖动过**：用他拖到的位置（本次运行内记住），每次显示时夹回可见区域；
//!   完全不在任何显示器内（例如副屏被拔掉）就退回默认位置。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, WebviewWindow,
    WindowEvent,
};

/// 窗口距显示器 `work_area` 边缘的边距（物理像素）。
const MARGIN: i32 = 16;

/// 用户拖出来的位置（`None` = 还没拖过，用默认位置）。
static USER_POSITION: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);
/// 我们最近一次替窗口设置的位置：用来把「程序摆放」与「用户拖动」区分开。
static LAST_PLACED: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);
/// `Moved` 监听只需要注册一次（窗口活到应用退出）。
static WATCHING: AtomicBool = AtomicBool::new(false);
/// 「用户碰过窗口」的监听同样只注册一次。
static INTERACTION_WATCHED: AtomicBool = AtomicBool::new(false);

/// 摆好位置并显示进度窗口（`worker.rs` 的任务启动路径调用）。
pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window("progress") else {
        return;
    };
    watch_user_drag(&window);
    watch_user_interaction(app, &window);
    place(app, &window);
    let _ = window.show();
}

/// 用户点过/拖过窗口 → 告诉前端「这是他在关心结果，别自动关」。
///
/// 为什么不能只靠前端的 `pointerdown`：macOS 上点击一个**未激活**的窗口时，那一下鼠标
/// 事件可能被系统吞掉（只把窗口激活，不投递给 webview）—— 前端收不到，于是任务结束后
/// 照样自动隐藏。窗口**获得焦点**是同一动作更可靠的信号（被吞掉的点击也会让窗口变 key）。
fn watch_user_interaction(app: &AppHandle, window: &WebviewWindow) {
    if INTERACTION_WATCHED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(true) = event {
            // 只发给进度窗口自己（它跑的是同一套 Vue 应用，广播会打扰别的窗口）
            let _ = app.emit_to("progress", "progress-interacted", ());
        }
    });
}

/// 记住用户拖出来的位置。
///
/// `WindowEvent::Moved` 对「程序设置的位置」也会触发，所以用 [`LAST_PLACED`] 对比：
/// 与我们刚设置的不同 = 用户在拖。
fn watch_user_drag(window: &WebviewWindow) {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    window.on_window_event(|event| {
        if let WindowEvent::Moved(position) = event {
            let ours = LAST_PLACED.lock().unwrap();
            if ours.as_ref() != Some(position) {
                drop(ours);
                *USER_POSITION.lock().unwrap() = Some(*position);
                // 同一次拖动里后续的 Moved 事件位置相同，记下来就不会重复写
                *LAST_PLACED.lock().unwrap() = Some(*position);
            }
        }
    });
}

/// 按策略摆放窗口（不负责显示）。
fn place(app: &AppHandle, window: &WebviewWindow) {
    let Ok(size) = window.outer_size() else {
        return;
    };
    let remembered = *USER_POSITION.lock().unwrap();
    let target = match remembered {
        Some(position) => {
            let areas = work_areas(app);
            clamp_into(position, size, &areas).or_else(|| default_position(app, window, size))
        }
        None => default_position(app, window, size),
    };
    if let Some(position) = target {
        // 先记下「这是我们设的」，再设置：紧随其后的 Moved 事件就不会被当成用户拖动
        *LAST_PLACED.lock().unwrap() = Some(position);
        let _ = window.set_position(position);
    }
}

/// 默认位置：鼠标所在显示器（退路：窗口当前显示器 → 主显示器）的右上角。
fn default_position(
    app: &AppHandle,
    window: &WebviewWindow,
    size: PhysicalSize<u32>,
) -> Option<PhysicalPosition<i32>> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;
    Some(top_right(to_area(*monitor.work_area()), size))
}

/// 所有显示器的可用区域（拿不到就空列表，调用方回退默认位置）。
fn work_areas(app: &AppHandle) -> Vec<Area> {
    app.available_monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|monitor| to_area(*monitor.work_area()))
        .collect()
}

fn to_area(rect: PhysicalRect<i32, u32>) -> Area {
    Area {
        x: rect.position.x,
        y: rect.position.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// 一块可用区域（显示器 work_area），全物理像素。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Area {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// 区域右上角摆放（留出 [`MARGIN`] 边距）。
fn top_right(area: Area, window: PhysicalSize<u32>) -> PhysicalPosition<i32> {
    PhysicalPosition::new(
        area.x + area.width as i32 - window.width as i32 - MARGIN,
        area.y + MARGIN,
    )
}

/// 把位置夹进某块显示器的可用区域；完全不在任何区域内时返回 `None`。
///
/// 「在区域内」的判定用一块固定的可抓取区域（`GRABBABLE`），只要它有一部分落在显示器里
/// 就算还在 —— 拖到屏幕边缘的窗口不该被强行挪走。
fn clamp_into(
    position: PhysicalPosition<i32>,
    window: PhysicalSize<u32>,
    areas: &[Area],
) -> Option<PhysicalPosition<i32>> {
    /// 至少要有这么多像素留在屏幕内，才认为窗口还看得见（够抓住它拖回来）。
    const GRABBABLE: i32 = 48;

    for area in areas {
        let left = area.x;
        let top = area.y;
        let right = left + area.width as i32;
        let bottom = top + area.height as i32;

        let visible = position.x + window.width.min(GRABBABLE as u32) as i32 > left
            && position.x < right
            && position.y + window.height.min(GRABBABLE as u32) as i32 > top
            && position.y < bottom;
        if !visible {
            continue;
        }

        // 还在（至少一块在）这块屏上：只把跑出边界的部分夹回来
        let max_x = (right - window.width as i32).max(left);
        let max_y = (bottom - window.height as i32).max(top);
        return Some(PhysicalPosition::new(
            position.x.clamp(left, max_x),
            position.y.clamp(top, max_y),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: PhysicalSize<u32> = PhysicalSize::new(420, 96);

    #[test]
    fn default_position_sits_at_top_right_of_work_area() {
        // 主屏：1920×1080，任务栏/菜单栏扣掉后可用区从 (0, 25) 开始
        let area = Area {
            x: 0,
            y: 25,
            width: 1920,
            height: 1030,
        };
        assert_eq!(
            top_right(area, WINDOW),
            PhysicalPosition::new(1920 - WINDOW.width as i32 - MARGIN, 25 + MARGIN)
        );
    }

    #[test]
    fn default_position_handles_monitor_left_of_primary() {
        // 副屏在主屏左边：坐标是负的
        let area = Area {
            x: -1280,
            y: 0,
            width: 1280,
            height: 800,
        };
        assert_eq!(
            top_right(area, WINDOW),
            PhysicalPosition::new(-1280 + 1280 - WINDOW.width as i32 - MARGIN, MARGIN)
        );
    }

    #[test]
    fn clamp_keeps_position_inside() {
        let area = Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let position = PhysicalPosition::new(1500, 40);
        assert_eq!(clamp_into(position, WINDOW, &[area]), Some(position));
    }

    #[test]
    fn clamp_pulls_back_window_that_runs_off_the_edge() {
        let area = Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // 右/下都跑出去了 → 夹回右上角内侧
        let clamped = clamp_into(PhysicalPosition::new(1900, 1070), WINDOW, &[area]);
        assert_eq!(
            clamped,
            Some(PhysicalPosition::new(
                1920 - WINDOW.width as i32,
                1080 - WINDOW.height as i32
            ))
        );
    }

    #[test]
    fn clamp_returns_none_when_monitor_is_gone() {
        // 记住的位置来自已被拔掉的副屏 → 交给调用方回退默认位置
        let only_primary = Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            clamp_into(PhysicalPosition::new(-1200, 100), WINDOW, &[only_primary]),
            None
        );
    }

    #[test]
    fn clamp_returns_none_without_monitors() {
        assert_eq!(clamp_into(PhysicalPosition::new(10, 10), WINDOW, &[]), None);
    }
}
