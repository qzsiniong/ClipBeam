//! 全局热键:用 `tauri-plugin-global-shortcut` 替代裸 `global-hotkey` crate。
//! 动态注册,避免空闲时拦截 Esc 等单键影响其他应用:
//! - 空闲:仅注册 send/recv 触发键(stop/Esc 不注册,透传给前台应用);
//! - 忙时:注销触发键,改注册 stop(Esc)+ 当前任务热键(再按即停),二者均取消任务。
//! 启动失败通过系统通知反馈。

use crate::{commands, config::Config, notify, worker};
use tauri::{async_runtime::spawn, AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

type GsResult<T> = Result<T, tauri_plugin_global_shortcut::Error>;

/// 热键注册模式。
pub enum HotkeyMode {
    /// 空闲:只注册发送/接收触发键。
    Idle,
    /// 忙时:注册中止键 + 该任务自己的触发热键(DeployType 无触发热键)。
    Busy(worker::TaskKind),
}

/// 按模式重建全部热键。内部先 unregister_all,可安全重复调用。
/// 注册失败返回错误,由调用方决定是否提示(不应阻断任务生命周期)。
pub fn set_mode(app: &AppHandle, cfg: &Config, mode: HotkeyMode) -> GsResult<()> {
    let gs = app.global_shortcut();
    gs.unregister_all()?;
    match mode {
        HotkeyMode::Idle => {
            gs.on_shortcut(cfg.send_hotkey.as_str(), on_start_send)?;
            gs.on_shortcut(cfg.recv_hotkey.as_str(), on_start_recv)?;
        }
        HotkeyMode::Busy(kind) => {
            gs.on_shortcut(cfg.stop_hotkey.as_str(), on_cancel)?;
            match kind {
                worker::TaskKind::SendRaw => {
                    gs.on_shortcut(cfg.send_raw_hotkey.as_str(), on_cancel)?;
                }
                worker::TaskKind::Send => {
                    gs.on_shortcut(cfg.send_hotkey.as_str(), on_cancel)?;
                }
                worker::TaskKind::Recv => {
                    gs.on_shortcut(cfg.recv_hotkey.as_str(), on_cancel)?;
                }
                worker::TaskKind::DeployType => {}
            }
        }
    }
    Ok(())
}

/// 空闲模式:发送热键 → 启动发送任务。
fn on_start_send(app: &AppHandle, _sc: &Shortcut, event: ShortcutEvent) {
    if event.state() == ShortcutState::Pressed {
        let app_clone = app.clone();
        spawn(async move {
            let state = app_clone.state::<worker::WorkerState>();
            if let Err(e) = commands::start_send(app_clone.clone(), state).await {
                notify::notify("ClipBeam", &e);
            }
        });
    }
}

/// 空闲模式:接收热键 → 启动接收任务。
fn on_start_recv(app: &AppHandle, _sc: &Shortcut, event: ShortcutEvent) {
    if event.state() == ShortcutState::Pressed {
        let app_clone = app.clone();
        spawn(async move {
            let state = app_clone.state::<worker::WorkerState>();
            if let Err(e) = commands::start_recv(app_clone.clone(), state).await {
                notify::notify("ClipBeam", &e);
            }
        });
    }
}

/// 忙时模式:Esc 或当前任务热键 → 取消任务。
fn on_cancel(app: &AppHandle, _sc: &Shortcut, event: ShortcutEvent) {
    if event.state() == ShortcutState::Pressed {
        let app_clone = app.clone();
        spawn(async move {
            let state = app_clone.state::<worker::WorkerState>();
            let _ = commands::cancel_task(app_clone.clone(), state).await;
        });
    }
}

/// 前端捕获热键后,把物理键名 + 修饰键转成 global-shortcut 可解析的规范字符串。
/// 修饰键按物理语义独立映射(Ctrl→Ctrl、Meta/Win→Super),不做跨平台偷换:
/// 输入 code="KeyK", mods={super_:true,shift:true} → "Super+Shift+KeyK"
/// (macOS 注册为 Cmd+Shift+K,Windows 上 Ctrl 组合则为 "Ctrl+Shift+KeyK")。
pub fn spec_from_frontend(code: &str, mods: &HotkeyMods) -> Option<String> {
    // 前端传的 e.code 已经是 "KeyK" / "Escape" / "Digit1" 格式,
    // 与原 winit KeyCode 的 Debug 名一致,直接用。
    let name = code;
    let mut parts: Vec<&str> = Vec::new();
    // 修饰键必须独立映射,不能用 || 合并:
    // 否则 macOS 上 Ctrl 会被偷换成 Cmd、Windows 上 Super 会被偷换成 Ctrl。
    // global-hotkey 解析时 SUPER/CMD/COMMAND 同义(平台各自映射)。
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.super_ {
        parts.push("Super");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    let spec = if parts.is_empty() {
        name.to_string()
    } else {
        format!("{}+{name}", parts.join("+"))
    };
    // 用 global-shortcut 的 Shortcut 类型校验可解析性
    Shortcut::try_from(spec.as_str()).ok()?;
    Some(spec)
}

/// 修饰键状态(前端 KeyboardEvent 的布尔字段)。
#[derive(serde::Deserialize, Clone, Copy)]
pub struct HotkeyMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_: bool,
}
