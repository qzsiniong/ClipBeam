//! Tauri 命令桥接层:把核心业务模块的同步调用包装成 `#[tauri::command]`,
//! 供前端 `invoke`。命令内部通过 `WorkerState` 派发任务,通过事件推送进度。

use crate::config::Config;
use crate::hotkey::HotkeyMods;
use crate::script_runner;
use crate::worker::{Status, TaskKind, WorkerState};
use clipbeam_scripting::scripts::ScriptMeta;
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
    // 按当前忙闲状态用新配置重建热键(忙时保留 Esc + 当前任务热键)
    let mode = match state.current_kind().await {
        Some(kind) => crate::hotkey::HotkeyMode::Busy(kind),
        None => crate::hotkey::HotkeyMode::Idle,
    };
    crate::hotkey::set_mode(&app, &config, mode).map_err(|e| e.to_string())?;
    let _ = app.emit("config-saved", &config);
    crate::notify::notify("ClipBeam", "设置已保存,热键已重注册");
    Ok(())
}
/// 启动发送任务(无协议,原样发送)。
#[tauri::command]
pub async fn start_send_raw(app: AppHandle, state: State<'_, WorkerState>) -> Result<(), String> {
    state.start(TaskKind::SendRaw, app).await
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
    crate::hotkey::spec_from_frontend(&code, &mods).ok_or_else(|| "无法识别该组合键".into())
}

/// 查询开机自启动状态(以系统登录项/注册表的实际状态为准)。
#[tauri::command]
pub async fn get_autostart(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("读取自启动状态失败: {e}"))
}

/// 立即启用/关闭开机自启动(写入 macOS LoginAgent / Windows 注册表 / Linux desktop entry)。
#[tauri::command]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let current = manager.is_enabled().unwrap_or(false);
    if enabled && !current {
        manager
            .enable()
            .map_err(|e| format!("启用自启动失败: {e}"))?;
    } else if !enabled && current {
        manager
            .disable()
            .map_err(|e| format!("关闭自启动失败: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 脚本（JS/TS）：目录 CRUD + 运行
// ---------------------------------------------------------------------------

/// 列出脚本目录里的脚本（按文件名排序）。
#[tauri::command]
pub async fn list_scripts() -> Result<Vec<ScriptMeta>, String> {
    script_runner::list()
}

/// 读取脚本源码。
#[tauri::command]
pub async fn read_script(name: String) -> Result<String, String> {
    script_runner::read(&name)
}

/// 保存脚本源码。
#[tauri::command]
pub async fn write_script(name: String, source: String) -> Result<(), String> {
    script_runner::write(&name, &source)
}

/// 删除脚本。
#[tauri::command]
pub async fn delete_script(name: String) -> Result<(), String> {
    script_runner::delete(&name)
}

/// 脚本目录路径 + 内置示例状态（供前端提示「文件在哪」）。
#[tauri::command]
pub async fn scripts_info() -> Result<serde_json::Value, String> {
    let seeded = script_runner::seed()?;
    Ok(serde_json::json!({
        "dir": script_runner::dir_display(),
        "seeded": seeded,
    }))
}

/// 当前可用的脚本能力清单（CodeMirror 补全的数据源）。
#[tauri::command]
pub async fn list_capabilities() -> Result<Vec<clipbeam_scripting::Capability>, String> {
    Ok(crate::scripting::capability_list())
}

/// 运行一个脚本：读文件 → 必要时 TS 转译 → 交给 Worker 执行（待命窗口 → 逐键输出）。
#[tauri::command]
pub async fn start_script(
    app: AppHandle,
    state: State<'_, WorkerState>,
    name: String,
) -> Result<(), String> {
    // 先做「读 + 转译」：语法错误要立刻反馈，不要等用户点完待命窗口才报错
    let source = script_runner::load(&name)?;

    state.set_pending_script(crate::worker::ScriptRequest {
        name: name.clone(),
        source,
    });

    // 启动失败（例如已有任务在跑）时要把请求清掉：否则下一次启动脚本任务会
    // 取到这次残留的内容，跑错文件。
    if let Err(e) = state.start(TaskKind::Script, app).await {
        state.clear_pending_script();
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 脚本窗口 / Console 面板
// ---------------------------------------------------------------------------

/// 打开(或聚焦)脚本编辑窗口。
#[tauri::command]
pub async fn open_scripts_window(app: AppHandle) -> Result<(), String> {
    crate::open_scripting_window(&app);
    Ok(())
}

/// 显示并聚焦主窗口(脚本窗口侧边栏的「总览 / 设置」用)。
#[tauri::command]
pub async fn open_main_window(app: AppHandle) -> Result<(), String> {
    use tauri::Manager;

    match app.get_webview_window("main") {
        Some(window) => {
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())
        }
        None => Err("找不到主窗口".to_string()),
    }
}

/// 取脚本 Console 面板的全部输出(面板挂载或窗口重开时用)。
#[tauri::command]
pub async fn get_script_console(
    state: State<'_, WorkerState>,
) -> Result<Vec<crate::console_panel::ConsoleLine>, String> {
    Ok(state.console.snapshot())
}

/// 清空脚本 Console 面板。
#[tauri::command]
pub async fn clear_script_console(state: State<'_, WorkerState>) -> Result<(), String> {
    state.console.clear();
    Ok(())
}
