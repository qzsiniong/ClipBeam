//! 全局热键:用 `tauri-plugin-global-shortcut` 替代裸 `global-hotkey` crate。
//! 策略简化:Rust 侧固定注册三个热键,按下时 emit "hotkey" 事件 + 语义 id
//! (send/recv/stop);前端监听事件后根据当前 worker 状态决定 start/cancel。
//! 这样 Rust 侧无需做忙时热键切换注册。

use crate::config::Config;
use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const ID_SEND: &str = "send";
const ID_RECV: &str = "recv";
const ID_STOP: &str = "stop";

type GsResult<T> = Result<T, tauri_plugin_global_shortcut::Error>;

/// 注册三个全局热键(空闲模式:send + recv + stop)。
/// 重复注册会失败,调用前应先 `unregister_all`。
pub fn register(app: &AppHandle, cfg: &Config) -> GsResult<()> {
    let gs = app.global_shortcut();

    let spec_send = cfg.send_hotkey.clone();
    gs.on_shortcut(spec_send.as_str(), move |app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let _ = app.emit("hotkey", ID_SEND);
        }
    })?;

    let spec_recv = cfg.recv_hotkey.clone();
    gs.on_shortcut(spec_recv.as_str(), move |app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let _ = app.emit("hotkey", ID_RECV);
        }
    })?;

    let spec_stop = cfg.stop_hotkey.clone();
    gs.on_shortcut(spec_stop.as_str(), move |app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let _ = app.emit("hotkey", ID_STOP);
        }
    })?;

    Ok(())
}

/// 注销全部热键后重新注册(配置变更后用)。
pub fn reregister(app: &AppHandle, cfg: &Config) -> GsResult<()> {
    let gs = app.global_shortcut();
    gs.unregister_all()?;
    register(app, cfg)
}

/// 前端捕获热键后,把物理键名 + 修饰键转成 global-shortcut 可解析的规范字符串。
/// 复用原 settings.rs::hotkey_spec 的规范化逻辑:
/// 输入 code="KeyK", mods={ctrl:true,shift:true} → 输出 "Cmd+Shift+KeyK"(macOS)
/// 或 "Ctrl+Shift+KeyK"(其他平台)。
pub fn spec_from_frontend(code: &str, mods: &HotkeyMods) -> Option<String> {
    // 前端传的 e.code 已经是 "KeyK" / "Escape" / "Digit1" 格式,
    // 与原 winit KeyCode 的 Debug 名一致,直接用。
    let name = code;
    let mut parts: Vec<&str> = Vec::new();
    #[cfg(target_os = "macos")]
    let primary = "Cmd";
    #[cfg(not(target_os = "macos"))]
    let primary = "Ctrl";
    if mods.super_ || mods.ctrl {
        parts.push(primary);
    }
    if mods.shift {
        parts.push("Shift");
    }
    if mods.alt {
        parts.push("Alt");
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
