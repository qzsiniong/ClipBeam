//! 后台 Worker 管理:从原 main.rs 的 `Worker`/`WorkerMsg`/`run_worker` 抽出,
//! 改为 Tauri 友好的异步结构。业务调用(send/receive/deploy)是同步阻塞的,
//! 通过 `tokio::task::spawn_blocking` 在独立线程执行;进度/结果通过
//! `app.emit` 推送到前端。

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::{deploy, notify, receive, send, typer};
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

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
    pub got: usize,
    pub total: usize,
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
}

impl WorkerState {
    pub fn new(cfg: Config) -> Self {
        Self {
            config: Arc::new(RwLock::new(cfg)),
            current: Arc::new(Mutex::new(None)),
            progress: Arc::new(RwLock::new((0, 0))),
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
        let tok = token.clone();

        let handle = tokio::task::spawn_blocking(move || {
            let app_for_progress = app_clone.clone();
            let send_progress = move |got, total| {
                if let Ok(mut p) = progress.write() {
                    *p = (got, total);
                }
                let _ = app_for_progress.emit("worker-progress", Progress { got, total });
            };
            let outcome = run_worker(kind, cfg, tok, send_progress);

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
        *self.progress.write().unwrap() = (0, 0);
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
    send_progress: F,
) -> TaskOutcome
where
    F: Fn(usize, usize) + Send + 'static,
{
    let outcome = match kind {
        TaskKind::Send => match send::run_once(&cfg, &token, true) {
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
        TaskKind::DeployType => match deploy::type_bootstrap(&cfg, &token, true) {
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
        },
    };
    outcome
}
