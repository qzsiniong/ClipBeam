// ClipBeam：宿主机 ↔ 远程浏览器剪贴板桥（键盘通道 + 二维码光通道）。
// 常驻系统托盘程序；send-once / recv-once / deploy-* 为联调用的一次性子命令。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cancel;
mod config;
mod deploy;
mod keymap;
mod notify;
mod protocol;
mod receive;
mod send;
mod settings;
mod tray;
mod typer;

use std::thread::JoinHandle;

use cancel::CancellationToken;
use clap::{Parser, Subcommand};
use config::Config;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use settings::SettingsOutcome;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;

const ID_SEND: u32 = 1;
const ID_RECV: u32 = 2;
const ID_STOP: u32 = 3;

#[derive(Parser)]
#[command(name = "clipbeam", about = "宿主机 ↔ 远程浏览器剪贴板桥")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 立即执行一次"发送本机剪贴板到远程"（联调用；请先把焦点切到远程接收页）
    SendOnce,
    /// 立即执行一次"截屏接收远程二维码"（联调用）
    RecvOnce,
    /// 生成自解压接收页并逐键打进当前焦点窗口（请先切到远程记事本）
    DeployType,
    /// 生成自解压接收页并复制到宿主机剪贴板（远程桌面支持剪贴板同步时用）
    DeployCopy,
}

// ---------------------------------------------------------------------------
// 托盘常驻模式
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Send,
    Recv,
    DeployType,
}

impl TaskKind {
    fn label(&self) -> &'static str {
        match self {
            TaskKind::Send => "发送到远程",
            TaskKind::Recv => "截屏接收",
            TaskKind::DeployType => "部署接收页",
        }
    }
}

/// 后台任务的最终结果（已转成可展示文案）。
struct TaskOutcome {
    title: String,
    body: String,
}

enum UserEvent {
    /// 托盘菜单点击（由菜单泵线程从全局通道取出后转发）。
    Menu(tray_icon::menu::MenuEvent),
    /// 全局热键事件（由热键泵线程转发）。
    Hotkey(GlobalHotKeyEvent),
    /// 后台工作线程消息。
    Worker(WorkerMsg),
}

enum WorkerMsg {
    Progress { got: usize, total: usize },
    Finished(TaskOutcome),
}

struct Worker {
    kind: TaskKind,
    token: CancellationToken,
    _handle: JoinHandle<()>,
}

struct App {
    cfg: Config,
    hk: GlobalHotKeyManager,
    tray: tray::TrayMenu,
    worker: Option<Worker>,
    settings: Option<settings::SettingsWindow>,
    /// 当前实际注册到系统的热键（重注册前先注销）。
    registered: Vec<HotKey>,
}

impl App {
    fn new(cfg: Config) -> Self {
        let hk = GlobalHotKeyManager::new().expect("无法初始化全局热键");
        let tray = tray::build().expect("无法创建系统托盘");
        let mut app = Self {
            cfg,
            hk,
            tray,
            worker: None,
            settings: None,
            registered: Vec::new(),
        };
        app.apply_hotkeys();
        app.tray.set_busy(false, "状态：空闲");
        app
    }

    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    /// 按忙闲状态重注册热键：
    /// - 空闲：发送键 + 接收键
    /// - 忙：中止键 + 当前任务的热键（再按一次即可中止）
    fn apply_hotkeys(&mut self) {
        for h in &self.registered {
            let _ = self.hk.unregister(*h);
        }
        self.registered.clear();
        let specs: Vec<(u32, &str)> = if let Some(w) = &self.worker {
            let mut specs = vec![(ID_STOP, self.cfg.stop_hotkey.as_str())];
            match w.kind {
                TaskKind::Send => specs.push((ID_SEND, self.cfg.send_hotkey.as_str())),
                TaskKind::Recv => specs.push((ID_RECV, self.cfg.recv_hotkey.as_str())),
                TaskKind::DeployType => {}
            }
            specs
        } else {
            vec![
                (ID_SEND, self.cfg.send_hotkey.as_str()),
                (ID_RECV, self.cfg.recv_hotkey.as_str()),
            ]
        };
        for (id, spec) in specs {
            if let Ok(mut h) = spec.parse::<HotKey>() {
                h.id = id;
                if self.hk.register(h).is_ok() {
                    self.registered.push(h);
                } else {
                    notify::notify("ClipBeam 热键注册失败", spec);
                }
            }
        }
    }

    fn start_task(&mut self, kind: TaskKind, proxy: &winit::event_loop::EventLoopProxy<UserEvent>) {
        if self.busy() {
            return;
        }
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let cfg = self.cfg.clone();
        let proxy = proxy.clone();
        let handle = std::thread::spawn(move || run_worker(kind, cfg, worker_token, proxy));
        self.worker = Some(Worker {
            kind,
            token,
            _handle: handle,
        });
        self.tray.set_busy(
            true,
            &format!("状态：{}中…（再次触发热键或按中止键停止）", kind.label()),
        );
        self.apply_hotkeys();
    }

    fn cancel_worker(&mut self) {
        if let Some(w) = &self.worker {
            w.token.cancel();
            self.tray
                .set_busy(true, &format!("状态：正在中止{}…", w.kind.label()));
        }
    }

    fn on_worker_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Progress { got, total } => {
                if let Some(w) = &self.worker {
                    self.tray.set_busy(
                        true,
                        &format!("状态：{}中… {got}/{total} 帧", w.kind.label()),
                    );
                }
            }
            WorkerMsg::Finished(out) => {
                self.worker = None;
                self.tray.set_busy(false, "状态：空闲");
                self.apply_hotkeys();
                notify::notify(&out.title, &out.body);
            }
        }
    }

    /// 热键事件（仅 Pressed）。
    fn on_hotkey(&mut self, id: u32, proxy: &winit::event_loop::EventLoopProxy<UserEvent>) {
        match (id, self.worker.as_ref().map(|w| w.kind)) {
            (ID_STOP, Some(_)) => self.cancel_worker(),
            (ID_SEND, Some(TaskKind::Send)) => self.cancel_worker(),
            (ID_RECV, Some(TaskKind::Recv)) => self.cancel_worker(),
            (ID_SEND, None) => self.start_task(TaskKind::Send, proxy),
            (ID_RECV, None) => self.start_task(TaskKind::Recv, proxy),
            _ => {}
        }
    }

    /// 托盘菜单点击。
    fn on_menu(
        &mut self,
        id: &str,
        proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
        elwt: &winit::event_loop::ActiveEventLoop,
    ) {
        match id {
            tray::M_SEND => self.start_task(TaskKind::Send, proxy),
            tray::M_RECV => self.start_task(TaskKind::Recv, proxy),
            tray::M_DEPLOY_TYPE => self.start_task(TaskKind::DeployType, proxy),
            tray::M_DEPLOY_COPY => match deploy::copy_bootstrap() {
                Ok(()) => {
                    let (chars, _) = deploy::sizes();
                    notify::notify(
                        "ClipBeam",
                        &format!("自解压接收页（{chars} 字符）已复制到宿主机剪贴板"),
                    );
                }
                Err(e) => notify::notify("ClipBeam 部署失败", &e),
            },
            tray::M_SETTINGS => {
                if self.settings.is_none() {
                    match settings::SettingsWindow::open(elwt, &self.cfg) {
                        Ok(w) => {
                            w.request_redraw();
                            self.settings = Some(w);
                        }
                        Err(e) => notify::notify("ClipBeam 无法打开设置", &e),
                    }
                } else if let Some(w) = &self.settings {
                    w.focus();
                }
            }
            tray::M_QUIT => elwt.exit(),
            _ => {}
        }
    }
}

/// 后台线程入口：执行任务并回传进度/结果。
fn run_worker(
    kind: TaskKind,
    cfg: Config,
    token: CancellationToken,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    let send_progress = |got, total| {
        let _ = proxy.send_event(UserEvent::Worker(WorkerMsg::Progress { got, total }));
    };
    let outcome = match kind {
        TaskKind::Send => match send::run_once(&cfg, &token, true) {
            send::SendReport::Done {
                frame_chars,
                text_bytes,
            } => TaskOutcome {
                title: "✓ 已发送到远程".into(),
                body: format!("{text_bytes} 字节，共敲入 {frame_chars} 个字符"),
            },
            send::SendReport::Cancelled { sent } => TaskOutcome {
                title: "发送已中止".into(),
                body: format!("约 {sent} 个字符可能已落入当前焦点窗口，请人工检查"),
            },
            send::SendReport::Error(e) => TaskOutcome {
                title: "发送失败".into(),
                body: e,
            },
        },
        TaskKind::Recv => match receive::run_once(&cfg, &token, send_progress) {
            receive::RecvReport::Done { frames, text_bytes } => TaskOutcome {
                title: "✓ 远程剪贴板已接收".into(),
                body: format!("{frames} 帧，{text_bytes} 字节已写入本机剪贴板"),
            },
            receive::RecvReport::Cancelled { got } => TaskOutcome {
                title: "接收已中止".into(),
                body: format!("已收集 {got} 帧"),
            },
            receive::RecvReport::Timeout { got } => TaskOutcome {
                title: "接收超时".into(),
                body: format!("超时未收齐（已收集 {got} 帧）"),
            },
            receive::RecvReport::Error(e) => TaskOutcome {
                title: "接收失败".into(),
                body: e,
            },
        },
        TaskKind::DeployType => match deploy::type_bootstrap(&cfg, &token, true) {
            typer::TypeResult::Completed(n) => TaskOutcome {
                title: "✓ 接收页引导包已输入".into(),
                body: format!("{n} 字符；请把记事本内容另存为 clipbeam.html 后打开"),
            },
            typer::TypeResult::Cancelled(n) => TaskOutcome {
                title: "部署已中止".into(),
                body: format!("约 {n} 字符可能已落入记事本，请清空后重试"),
            },
            typer::TypeResult::Failed(n, e) => TaskOutcome {
                title: "部署失败".into(),
                body: format!("{e}（已敲入约 {n} 字符，请清空记事本后重试）"),
            },
        },
    };
    let _ = proxy.send_event(UserEvent::Worker(WorkerMsg::Finished(outcome)));
}

/// winit ApplicationHandler 状态：托盘应用 + 当前修饰键状态。
struct TrayState {
    app: App,
    proxy: EventLoopProxy<UserEvent>,
    modifiers: ModifiersState,
}

impl TrayState {
    /// 把窗口事件路由给设置窗口（非设置窗口的事件直接忽略）。
    fn route_settings(&mut self, window_id: winit::window::WindowId, event: &WindowEvent) {
        let for_settings = self
            .app
            .settings
            .as_ref()
            .is_some_and(|w| w.window_id() == window_id);
        if !for_settings {
            return;
        }
        if let WindowEvent::ModifiersChanged(m) = event {
            self.modifiers = m.state();
        }
        let repaint = self
            .app
            .settings
            .as_mut()
            .is_some_and(|w| w.handle_event(event, self.modifiers));
        if repaint {
            if let Some(w) = &self.app.settings {
                w.request_redraw();
            }
        }
        // 设置窗口产生的结果（保存/关闭）
        if let Some(outcome) = self.app.settings.as_mut().and_then(|w| w.take_outcome()) {
            if let SettingsOutcome::Saved(new_cfg) = outcome {
                if let Err(e) = new_cfg.save() {
                    notify::notify("ClipBeam 配置保存失败", &e.to_string());
                }
                self.app.cfg = new_cfg;
                self.app.apply_hotkeys();
                notify::notify("ClipBeam", "设置已保存，热键已重注册");
            }
            // drop SettingsWindow（关闭 GL 表面与窗口）
            self.app.settings = None;
        }
    }
}

impl ApplicationHandler<UserEvent> for TrayState {
    fn resumed(&mut self, _elwt: &ActiveEventLoop) {}

    fn user_event(&mut self, elwt: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(ev) => self.app.on_menu(ev.id.0.as_str(), &self.proxy, elwt),
            UserEvent::Hotkey(ev) => {
                if ev.state == HotKeyState::Pressed {
                    self.app.on_hotkey(ev.id, &self.proxy);
                }
            }
            UserEvent::Worker(msg) => self.app.on_worker_msg(msg),
        }
    }

    fn window_event(
        &mut self,
        _elwt: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        self.route_settings(window_id, &event);
    }
}

fn run_tray() {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // macOS：Accessory 策略 = 纯后台程序，不显示 Dock 图标（设置窗口仍可弹出）。
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().expect("无法创建事件循环");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();

    // 菜单/热键泵线程：crossbeam 通道阻塞等待，取到事件后转发给 winit。
    spawn_pump("menu", proxy.clone(), |p| {
        let r = tray_icon::menu::MenuEvent::receiver();
        move || {
            if let Ok(ev) = r.recv() {
                let _ = p.send_event(UserEvent::Menu(ev));
            }
        }
    });
    spawn_pump("hotkey", proxy.clone(), |p| {
        let r = GlobalHotKeyEvent::receiver();
        move || {
            if let Ok(ev) = r.recv() {
                let _ = p.send_event(UserEvent::Hotkey(ev));
            }
        }
    });

    let mut state = TrayState {
        app: App::new(Config::load()),
        proxy,
        modifiers: ModifiersState::empty(),
    };
    event_loop.run_app(&mut state).expect("事件循环异常");
}

/// 起一个常驻泵线程；`make` 给出每轮执行体。
fn spawn_pump<F>(
    name: &'static str,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    make: impl FnOnce(winit::event_loop::EventLoopProxy<UserEvent>) -> F + Send + 'static,
) where
    F: FnMut() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut f = make(proxy);
            loop {
                f();
            }
        })
        .expect("无法启动泵线程");
}

// ---------------------------------------------------------------------------
// CLI 入口
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::SendOnce) => {
            let cfg = Config::load();
            let token = CancellationToken::new();
            eprintln!("请确认焦点已在远程接收页，0.2 秒后开始发送（Ctrl-C 可强杀进程）……");
            match send::run_once(&cfg, &token, true) {
                send::SendReport::Done {
                    frame_chars,
                    text_bytes,
                } => {
                    eprintln!("✓ 发送完成：原始 {text_bytes} 字节，共敲入 {frame_chars} 个字符");
                }
                send::SendReport::Cancelled { sent } => {
                    eprintln!("✗ 已中止：约 {sent} 个字符可能已落入当前焦点窗口，请人工检查")
                }
                send::SendReport::Error(e) => eprintln!("✗ 发送失败：{e}"),
            }
        }
        Some(Command::RecvOnce) => {
            let cfg = Config::load();
            let token = CancellationToken::new();
            eprintln!(
                "开始截屏接收（超时 {}s，Ctrl-C 强杀进程）……请在远程页面循环播放二维码",
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
                    eprintln!("✓ 接收完成：{frames} 帧，{text_bytes} 字节已写入本机剪贴板")
                }
                receive::RecvReport::Cancelled { got } => {
                    eprintln!("✗ 已中止（已收集 {got} 帧）")
                }
                receive::RecvReport::Timeout { got } => {
                    eprintln!("✗ 超时未收齐（已收集 {got} 帧）；请确认二维码正在播放后重试")
                }
                receive::RecvReport::Error(e) => eprintln!("✗ 接收失败：{e}"),
            }
        }
        Some(Command::DeployType) => {
            let cfg = Config::load();
            let token = CancellationToken::new();
            let (chars, page_bytes) = deploy::sizes();
            let secs = chars as f64 * cfg.key_delay_ms as f64 / 1000.0;
            eprintln!(
                "请在远程打开记事本并聚焦，{:.1} 秒后开始敲入自解压接收页（共 {chars} 字符，约 {secs:.0} 秒，内嵌页面 {page_bytes} 字节）。Ctrl-C 可强杀。",
                cfg.settle_ms as f64 / 1000.0
            );
            match deploy::type_bootstrap(&cfg, &token, true) {
                typer::TypeResult::Completed(n) => {
                    eprintln!("✓ 引导包输入完成（{n} 字符）。请在远程把记事本内容另存为 clipbeam.html（编码 UTF-8），双击打开即可。")
                }
                typer::TypeResult::Cancelled(n) => {
                    eprintln!("✗ 已中止：约 {n} 字符可能已落入记事本，请清空后重试")
                }
                typer::TypeResult::Failed(_, e) => eprintln!("✗ 部署失败：{e}"),
            }
        }
        Some(Command::DeployCopy) => {
            let (chars, page_bytes) = deploy::sizes();
            match deploy::copy_bootstrap() {
                Ok(()) => eprintln!(
                    "✓ 自解压接收页（{chars} 字符，内嵌 {page_bytes} 字节）已复制到宿主机剪贴板，请在远程粘贴、另存为 .html 后打开"
                ),
                Err(e) => eprintln!("✗ 复制失败：{e}"),
            }
        }
        None => run_tray(),
    }
}
