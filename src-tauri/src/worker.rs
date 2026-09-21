//! 后台 Worker 管理:从原 main.rs 的 `Worker`/`WorkerMsg`/`run_worker` 抽出,
//! 改为 Tauri 友好的异步结构。业务调用(send/receive/deploy)是同步阻塞的,
//! 通过 `tokio::task::spawn_blocking` 在独立线程执行;进度/结果通过
//! `app.emit` 推送到前端。
//!
//! 启动流程为两阶段:待命阶段显示 standby 窗口引导用户点击目标窗口,
//! blur(失焦)触发后才真正开始执行,避免键盘事件注入到错误窗口。

use crate::cancel::CancellationToken;
use crate::config::{Config, ProgressDisplay};
use crate::{deploy, notify, receive, send, tray, typer};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

/// 进度事件节流间隔(毫秒)。1s 更新一次,避免数字跳动;首帧/末帧强制 emit。
const PROGRESS_THROTTLE_MS: u64 = 1000;

/// 待命窗口超时(秒)。`standby::arm` 用它做超时(两条待命路径共用)。
pub const STANDBY_TIMEOUT_SECS: u64 = 10;

/// 任务种类(与原 main.rs 的 TaskKind 等价)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    SendRaw,
    Send,
    Recv,
    DeployType,
    /// 运行用户脚本(JS/TS)。输出走 Typer,确认走前端弹窗。
    Script,
}

/// 待命窗口上显示的热键文案：按任务类型取对应的键。
///
/// `Recv` 不经过待命窗口（截屏不注入键盘事件），取中止键即可 —— 这里不再 `unreachable!()`。
/// 脚本的惰性待命（`standby::ensure_ready`）也调它，两条路径共用一份文案逻辑。
pub(crate) fn standby_hotkey(cfg: &Config, kind: TaskKind) -> String {
    match kind {
        TaskKind::SendRaw => cfg.send_raw_hotkey.clone(),
        TaskKind::Send => cfg.send_hotkey.clone(),
        TaskKind::DeployType | TaskKind::Script | TaskKind::Recv => cfg.stop_hotkey.clone(),
    }
}

/// 一次脚本任务的请求:脚本名 + 已转译的源码。
///
/// 由 `commands::start_script` 准备好后放进 `WorkerState::pending_script`,
/// `WorkerState::start` 取出执行。之所以做成「先放请求再 start」:
/// `start(kind, app)` 的签名保持不变,不牵连既有的四个任务。
#[derive(Clone)]
pub struct ScriptRequest {
    /// 出现在错误信息里的脚本名(文件名)。
    pub name: String,
    /// 已经过 TS 转译的源码。
    pub source: String,
}

/// 任务最终结果(已转成可展示文案)。
#[derive(Clone, serde::Serialize)]
pub struct TaskOutcome {
    pub title: String,
    pub body: String,
}

/// 进度推送负载。
#[derive(Clone, serde::Serialize)]
pub struct Progress {
    pub kind: TaskKind,
    pub got: usize,
    pub total: usize,
    pub started_at: u64,
    pub ts: u64,
    /// 脚本任务当前输出的片段(其它任务为空)。加了 serde 默认值,
    /// 老前端收到多出来的字段不会报错。
    #[serde(default)]
    pub snippet: String,
}

/// 当前 Worker 状态快照。
#[derive(Clone, serde::Serialize)]
pub struct Status {
    pub busy: bool,
    pub kind: Option<TaskKind>,
    pub got: usize,
    pub total: usize,
}

struct Handle {
    kind: TaskKind,
    token: CancellationToken,
    abort: tokio::task::AbortHandle,
}

/// 全局 Worker 状态:持有当前 Config 和正在运行的任务句柄。
pub struct WorkerState {
    pub config: Arc<RwLock<Config>>,
    current: Arc<Mutex<Option<Handle>>>,
    progress: Arc<RwLock<(usize, usize)>>,
    started_at: Arc<RwLock<u64>>,
    /// 待执行的脚本任务(见 [`ScriptRequest`])。
    pending_script: Arc<std::sync::Mutex<Option<ScriptRequest>>>,
    /// 脚本 Console 面板的输出缓冲(见 [`crate::console_panel`])。
    pub console: Arc<crate::console_panel::ConsoleBuffer>,
    /// 待命窗口的「暂停/恢复失焦检测」开关（前端按钮 → 命令 → 这里 → 待命驱动）。
    pub standby_ctl: Arc<crate::standby::StandbyControl>,
}

impl WorkerState {
    pub fn new(cfg: Config, app: tauri::AppHandle) -> Self {
        Self {
            config: Arc::new(RwLock::new(cfg)),
            current: Arc::new(Mutex::new(None)),
            progress: Arc::new(RwLock::new((0, 0))),
            started_at: Arc::new(RwLock::new(0)),
            pending_script: Arc::new(std::sync::Mutex::new(None)),
            console: crate::console_panel::ConsoleBuffer::new(app),
            standby_ctl: Arc::new(crate::standby::StandbyControl::new()),
        }
    }

    /// 启动一个后台任务;已有任务在跑时返回错误。
    /// 两阶段:先显示待命窗口,blur 触发后执行。
    pub async fn start(&self, kind: TaskKind, app: AppHandle) -> Result<(), String> {
        let mut cur = self.current.lock().await;
        if cur.is_some() {
            return Err("已有任务在运行".into());
        }
        let token = CancellationToken::new();
        let cfg = self.config.read().unwrap().clone();
        let cfg_for_busy = cfg.clone();
        let cfg_for_task = cfg.clone();
        let progress = self.progress.clone();
        let current = self.current.clone();
        let config_store = self.config.clone();
        let started_at_clone = self.started_at.clone();
        let script_request = self.pending_script.lock().unwrap().take();
        let tok = token.clone();

        // 记录任务起始时间(epoch 毫秒)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        *self.started_at.write().unwrap() = now_ms;
        *self.progress.write().unwrap() = (0, 0);

        // 占位 Handle,切忙时热键
        cur.replace(Handle {
            kind,
            token,
            abort: dummy_abort_handle(),
        });
        drop(cur); // 释放锁,允许 cancel 取 take

        let _ = app.emit("worker-started", kind);
        // 切换为忙时热键:Esc + 当前任务热键
        if let Err(e) =
            crate::hotkey::set_mode(&app, &cfg_for_busy, crate::hotkey::HotkeyMode::Busy(kind))
        {
            log::error!("切换忙时热键失败: {e}");
            notify::notify("ClipBeam", "热键切换失败,任务仍在运行");
        }

        // 不需要「启动前待命」的任务直接执行:
        // - Recv:截屏不注入键盘事件到其他窗口,焦点安全;
        // - Script:脚本不一定输出,启动前弹待命会打扰纯计算脚本。它的待命是**惰性**的,
        //   只在首次 `$.type_str` 之前由 StandbyGate 触发(见 scripting.rs 的 type_str)。
        if matches!(kind, TaskKind::Recv | TaskKind::Script) {
            Self::execute_task(
                app,
                kind,
                cfg_for_task,
                tok,
                progress,
                current,
                config_store,
                started_at_clone,
                script_request,
                None,
            );
            return Ok(());
        }

        // Send/SendRaw/DeployType 走待命阶段：这些任务一定会敲键盘，所以在执行前先把焦点
        // 交给用户 —— 待命窗口自己拿焦点，它失焦即表示用户已经把焦点移到了目标窗口。
        // 窗口/事件/超时/暂停的编排都在 `StandbyGate` 里，任务级收尾留在下面。
        let on_standby_cancel: crate::scripting::CancelHook = {
            let app = app.clone();
            let current = current.clone();
            let config_store = config_store.clone();
            Arc::new(move || {
                crate::notify::notify("ClipBeam", "未检测到点击,任务已取消");
                // 此刻任务还没开始执行，托盘也还没被设成忙：清掉当前任务、广播取消、
                // 把热键恢复成空闲即可（任务中途重新待命失败时会由正常收尾流程处理）。
                if let Ok(mut c) = current.try_lock() {
                    *c = None;
                }
                let _ = app.emit("worker-cancelled", kind);
                let cfg_fresh = config_store.read().map(|c| c.clone()).unwrap_or_default();
                if let Err(e) =
                    crate::hotkey::set_mode(&app, &cfg_fresh, crate::hotkey::HotkeyMode::Idle)
                {
                    log::error!("恢复空闲热键失败: {e}");
                }
            })
        };
        let standby = Arc::new(crate::standby::StandbyGate::new(
            app.clone(),
            tok.clone(),
            on_standby_cancel,
            kind,
            standby_hotkey(&cfg, kind),
        ));
        let outcome = standby.arm(None);

        let app_clone = app.clone();
        let script_request_for_task = script_request.clone();
        let standby_for_task = standby.clone();
        tokio::spawn(async move {
            match outcome.await {
                Ok(crate::standby::StandbyOutcome::Ready { target }) => {
                    standby_for_task.confirm(target);
                    Self::execute_task(
                        app_clone,
                        kind,
                        cfg_for_task,
                        tok,
                        progress,
                        current,
                        config_store,
                        started_at_clone,
                        script_request_for_task,
                        Some(standby_for_task),
                    );
                }
                // 超时/异常：收尾已经由 gate 的 `on_cancel` 做完了
                Ok(crate::standby::StandbyOutcome::TimedOut) | Err(_) => standby_for_task.abort(),
                // 用户在待命期间取消：`WorkerState::cancel` 已经收尾，这里不用再做什么
                Ok(crate::standby::StandbyOutcome::Cancelled) => {}
            }
        });

        Ok(())
    }

    /// 真正执行任务(spawn_blocking)。
    #[allow(clippy::too_many_arguments)]
    fn execute_task(
        app: AppHandle,
        kind: TaskKind,
        cfg: Config,
        token: CancellationToken,
        progress: Arc<RwLock<(usize, usize)>>,
        current: Arc<Mutex<Option<Handle>>>,
        config_store: Arc<RwLock<Config>>,
        started_at: Arc<RwLock<u64>>,
        script_request: Option<ScriptRequest>,
        // Send/SendRaw/DeployType 的待命门闩（脚本路径自己构造，见 `run_script_task`）
        standby: Option<Arc<crate::standby::StandbyGate>>,
    ) {
        let progress_display = cfg.progress_display;
        // 显示进度窗口(如果配置 Floating 或 Both),不抢焦点。
        // 定位在 `progress_window::show` 里先摆好再 show（默认鼠标所在显示器右上角，
        // 用户拖过就用他拖到的位置），避免窗口在旧位置闪一下。
        if matches!(
            progress_display,
            ProgressDisplay::Floating | ProgressDisplay::Both
        ) {
            crate::progress_window::show(&app);
        }
        // 启用托盘忙时状态
        let _ = tray::set_busy(&app, true, "状态:运行中");

        // 自动弹出托盘菜单
        // 暂时不使用异步弹出,因为会阻塞任务启动,导致任务启动失败
        // let _ = tray::show_menu(&app);

        tokio::task::spawn_blocking(move || {
            let app_for_progress = app.clone();
            let last_emit: Arc<std::sync::Mutex<u64>> = Arc::new(std::sync::Mutex::new(0));
            let last_emit_clone = last_emit.clone();
            let kind_for_closure = kind;
            let started = started_at.clone();
            let progress_shared = progress.clone();
            let send_progress = move |got, total| {
                if let Ok(mut p) = progress_shared.write() {
                    *p = (got, total);
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let started_at_val = started.read().map_or(0, |v| *v);
                // 节流:首帧、末帧必发;其余 1s 内只更新共享状态不 emit
                let should_emit = {
                    let mut last = last_emit_clone.lock().unwrap();
                    let elapsed = now.saturating_sub(*last);
                    if got == 1 || got == total || elapsed >= PROGRESS_THROTTLE_MS {
                        *last = now;
                        true
                    } else {
                        false
                    }
                };
                if should_emit {
                    let _ = app_for_progress.emit(
                        "worker-progress",
                        Progress {
                            kind: kind_for_closure,
                            got,
                            total,
                            started_at: started_at_val,
                            ts: now,
                            // 脚本任务的片段由 TauriScriptHost 直接 emit,这里不带
                            snippet: String::new(),
                        },
                    );
                    // 更新托盘图标 + 状态行(如果配置 Tray 或 Both)
                    if matches!(
                        progress_display,
                        ProgressDisplay::Tray | ProgressDisplay::Both
                    ) {
                        let percent = (got * 100).checked_div(total).unwrap_or(0) as u8;
                        let status_text =
                            format!("状态:{kind_for_closure:?} {percent}% · {got}/{total}");
                        let _ = tray::set_status_text(&app_for_progress, &status_text);
                        let _ = tray::set_tray_progress(&app_for_progress, percent);
                    }
                }
            };
            let app_for_task = app.clone();
            let progress_for_task = progress.clone();
            let current_for_task = current.clone();
            let outcome = run_worker(
                kind,
                cfg,
                token,
                &send_progress,
                script_request,
                &app_for_task,
                progress_for_task,
                current_for_task,
                standby,
            );

            // 组装统计信息(用时、平均速度、已发/总)
            let (got, total) = progress.read().map(|p| *p).unwrap_or((0, 0));
            let started_at_val = started_at.read().map_or(0, |v| *v);
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let elapsed_ms = now_ms.saturating_sub(started_at_val);
            let elapsed_secs = (elapsed_ms / 1000).max(1);
            let avg_speed = if elapsed_secs > 0 {
                got / elapsed_secs as usize
            } else {
                0
            };
            let unit = if kind == TaskKind::Recv {
                "帧"
            } else {
                "字符"
            };
            let outcome_with_stats = TaskOutcome {
                title: outcome.title,
                body: format!(
                    "{} · 用时 {}s · 平均 {} {}/s · {got}/{total} {}",
                    outcome.body, elapsed_secs, avg_speed, unit, unit
                ),
            };

            // 收尾:通知前端 + 系统通知 + 清理 current
            let _ = app.emit("worker-finished", &outcome_with_stats);
            notify::notify(&outcome_with_stats.title, &outcome_with_stats.body);
            if let Ok(mut c) = current.try_lock() {
                *c = None;
            }
            // 隐藏进度窗口(由前端控制)
            // if let Some(w) = app.get_webview_window("progress") {
            //     let _ = w.hide();
            // }
            // 恢复托盘 idle 状态
            let _ = tray::set_busy(&app, false, "状态:空闲");
            // 任务真正结束后恢复空闲热键(释放 Esc);读最新配置
            let cfg_fresh = config_store.read().map(|c| c.clone()).unwrap_or_default();
            if let Err(e) =
                crate::hotkey::set_mode(&app, &cfg_fresh, crate::hotkey::HotkeyMode::Idle)
            {
                log::error!("恢复空闲热键失败: {e}");
            }
        });
    }

    /// 登记待运行的脚本(见 [`ScriptRequest`])；`start` 时取走。
    pub fn set_pending_script(&self, request: ScriptRequest) {
        *self.pending_script.lock().unwrap() = Some(request);
    }

    /// 清掉待运行的脚本（启动失败时用，避免残留污染下一次任务）。
    pub fn clear_pending_script(&self) {
        *self.pending_script.lock().unwrap() = None;
    }

    /// 中止当前任务(如有)。
    pub async fn cancel(&self, app: &AppHandle) {
        if let Some(h) = self.current.lock().await.take() {
            h.token.cancel();
            h.abort.abort();
            // 隐藏待命窗口(如果在待命阶段)
            if let Some(w) = app.get_webview_window("standby") {
                let _ = w.hide();
            }
            let _ = app.emit("worker-cancelled", h.kind);
            // 恢复托盘 idle 状态
            let _ = tray::set_busy(app, false, "状态:空闲");
            // 恢复空闲热键
            let cfg = self.config.read().unwrap().clone();
            if let Err(e) = crate::hotkey::set_mode(app, &cfg, crate::hotkey::HotkeyMode::Idle) {
                log::error!("恢复空闲热键失败: {e}");
            }
        }
    }

    pub fn status(&self) -> Status {
        let cur = self.current.try_lock();
        let (got, total) = *self.progress.read().unwrap();
        match cur {
            Ok(guard) => {
                let h = guard.as_ref();
                Status {
                    busy: h.is_some(),
                    kind: h.map(|h| h.kind),
                    got,
                    total,
                }
            }
            Err(_) => Status {
                busy: true,
                kind: None,
                got,
                total,
            },
        }
    }

    /// 当前任务种类(供配置保存后选择忙/闲热键注册模式)。
    pub async fn current_kind(&self) -> Option<TaskKind> {
        self.current.lock().await.as_ref().map(|h| h.kind)
    }
}

/// 后台线程入口:执行任务并回传进度/结果。
///
/// 脚本任务(`TaskKind::Script`)在这里调 `tauri::async_runtime::block_on` 驱动引擎 ——
/// 当前函数本身运行在 `spawn_blocking` 出来的线程上,阻塞它是安全的。
#[allow(clippy::too_many_arguments)]
fn run_worker<F>(
    kind: TaskKind,
    cfg: Config,
    token: CancellationToken,
    send_progress: &F,
    script_request: Option<ScriptRequest>,
    app: &tauri::AppHandle,
    progress: Arc<RwLock<(usize, usize)>>,
    current: Arc<Mutex<Option<Handle>>>,
    standby: Option<Arc<crate::standby::StandbyGate>>,
) -> TaskOutcome
where
    F: Fn(usize, usize),
{
    let outcome = match kind {
        TaskKind::SendRaw => {
            match send::run_once(&cfg, true, &token, true, standby.clone(), send_progress) {
                send::SendReport::Done {
                    frame_chars,
                    text_bytes,
                } => TaskOutcome {
                    title: "✓ 已发送到远程".into(),
                    body: format!("{text_bytes} 字节,共敲入 {frame_chars} 个字符"),
                },
                send::SendReport::Cancelled { sent } => TaskOutcome {
                    title: "发送已中止".into(),
                    body: format!("约 {sent} 个字符可能已落入当前焦点窗口,请人工检查"),
                },
                send::SendReport::Error(e) => TaskOutcome {
                    title: "发送失败".into(),
                    body: e,
                },
            }
        }
        TaskKind::Send => {
            match send::run_once(&cfg, false, &token, true, standby.clone(), send_progress) {
                send::SendReport::Done {
                    frame_chars,
                    text_bytes,
                } => TaskOutcome {
                    title: "✓ 已发送到远程".into(),
                    body: format!("{text_bytes} 字节,共敲入 {frame_chars} 个字符"),
                },
                send::SendReport::Cancelled { sent } => TaskOutcome {
                    title: "发送已中止".into(),
                    body: format!("约 {sent} 个字符可能已落入当前焦点窗口,请人工检查"),
                },
                send::SendReport::Error(e) => TaskOutcome {
                    title: "发送失败".into(),
                    body: e,
                },
            }
        }
        TaskKind::Recv => match receive::run_once(&cfg, &token, send_progress) {
            receive::RecvReport::Done { frames, text_bytes } => TaskOutcome {
                title: "✓ 远程剪贴板已接收".into(),
                body: format!("{frames} 帧,{text_bytes} 字节已写入本机剪贴板"),
            },
            receive::RecvReport::Cancelled { got } => TaskOutcome {
                title: "接收已中止".into(),
                body: format!("已收集 {got} 帧"),
            },
            receive::RecvReport::Timeout { got } => TaskOutcome {
                title: "接收超时".into(),
                body: format!("超时未收齐(已收集 {got} 帧)"),
            },
            receive::RecvReport::Error(e) => TaskOutcome {
                title: "接收失败".into(),
                body: e,
            },
        },
        TaskKind::DeployType => {
            match deploy::type_bootstrap(&cfg, &token, true, standby.clone(), send_progress) {
                typer::TypeResult::Completed(n) => TaskOutcome {
                    title: "✓ 接收页引导包已输入".into(),
                    body: format!("{n} 字符;请把记事本内容另存为 clipbeam.html 后打开"),
                },
                typer::TypeResult::Cancelled(n) => TaskOutcome {
                    title: "部署已中止".into(),
                    body: format!("约 {n} 字符可能已落入记事本,请清空后重试"),
                },
                typer::TypeResult::Failed(n, e) => TaskOutcome {
                    title: "部署失败".into(),
                    body: format!("{e}(已敲入约 {n} 字符,请清空记事本后重试)"),
                },
            }
        }
        TaskKind::Script => run_script_task(
            app,
            &token,
            script_request,
            progress,
            current,
            send_progress,
        ),
    };
    outcome
}

/// 执行一个脚本任务:转译 → 在引擎里跑 → 把结果整理成文案。
///
/// 进度由宿主(`TauriScriptHost`)直接 emit;这里只负责首帧与结果文案。
fn run_script_task<F>(
    app: &tauri::AppHandle,
    token: &CancellationToken,
    request: Option<ScriptRequest>,
    progress: Arc<RwLock<(usize, usize)>>,
    current: Arc<Mutex<Option<Handle>>>,
    send_progress: &F,
) -> TaskOutcome
where
    F: Fn(usize, usize),
{
    let Some(request) = request else {
        return TaskOutcome {
            title: "脚本未执行".into(),
            body: "没有拿到脚本内容(内部状态异常,请重试)".into(),
        };
    };

    let started = std::time::Instant::now();

    // Console 面板只属于**本次运行**:上一轮的输出留在面板里只会干扰排查
    let console = app.state::<WorkerState>().console.clone();
    console.clear();
    console.push(
        "info",
        &format!(
            "▶ 运行 {}{}",
            request.name,
            if crate::scripting::may_type(&request.source) {
                "(含键盘输出)"
            } else {
                ""
            }
        ),
    );

    // 待命门闩:惰性放行 —— 只有真的要把内容打进目标窗口时才需要用户先点一下目标窗口
    let on_standby_cancel: crate::scripting::CancelHook = {
        let app = app.clone();
        let current = current.clone();
        Arc::new(move || {
            crate::notify::notify("ClipBeam", "未检测到点击,已停止输出");
            // 与 cancel() 一致的收尾:清掉当前任务、恢复托盘与空闲热键。
            // 脚本线程随后会因为取消令牌而退出,最终由 Worker 正常收尾。
            if let Ok(mut guard) = current.try_lock() {
                *guard = None;
            }
            let _ = tray::set_busy(&app, false, "状态:空闲");
            let cfg_fresh = app
                .state::<WorkerState>()
                .config
                .read()
                .map(|cfg| cfg.clone())
                .unwrap_or_default();
            if let Err(e) =
                crate::hotkey::set_mode(&app, &cfg_fresh, crate::hotkey::HotkeyMode::Idle)
            {
                log::error!("恢复空闲热键失败: {e}");
            }
        })
    };
    let cfg_now = app
        .state::<WorkerState>()
        .config
        .read()
        .map(|cfg| cfg.clone())
        .unwrap_or_default();
    let standby = Arc::new(crate::standby::StandbyGate::new(
        app.clone(),
        token.clone(),
        on_standby_cancel,
        TaskKind::Script,
        standby_hotkey(&cfg_now, TaskKind::Script),
    ));

    // 待命窗口不在这里弹:它只在**首次 `$.type_str` 之前**由 `StandbyGate::ensure_ready` 触发
    // (见 scripting.rs 的 type_str)。纯计算脚本因此完全不会被待命窗口打扰。
    let host = Arc::new(crate::scripting::TauriScriptHost::new(
        app.clone(),
        token.clone(),
        standby,
        progress.clone(),
        console.clone(),
    ));

    // 先报一次 0/0,让进度窗口立刻有反应
    send_progress(0, 0);

    let engine_cancel = crate::script_runner::engine_cancel(token);
    // 引擎需要 tokio 上下文(AsyncRuntime 与 sleep 都依赖它):block_on 会把当前线程
    // 带进 Tauri 的异步运行时。当前线程是 spawn_blocking 出来的,阻塞它是安全的。
    let result = tauri::async_runtime::block_on(crate::script_runner::run_source(
        &request.name,
        &request.source,
        host.clone() as Arc<dyn clipbeam_scripting::ScriptHost>,
        host.clone() as Arc<dyn script_engine::ConsoleHook>,
        engine_cancel,
    ));

    let typed = host.typed_chars();
    let (got, total) = progress.read().map(|p| *p).unwrap_or((typed, typed));
    send_progress(got.max(typed), total.max(typed));

    match result {
        Ok(()) => {
            console.push(
                "info",
                &format!(
                    "✓ 完成(输出 {typed} 个字符,用时 {}ms)",
                    started.elapsed().as_millis()
                ),
            );
            TaskOutcome {
                title: "✓ 脚本执行完成".into(),
                body: if typed > 0 {
                    format!("已敲入 {typed} 个字符")
                } else {
                    "脚本执行完成(没有键盘输出)".to_string()
                },
            }
        }
        Err(err) => {
            let text = crate::script_runner::describe_error(&err);
            if text.contains("已中止") {
                console.push("warn", &format!("脚本已中止(已输出 {typed} 个字符)"));
                TaskOutcome {
                    title: "脚本已中止".into(),
                    body: format!("已敲入 {typed} 个字符,请检查目标窗口内容是否完整"),
                }
            } else {
                // 脚本异常也进面板:面板是"这次运行到底发生了什么"的完整记录
                console.push("error", &text);
                TaskOutcome {
                    title: "脚本执行失败".into(),
                    body: format!("{text}(已敲入 {typed} 个字符)"),
                }
            }
        }
    }
}

/// 创建一个已 abort 的 AbortHandle 占位(待命阶段无实际任务)。
fn dummy_abort_handle() -> tokio::task::AbortHandle {
    tokio::task::spawn(async {}).abort_handle()
}
