//! ClipBeam 库:Tauri 2 应用初始化 + CLI 子命令分流。
//!
//! 核心业务模块(send/receive/deploy/protocol/config/cancel/typer/keymap/notify)
//! 与 GUI 解耦,本文件只做编排:把它们通过 `#[tauri::command]` 桥接给前端,
//! 通过 Tauri 事件系统推送 worker 进度,通过 Tauri tray/global-shortcut API
//! 替代原 winit + tray-icon + global-hotkey 三件套。

mod cancel;
mod commands;
mod config;
mod deploy;
mod hotkey;
mod keymap;
mod notify;
mod protocol;
mod receive;
mod send;
mod tray;
mod typer;
mod worker;

use clap::{Parser, Subcommand};
use config::Config;
use tauri::{Emitter, Manager, async_runtime::spawn};

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
}

/// CLI 入口:执行单次子命令(无 Tauri 事件循环)。
pub fn run_cli(cmd: Command) {
    let cfg = Config::load();
    let token = cancel::CancellationToken::new();
    match cmd {
        Command::SendOnce => {
            eprintln!("请确认焦点已在远程接收页,0.2 秒后开始发送(Ctrl-C 可强杀进程)……");
            match send::run_once(&cfg, &token, true, |_, _| {}) {
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
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // macOS: 托盘常驻模式,不占 Dock(Tauri 默认是 Regular 策略)
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
            let cfg = Config::load();
            tray::build(app, &cfg)?;
            // 启动时为空闲模式:仅注册发送/接收热键,Esc 不拦截
            hotkey::set_mode(app.handle(), &cfg, hotkey::HotkeyMode::Idle)?;
            app.manage(worker::WorkerState::new(cfg));
            Ok(())
        })
        .on_tray_icon_event(|app, event| tray::on_tray_event(app, event))
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                tray::M_QUIT => app.exit(0),
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
                            Err(e) => {
                                crate::notify::notify("ClipBeam", &format!("复制失败: {e}"))
                            }
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
                _ => {
                    let _ = app.emit("tray-menu", id);
                },
            }
        })
        .on_window_event(|window, event| {
            // 关闭按钮 → 隐藏到托盘(不退出)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::start_send,
            commands::start_recv,
            commands::start_deploy_type,
            commands::deploy_copy,
            commands::cancel_task,
            commands::get_task_status,
            commands::capture_hotkey,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
