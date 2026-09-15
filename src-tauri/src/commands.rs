//! Tauri 命令桥接层:把核心业务模块的同步调用包装成 `#[tauri::command]`,
//! 供前端 `invoke`。命令内部通过 `WorkerState` 派发任务,通过事件推送进度。

use crate::config::Config;
use crate::hotkey::HotkeyMods;
use crate::worker::{Status, TaskKind, WorkerState};
use tauri::{AppHandle, Emitter, State};

/// 读取当前配置。
#[tauri::command]
pub async fn get_config(state: State<'_, WorkerState>) -> Result<Config, String> {
    Ok(state.config.read().unwrap().clone())
}

/// 保存配置:校验 → 持久化 → 重注册热键 → 通知前端。
#[tauri::command]
pub async fn save_config(
    app: AppHandle,
    state: State<'_, WorkerState>,
    config: Config,
) -> Result<(), String> {
    let errs = config.validate();
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    config.save().map_err(|e| e.to_string())?;
    *state.config.write().unwrap() = config.clone();
    crate::hotkey::reregister(&app, &config).map_err(|e| e.to_string())?;
    let _ = app.emit("config-saved", &config);
    crate::notify::notify("ClipBeam", "设置已保存,热键已重注册");
    Ok(())
}

/// 启动发送任务(协议 A)。
#[tauri::command]
pub async fn start_send(app: AppHandle, state: State<'_, WorkerState>) -> Result<(), String> {
    state.start(TaskKind::Send, app).await
}

/// 启动接收任务(协议 B)。
#[tauri::command]
pub async fn start_recv(app: AppHandle, state: State<'_, WorkerState>) -> Result<(), String> {
    state.start(TaskKind::Recv, app).await
}

/// 启动键盘部署任务(协议 C,type 版)。
#[tauri::command]
pub async fn start_deploy_type(
    app: AppHandle,
    state: State<'_, WorkerState>,
) -> Result<(), String> {
    state.start(TaskKind::DeployType, app).await
}

/// 复制自解压接收页到宿主机剪贴板(协议 C,copy 版,无后台任务)。
#[tauri::command]
pub async fn deploy_copy() -> Result<usize, String> {
    crate::deploy::copy_bootstrap()?;
    Ok(crate::deploy::sizes().0)
}

/// 中止当前任务(如有)。
#[tauri::command]
pub async fn cancel_task(app: AppHandle, state: State<'_, WorkerState>) -> Result<(), String> {
    state.cancel(&app).await;
    Ok(())
}

/// 查询当前任务状态快照。
#[tauri::command]
pub async fn get_task_status(state: State<'_, WorkerState>) -> Result<Status, String> {
    Ok(state.status())
}

/// 前端捕获热键后,把物理键名 + 修饰键传给后端,后端转成规范字符串并校验。
#[tauri::command]
pub async fn capture_hotkey(code: String, mods: HotkeyMods) -> Result<String, String> {
    crate::hotkey::spec_from_frontend(&code, &mods)
        .ok_or_else(|| "无法识别该组合键".into())
}
