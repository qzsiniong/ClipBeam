//! 插件相关的 Tauri 命令：列表、启停、重载、刷新、日志。
//!
//! 与脚本的命令分开一个文件：脚本命令是「读文件 → 跑一次」的模型，插件命令是
//! 「常驻进程的启停 + 刷新」的模型，混在一起只会让两边都变难读。
//!
//! 每个会改变插件集合的命令都负责三件收尾：**同步托盘菜单**、**广播列表变化**、
//! **把错误原样返回给界面**（而不是只写日志 —— 用户在界面上点了开关，必须看到结果）。

use clipbeam_plugins::catalog;
use tauri::{AppHandle, State};

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
