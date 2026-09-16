//! 后台 Worker 管理:从原 main.rs 的 `Worker`/`WorkerMsg`/`run_worker` 抽出,
//! 改为 Tauri 友好的异步结构。业务调用(send/receive/deploy)是同步阻塞的,
//! 通过 `tokio::task::spawn_blocking` 在独立线程执行;进度/结果通过
//! `app.emit` 推送到前端。

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::{deploy, notify, receive, send, typer};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

/// 进度事件节流间隔(毫秒)。Send/Deploy 的逐字符回调频率高,需要节流避免 emit 风暴。
const PROGRESS_THROTTLE_MS: u64 = 50;

/// 任务种类(与原 main.rs 的 TaskKind 等价)。
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Send,
    Recv,
    DeployType,
}

impl TaskKind {
    #[allow(dead_code)]
    fn label(self) -> &'static str {
        match self {
            TaskKind::Send => "发送到远程",
            TaskKind::Recv => "截屏接收",
            TaskKind::DeployType => "部署接收页",
        }
    }
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
}

impl WorkerState {
    pub fn new(cfg: Config) -> Self {
        Self {
            config: Arc::new(RwLock::new(cfg)),
            current: Arc::new(Mutex::new(None)),
            progress: Arc::new(RwLock::new((0, 0))),
            started_at: Arc::new(RwLock::new(0)),
        }
    }

    /// 启动一个后台任务;已有任务在跑时返回错误。
    pub async fn start(&self, kind: TaskKind, app: AppHandle) -> Result<(), String> {
        let mut cur = self.current.lock().await;
        if cur.is_some() {
            return Err("已有任务在运行".into());
        }
        let token = CancellationToken::new();
        let cfg = self.config.read().unwrap().clone();
        let app_clone = app.clone();
        let progress = self.progress.clone();
        let current = self.current.clone();
        let started_at_clone = self.started_at.clone();
        let tok = token.clone();

        // 记录任务起始时间(epoch 毫秒)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        *self.started_at.write().unwrap() = now_ms;
        *self.progress.write().unwrap() = (0, 0);

        // 显示进度窗口
        if let Some(w) = app.get_webview_window("progress") {
            let _ = w.show();
            let _ = w.set_focus();
        }

        let handle = tokio::task::spawn_blocking(move || {
            let app_for_progress = app_clone.clone();
            let last_emit: Arc<std::sync::Mutex<u64>> = Arc::new(std::sync::Mutex::new(0));
            let last_emit_clone = last_emit.clone();
            let kind_for_closure = kind;
            let started = started_at_clone.clone();
            let send_progress = move |got, total| {
                if let Ok(mut p) = progress.write() {
                    *p = (got, total);
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let started_at_val = started.read().map_or(0, |v| *v);
                // 节流:首帧、末帧必发;其余 50ms 内只更新共享状态不 emit
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
                        },
                    );
                }
            };
            let outcome = run_worker(kind, cfg, tok, &send_progress);

            // 收尾:通知前端 + 系统通知 + 清理 current
            let _ = app_clone.emit("worker-finished", &outcome);
            notify::notify(&outcome.title, &outcome.body);
            if let Ok(mut c) = current.try_lock() {
                *c = None;
            }
        });

        cur.replace(Handle {
            kind,
            token,
            abort: handle.abort_handle(),
        });
        let _ = app.emit("worker-started", kind);
        Ok(())
    }

    /// 中止当前任务(如有)。
    pub async fn cancel(&self, app: &AppHandle) {
        if let Some(h) = self.current.lock().await.take() {
            h.token.cancel();
            h.abort.abort();
            let _ = app.emit("worker-cancelled", h.kind);
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

    /// 当前任务种类(供 hotkey.rs 忙闲切换用)。
    #[allow(dead_code)]
    pub async fn current_kind(&self) -> Option<TaskKind> {
        self.current.lock().await.as_ref().map(|h| h.kind)
    }
}

/// 后台线程入口:执行任务并回传进度/结果。
/// 沿用原 main.rs L266-L327 的 match 分支,仅把进度回调改为 emit。
fn run_worker<F>(
    kind: TaskKind,
    cfg: Config,
    token: CancellationToken,
    send_progress: &F,
) -> TaskOutcome
where
    F: Fn(usize, usize),
{
    let outcome = match kind {
        TaskKind::Send => match send::run_once(&cfg, &token, true, |g, t| send_progress(g, t)) {
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
        },
        TaskKind::Recv => match receive::run_once(&cfg, &token, |g, t| send_progress(g, t)) {
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
            match deploy::type_bootstrap(&cfg, &token, true, |g, t| send_progress(g, t)) {
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
    };
    outcome
}
