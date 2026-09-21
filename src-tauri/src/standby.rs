//! 惰性待命：**只在真的要把内容打进目标窗口之前**，请用户先点一下目标窗口。
//!
//! # 为什么是惰性
//!
//! 待命窗口存在的意义是「别把键盘事件打进错误的窗口」。脚本不一定输出：
//! 只读文件、算摘要、压缩分片的脚本完全用不到键盘，却没必要被一个倒计时窗口拦住。
//!
//! 因此触发点只有一处：`$.typeStr` 的实现里调 [`StandbyGate::ensure_ready`]，
//! 在**首次**输出前弹出待命窗口并阻塞等待。`worker.rs` 不做任何提前弹窗 ——
//! 脚本立刻开始执行，只有真的要敲键盘时才打断用户。
//!
//! 门闩**一旦放行就保持放行**：同一次脚本运行里后续的 `typeStr` 不再重复提示。

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use clipbeam_scripting::HostError;
use tauri::AppHandle;

use crate::cancel::CancellationToken as WorkerCancel;

/// 门闩放行前的等待粒度：取消（Esc）最多阻塞这么久。
const WAIT_SLICE: Duration = Duration::from_millis(100);

/// 等待待命窗口放行的总上限。与 `worker::STANDBY_TIMEOUT_SECS` 对齐（前端倒计时也是 10 秒），
/// 这里再留一点余量，避免边界上比前端先超时。
const READY_TIMEOUT: Duration = Duration::from_secs(12);

/// 待命门闩：跨线程协调「用户已经点好目标窗口」。
///
/// 一侧是 Worker 的脚本线程（`$.typeStr` 阻塞等待），另一侧是 Tauri 窗口事件
/// （待命窗口失焦 = 用户点了目标窗口）。用 `(Mutex, Condvar)` 而不是异步通道：
/// 脚本侧是阻塞调用，没有异步运行时可用。
#[derive(Default)]
pub struct StandbyGate {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl StandbyGate {
    /// 新建一个未放行的门闩。
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已经放行（Worker 侧放行后为真）。
    pub fn is_ready(&self) -> bool {
        *self.inner.0.lock().unwrap()
    }

    /// 放行：可以开始输出了。幂等，且永不重新上锁。
    pub fn mark_ready(&self) {
        let (lock, signal) = &*self.inner;
        *lock.lock().unwrap() = true;
        signal.notify_all();
    }

    /// 等待放行，**不负责弹出待命窗口**（弹窗由 [`arm_standby`] 负责）。
    ///
    /// 返回 `Ok(())` 表示可以输出；`Err(HostError::Cancelled)` 表示用户在等待期间中止了任务。
    /// 已放行时立即返回（这就是「后续 `typeStr` 不再提示」的实现）。
    pub fn wait_ready(&self, cancel: &WorkerCancel) -> Result<(), HostError> {
        let (lock, signal) = &*self.inner;
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut ready = lock.lock().unwrap();

        while !*ready {
            if cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // 待命窗口没等到失焦就消失了（例如窗口被关掉）：按超时处理，
                // 与 worker 侧「超时即取消」的语义保持一致。
                return Err(HostError::Cancelled);
            }
            let (next, _timeout) = signal
                .wait_timeout(ready, left.min(WAIT_SLICE))
                .expect("待命门闩的锁不会中毒");
            ready = next;
        }
        Ok(())
    }

    /// `$.typeStr` 用：确保门闩已放行（需要时先弹出待命窗口再等）。
    ///
    /// 这是待命窗口**唯一**的触发点：只有脚本真的要注入键盘事件时才会打扰用户。
    /// `on_cancel` 见 [`arm_standby`]。
    pub fn ensure_ready(
        self: &Arc<Self>,
        app: &AppHandle,
        cancel: &WorkerCancel,
        on_cancel: crate::scripting::CancelHook,
    ) -> Result<(), HostError> {
        if self.is_ready() {
            return Ok(());
        }
        arm_standby(app, self, cancel, on_cancel);
        self.wait_ready(cancel)
    }
}

/// 弹出待命窗口并等待「用户点了目标窗口」或超时。
///
/// 窗口已显示时不会重复弹（幂等），因此 `worker.rs` 的提前弹窗与
/// [`StandbyGate::ensure_ready`] 的兜底弹窗可以安全地同时存在。
///
/// 超时时**不在这里**做任务级收尾：调用方通过 `on_cancel` 自己决定（脚本路径的收尾是
/// 「取消令牌 + 提示用户」，随后由正常的任务结束流程统一处理热键/tray/current）。
pub fn arm_standby(
    app: &AppHandle,
    gate: &Arc<StandbyGate>,
    cancel: &WorkerCancel,
    on_cancel: crate::scripting::CancelHook,
) {
    use tauri::{Emitter, Manager, WindowEvent};

    if gate.is_ready() {
        return;
    }

    let kind = crate::worker::TaskKind::Script;
    let (blur_tx, blur_rx) = tokio::sync::oneshot::channel::<()>();
    let blur_tx = Arc::new(std::sync::Mutex::new(Some(blur_tx)));

    if let Some(window) = app.get_webview_window("standby") {
        if !window.is_visible().unwrap_or(false) {
            let _ = window.show();
            let _ = window.set_focus();

            let hotkey = app
                .state::<crate::worker::WorkerState>()
                .config
                .read()
                .map(|cfg| cfg.stop_hotkey.clone())
                .unwrap_or_else(|_| "Esc".to_string());
            // 只发给待命窗口本身：`emit` 会广播给所有窗口，而各窗口跑的是同一套 Vue 应用，
            // 广播可能让别的窗口也收到本不属于它的事件（见 lib.rs 里 tray-menu 的同类修复）。
            let _ = app.emit_to(
                "standby",
                "standby-config",
                serde_json::json!({ "kind": kind, "hotkey": hotkey }),
            );

            let blur_tx_clone = blur_tx.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::Focused(false) = event {
                    if let Some(tx) = blur_tx_clone.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                }
            });
        }
    }

    let app = app.clone();
    let gate = gate.clone();
    let cancel = cancel.clone();
    tokio::spawn(async move {
        let timeout = tokio::time::sleep(Duration::from_secs(crate::worker::STANDBY_TIMEOUT_SECS));
        tokio::pin!(timeout);

        tokio::select! {
            _ = blur_rx => {
                // 用户点了目标窗口：收起提示、放行
                if let Some(window) = app.get_webview_window("standby") {
                    let _ = window.hide();
                }
                let _ = app.emit("standby-start", ());
                gate.mark_ready();
            }
            _ = &mut timeout => {
                if let Some(window) = app.get_webview_window("standby") {
                    let _ = window.hide();
                }
                let _ = app.emit("standby-cancel", ());
                // 先让脚本侧的中止检查与阻塞中的 typeStr 退出，再做调用方的收尾
                cancel.cancel();
                on_cancel();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_returns_immediately_after_mark_ready() {
        let gate = StandbyGate::new();
        assert!(!gate.is_ready());
        gate.mark_ready();
        assert!(gate.is_ready());

        let cancel = WorkerCancel::new();
        let started = Instant::now();
        gate.wait_ready(&cancel).expect("已放行应当直接返回");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "已放行时不应等待：{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn wait_unblocks_when_marked_from_another_thread() {
        let gate = Arc::new(StandbyGate::new());
        let cancel = WorkerCancel::new();

        let gate_clone = gate.clone();
        let waiter = std::thread::spawn(move || {
            let started = Instant::now();
            gate_clone.wait_ready(&cancel).expect("放行后应当成功");
            started.elapsed()
        });

        std::thread::sleep(Duration::from_millis(120));
        gate.mark_ready();

        let waited = waiter.join().expect("等待线程不应 panic");
        assert!(
            waited >= Duration::from_millis(100),
            "应当真的等过一段时间：{waited:?}"
        );
        assert!(
            waited < Duration::from_secs(2),
            "放行后应当及时返回：{waited:?}"
        );
    }

    #[test]
    fn cancel_interrupts_wait() {
        let gate = Arc::new(StandbyGate::new());
        let cancel = WorkerCancel::new();

        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            cancel_clone.cancel();
        });

        let started = Instant::now();
        let result = gate.wait_ready(&cancel);
        assert!(
            matches!(result, Err(HostError::Cancelled)),
            "取消后应当返回 Cancelled：{result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "取消应当很快生效：{:?}",
            started.elapsed()
        );
    }
}
