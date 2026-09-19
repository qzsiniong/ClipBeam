//! ClipBeam 库:Tauri 2 应用初始化 + CLI 子命令分流。
//!
//! 核心业务模块(send/receive/deploy/protocol/config/cancel/typer/keymap/notify)
//! 与 GUI 解耦,本文件只做编排:把它们通过 `#[tauri::command]` 桥接给前端,
//! 通过 Tauri 事件系统推送 worker 进度,通过 Tauri tray/global-shortcut API
//! 替代原 winit + tray-icon + global-hotkey 三件套。

mod cancel;
mod commands;
mod config;
mod console_panel;
mod deploy;
mod hotkey;
mod keymap;
mod notify;
mod protocol;
mod receive;
mod script_runner;
mod scripting;
mod send;
mod standby;
mod tray;
mod typer;
mod worker;

use clap::{Parser, Subcommand};
use config::Config;
use tauri::{async_runtime::spawn, Emitter, Manager};

// ---------------------------------------------------------------------------
// CLI 子命令(联调用)
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "clipbeam", about = "宿主机 ↔ 远程浏览器剪贴板桥")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// 立即执行一次"发送本机剪贴板到远程"(联调用;请先把焦点切到远程接收页)
    SendOnce,
    /// 立即执行一次"截屏接收远程二维码"(联调用)
    RecvOnce,
    /// 生成自解压接收页并逐键打进当前焦点窗口(请先切到远程记事本)
    DeployType,
    /// 生成自解压接收页并复制到宿主机剪贴板(远程桌面支持剪贴板同步时用)
    DeployCopy,
    /// 在终端里运行一个脚本(.js / .ts);TS 会在进程内用 oxc 转译
    Script {
        /// 脚本文件路径
        path: std::path::PathBuf,
        /// 原样把所有内容打出来(默认按 ClipBeam 的键盘打字节奏逐字输出)
        #[arg(long)]
        raw: bool,
    },
}

/// CLI 里运行脚本:用终端宿主(`CliScriptHost`)驱动同一套引擎与能力集。
///
/// 与 GUI 的唯一区别就是宿主实现:这里 `$.typeStr` 写终端、`$.confirm` 读 stdin。
fn run_cli_script(path: &std::path::Path, raw: bool, token: &cancel::CancellationToken) {
    // 首次运行顺带把内置示例落到脚本目录,方便用户照抄
    match clipbeam_scripting::scripts::ensure_seed_scripts() {
        Ok(0) => {}
        Ok(n) => eprintln!("已在 {} 写入 {n} 个内置示例脚本", script_runner::dir_display()),
        Err(e) => eprintln!("提示:脚本目录初始化失败({e}),不影响本次运行"),
    }

    // CLI 允许任意路径（GUI 才用「脚本目录 + 文件名」那套校验）
    let display_name = path.to_string_lossy().into_owned();
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("✗ 读取脚本 {display_name} 失败：{e}");
            std::process::exit(1);
        }
    };
    let source = match script_runner::transpile_if_needed(&display_name, &source) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("✗ TypeScript 转译失败：\n{e}");
            std::process::exit(1);
        }
    };

    // `--raw`：按整段写出（不走逐字打字机节奏），适合重定向到文件/管道
    let host = std::sync::Arc::new(clipbeam_scripting::cmd::CliScriptHost::with_chunked(
        script_runner::engine_cancel(token),
        raw,
    ));
    let name = display_name;
    let started = std::time::Instant::now();

    // 引擎需要 tokio 上下文;CLI 没有 Tauri 运行时,这里临时建一个
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("✗ 无法创建异步运行时:{e}");
            std::process::exit(1);
        }
    };

    let typed = host.clone();
    let result = runtime.block_on(async move {
        script_runner::run_source(
            &name,
            &source,
            host as std::sync::Arc<dyn clipbeam_script::ScriptHost>,
            clipbeam_script::CancellationToken::new(),
        )
        .await
    });

    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(()) => println!(
            "\n✓ 脚本执行完成:输出 {} 个字符,耗时 {elapsed_ms}ms",
            typed.typed_chars()
        ),
        Err(e) => {
            let text = script_runner::describe_error(&e);
            eprintln!(
                "\n✗ {text}(已输出 {} 个字符,耗时 {elapsed_ms}ms)",
                typed.typed_chars()
            );
            std::process::exit(1);
        }
    }
}

/// 打开(或聚焦)脚本编辑窗口。
///
/// 脚本有独立的 **aside 布局大窗口**(见 tauri.conf.json 的 `scripting`),主窗口不再承载脚本页。
/// 三个入口都走这里:托盘「脚本编辑器…」、主窗口侧边栏「脚本」、仪表盘「运行脚本」。
pub fn open_scripting_window(app: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(window) = app.get_webview_window("scripting") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        // 显示脚本窗口期间需要在 Dock 里出现(见 `sync_dock_icon`)
        sync_dock_icon(app);
    } else {
        log::warn!("找不到 scripting 窗口(检查 tauri.conf.json)");
    }
}

/// 按**脚本窗口是否可见**同步 Dock 图标(macOS)。
///
/// 背景:本应用是托盘常驻程序,启动时把激活策略设成 `Accessory`,因此平时不占 Dock
/// (见 `setup`)。但脚本编辑器是个会长时间停留、还会被用户丢到别的屏幕的工作窗口,
/// 没有 Dock 图标就无法用 Cmd+Tab 切回来 —— 所以在它可见期间要显示 Dock 图标。
///
/// 用 [`tauri::AppHandle::set_dock_visibility`] 而不是再调一次 `set_activation_policy`:
/// tao 的 `set_dock_visibility` 内部带了防抖(切换太快会让 macOS 留下多个 Dock 图标)。
///
/// 幂等,可以随意重复调用;非 macOS 平台是空实现。
pub fn sync_dock_icon(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let visible = app
            .get_webview_window("scripting")
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        if let Err(e) = app.set_dock_visibility(visible) {
            log::warn!("同步 Dock 图标失败(visible={visible}): {e}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// 哪些窗口「可见时需要在 Dock 里出现」。
///
/// 目前只有脚本编辑器:主窗口是常见的托盘弹窗、进度/待命是浮动小窗,都不该占 Dock。
/// 抽成函数是为了能直接单测(不涉及任何平台 API)。
fn window_wants_dock_icon(label: &str) -> bool {
    label == "scripting"
}

/// CLI 入口:执行单次子命令(无 Tauri 事件循环)。
pub fn run_cli(cmd: Command) {
    let cfg = Config::load();
    let token = cancel::CancellationToken::new();
    match cmd {
        Command::SendOnce => {
            eprintln!("请确认焦点已在远程接收页,0.2 秒后开始发送(Ctrl-C 可强杀进程)……");
            match send::run_once(&cfg, false, &token, true, |_, _| {}) {
                send::SendReport::Done {
                    frame_chars,
                    text_bytes,
                } => {
                    eprintln!("✓ 发送完成:原始 {text_bytes} 字节,共敲入 {frame_chars} 个字符");
                }
                send::SendReport::Cancelled { sent } => {
                    eprintln!("✗ 已中止:约 {sent} 个字符可能已落入当前焦点窗口,请人工检查")
                }
                send::SendReport::Error(e) => eprintln!("✗ 发送失败:{e}"),
            }
        }
        Command::RecvOnce => {
            eprintln!(
                "开始截屏接收(超时 {}s,Ctrl-C 强杀进程)……请在远程页面循环播放二维码",
                cfg.receive_timeout_s
            );
            let report = receive::run_once(&cfg, &token, |got, total| {
                eprint!("\r已收集 {got}/{total} 帧   ");
                use std::io::Write;
                let _ = std::io::stderr().flush();
            });
            eprintln!();
            match report {
                receive::RecvReport::Done { frames, text_bytes } => {
                    eprintln!("✓ 接收完成:{frames} 帧,{text_bytes} 字节已写入本机剪贴板")
                }
                receive::RecvReport::Cancelled { got } => {
                    eprintln!("✗ 已中止(已收集 {got} 帧)")
                }
                receive::RecvReport::Timeout { got } => {
                    eprintln!("✗ 超时未收齐(已收集 {got} 帧);请确认二维码正在播放后重试")
                }
                receive::RecvReport::Error(e) => eprintln!("✗ 接收失败:{e}"),
            }
        }
        Command::DeployType => {
            let (chars, page_bytes) = deploy::sizes();
            let secs = chars as f64 * cfg.key_delay_ms as f64 / 1000.0;
            eprintln!(
                "请在远程打开记事本并聚焦,{:.1} 秒后开始敲入自解压接收页(共 {chars} 字符,约 {secs:.0} 秒,内嵌页面 {page_bytes} 字节)。Ctrl-C 可强杀。",
                cfg.settle_ms as f64 / 1000.0
            );
            match deploy::type_bootstrap(&cfg, &token, true, |_, _| {}) {
                typer::TypeResult::Completed(n) => {
                    eprintln!("✓ 引导包输入完成({n} 字符)。请在远程把记事本内容另存为 clipbeam.html(编码 UTF-8),双击打开即可。")
                }
                typer::TypeResult::Cancelled(n) => {
                    eprintln!("✗ 已中止:约 {n} 字符可能已落入记事本,请清空后重试")
                }
                typer::TypeResult::Failed(_, e) => eprintln!("✗ 部署失败:{e}"),
            }
        }
        Command::Script { path, raw } => {
            run_cli_script(&path, raw, &token);
        }
        Command::DeployCopy => {
            let (chars, page_bytes) = deploy::sizes();
            match deploy::copy_bootstrap() {
                Ok(()) => eprintln!(
                    "✓ 自解压接收页({chars} 字符,内嵌 {page_bytes} 字节)已复制到宿主机剪贴板,请在远程粘贴、另存为 .html 后打开"
                ),
                Err(e) => eprintln!("✗ 复制失败:{e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri 2 应用入口
// ---------------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 脚本 $.confirm 用系统原生确认框(见 scripting.rs)
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // macOS: 托盘常驻模式,不占 Dock(Tauri 默认是 Regular 策略)
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
            let cfg = Config::load();
            // 首次启动把内置示例脚本写进脚本目录(已存在的文件不覆盖)
            match clipbeam_scripting::scripts::ensure_seed_scripts() {
                Ok(0) => {}
                Ok(n) => log::info!("已写入 {n} 个内置示例脚本到 {}", script_runner::dir_display()),
                Err(e) => log::warn!("初始化脚本目录失败: {e}"),
            }
            tray::build(app, &cfg)?;
            // 启动时为空闲模式:仅注册发送/接收热键,Esc 不拦截
            hotkey::set_mode(app.handle(), &cfg, hotkey::HotkeyMode::Idle)?;
            app.manage(worker::WorkerState::new(cfg, app.handle().clone()));
            Ok(())
        })
        .on_tray_icon_event(tray::on_tray_event)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                tray::M_QUIT => app.exit(0),
                tray::M_SEND_RAW => {
                    let app_clone = app.clone();
                    spawn(async move {
                        if let Err(e) = commands::start_send_raw(
                            app_clone.clone(),
                            app_clone.state::<worker::WorkerState>(),
                        )
                        .await
                        {
                            crate::notify::notify("ClipBeam", &e);
                        }
                    });
                }
                tray::M_DEPLOY_TYPE => {
                    let app_clone = app.clone();
                    spawn(async move {
                        if let Err(e) = commands::start_deploy_type(
                            app_clone.clone(),
                            app_clone.state::<worker::WorkerState>(),
                        )
                        .await
                        {
                            crate::notify::notify("ClipBeam", &e);
                        }
                    });
                }
                tray::M_DEPLOY_COPY => {
                    spawn(async move {
                        match commands::deploy_copy().await {
                            Ok(chars) => crate::notify::notify(
                                "ClipBeam",
                                &format!("自解压接收页({chars} 字符)已复制到宿主机剪贴板"),
                            ),
                            Err(e) => crate::notify::notify("ClipBeam", &format!("复制失败: {e}")),
                        }
                    });
                }
                tray::M_RECV => {
                    let app_clone = app.clone();
                    spawn(async move {
                        if let Err(e) = commands::start_recv(
                            app_clone.clone(),
                            app_clone.state::<worker::WorkerState>(),
                        )
                        .await
                        {
                            crate::notify::notify("ClipBeam", &e);
                        }
                    });
                }
                tray::M_SEND => {
                    let app_clone = app.clone();
                    spawn(async move {
                        if let Err(e) = commands::start_send(
                            app_clone.clone(),
                            app_clone.state::<worker::WorkerState>(),
                        )
                        .await
                        {
                            crate::notify::notify("ClipBeam", &e);
                        }
                    });
                }
                tray::M_SCRIPTS => open_scripting_window(app),
                tray::M_SETTINGS => {
                    // 只发给主窗口。`app.emit` 会广播给**所有**窗口，而脚本窗口跑的是同一套
                    // Vue 应用（同一个 router），广播会让它也跟着导航到 /settings ——
                    // 于是就出现「两个窗口都在显示设置」。设置只属于主窗口。
                    let _ = app.emit_to("main", "tray-menu", id);
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {
                    // 其余菜单项暂未实现，统一转给主窗口处理
                    let _ = app.emit_to("main", "tray-menu", id);
                }
            }
        })
        .on_window_event(|window, event| {
            // 关闭按钮 → 隐藏到托盘(不退出)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // 脚本窗口藏起来后不再有 Dock 图标;Ctx 就是窗口所属的 AppHandle
                if window_wants_dock_icon(window.label()) {
                    sync_dock_icon(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::start_send_raw,
            commands::start_send,
            commands::start_recv,
            commands::start_deploy_type,
            commands::deploy_copy,
            commands::cancel_task,
            commands::get_task_status,
            commands::capture_hotkey,
            commands::get_autostart,
            commands::set_autostart,
            commands::list_scripts,
            commands::read_script,
            commands::write_script,
            commands::delete_script,
            commands::scripts_info,
            commands::list_capabilities,
            commands::start_script,
            commands::open_scripts_window,
            commands::open_main_window,
            commands::get_script_console,
            commands::clear_script_console,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}

#[cfg(test)]
mod dock_icon_tests {
    use super::window_wants_dock_icon;

    #[test]
    fn only_scripting_window_wants_a_dock_icon() {
        assert!(window_wants_dock_icon("scripting"));
        for label in ["main", "progress", "standby"] {
            assert!(
                !window_wants_dock_icon(label),
                "{label} 不该触发 Dock 图标"
            );
        }
    }
}
