//! 插件相关的 Tauri 命令：列表、启停、重载、刷新、日志。
//!
//! 与脚本的命令分开一个文件：脚本命令是「读文件 → 跑一次」的模型，插件命令是
//! 「常驻进程的启停 + 刷新」的模型，混在一起只会让两边都变难读。
//!
//! 每个会改变插件集合的命令都负责三件收尾：**同步托盘菜单**、**广播列表变化**、
//! **把错误原样返回给界面**（而不是只写日志 —— 用户在界面上点了开关，必须看到结果）。

use clipbeam_plugins::catalog;
use clipbeam_plugins::WindowNotice;
use tauri::{AppHandle, Manager, State};

use crate::console_panel::ConsoleLine;
use crate::plugin_manager::{PluginInfo, PluginManager};

/// 列出全部插件（会先重新扫描目录）。
#[tauri::command]
pub async fn list_plugins(
    app: AppHandle,
    state: State<'_, PluginManager>,
) -> Result<Vec<PluginInfo>, String> {
    let infos = state.refresh();
    crate::tray::sync_from_manager(&app, &state);
    Ok(infos)
}

/// 重新扫描插件目录（界面上的「刷新」）。
#[tauri::command]
pub async fn refresh_plugins(
    app: AppHandle,
    state: State<'_, PluginManager>,
) -> Result<Vec<PluginInfo>, String> {
    let infos = state.refresh();
    crate::tray::sync_from_manager(&app, &state);
    Ok(infos)
}

/// 启用一个插件。
#[tauri::command]
pub async fn enable_plugin(
    app: AppHandle,
    state: State<'_, PluginManager>,
    id: String,
) -> Result<Vec<PluginInfo>, String> {
    state.enable(&id)?;
    crate::tray::sync_from_manager(&app, &state);
    Ok(state.list())
}

/// 停用一个插件。
#[tauri::command]
pub async fn disable_plugin(
    app: AppHandle,
    state: State<'_, PluginManager>,
    id: String,
) -> Result<Vec<PluginInfo>, String> {
    state.disable(&id)?;
    crate::tray::sync_from_manager(&app, &state);
    Ok(state.list())
}

/// 重新加载（停用 + 启用）：改完插件代码后点它就生效。
#[tauri::command]
pub async fn reload_plugin(
    app: AppHandle,
    state: State<'_, PluginManager>,
    id: String,
) -> Result<Vec<PluginInfo>, String> {
    state.reload(&id)?;
    crate::tray::sync_from_manager(&app, &state);
    Ok(state.list())
}

/// 插件目录的绝对路径（界面提示「插件放哪」用）。
#[tauri::command]
pub async fn plugins_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "dir": catalog::plugins_dir_display(),
    }))
}

/// 取一个插件的日志（面板挂载或窗口重开时用）。
#[tauri::command]
pub async fn get_plugin_console(
    state: State<'_, PluginManager>,
    id: String,
) -> Result<Vec<ConsoleLine>, String> {
    Ok(state.console_snapshot(&id))
}

/// 清空一个插件的日志。
#[tauri::command]
pub async fn clear_plugin_console(
    state: State<'_, PluginManager>,
    id: String,
) -> Result<(), String> {
    state.clear_console(&id);
    Ok(())
}

// ---------------------------------------------------------------------------
// 插件窗口（前端 relay 用）
// ---------------------------------------------------------------------------

/// 插件窗口里的页面发来一条消息（前端 relay 转过来）。
///
/// 三件事：按窗口标签找到**是哪个插件的哪个窗口** → 交给那个插件的线程 →
/// 由它唤醒插件用 `$plugin.window.onMessage` 登记的回调。
#[tauri::command]
pub async fn plugin_window_message(
    app: AppHandle,
    label: String,
    message: serde_json::Value,
) -> Result<(), String> {
    let window = lookup_window(&app, &label)?;

    let manager = app
        .try_state::<PluginManager>()
        .ok_or_else(|| "插件管理器不可用".to_string())?;

    manager.deliver_window_notice(
        &window.plugin_id,
        WindowNotice::Message {
            window_id: window.plugin_window_id,
            message,
        },
    )
}

/// 插件窗口已经关闭（前端 `beforeunload` 转过来，或用户关窗后由它转过来）。
///
/// 顺手把登记表里的那一条摘掉：窗口没了，宿主不该再往那儿发消息。
/// 摘不到（已经摘过）也返回成功 —— 这个命令是**幂等**的，
/// 因为它有两条触发路径（页面 unload、Rust 侧主动关窗）。
#[tauri::command]
pub async fn plugin_window_closed(app: AppHandle, label: String) -> Result<(), String> {
    let Some(windows) = windows_of(&app) else {
        return Ok(());
    };

    let Some(window) = windows.remove(&label) else {
        return Ok(());
    };

    // 插件可能已经停用（那时线程没了，投递会失败）：这是正常的收尾顺序，不当错误
    if let Some(manager) = app.try_state::<PluginManager>() {
        if let Err(err) = manager.deliver_window_notice(
            &window.plugin_id,
            WindowNotice::Closed {
                window_id: window.plugin_window_id.clone(),
            },
        ) {
            log::debug!(
                "窗口 {label} 关闭后通知插件 {} 失败（多半是插件已停用）：{err}",
                window.plugin_id
            );
        }
    }

    Ok(())
}

/// 取窗口登记表。
fn windows_of(app: &AppHandle) -> Option<std::sync::Arc<crate::plugin_host::PluginWindows>> {
    app.try_state::<std::sync::Arc<crate::plugin_host::PluginWindows>>()
        .map(|state| state.inner().clone())
}

/// 按窗口标签找到它对应的登记项。
fn lookup_window(app: &AppHandle, label: &str) -> Result<crate::plugin_window::OpenWindow, String> {
    windows_of(app)
        .ok_or_else(|| "窗口登记表不可用".to_string())?
        .by_label(label)
        .ok_or_else(|| format!("没有登记这个窗口：{label}（可能已经关闭）"))
}
