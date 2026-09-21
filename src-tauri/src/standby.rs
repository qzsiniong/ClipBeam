//! 待命门闩：**在把键盘事件打进目标窗口之前**，确保用户已经把焦点移到了目标窗口。
//!
//! # 为什么是惰性（脚本路径）
//!
//! 待命窗口存在的意义是「别把键盘事件打进错误的窗口」。脚本不一定输出：
//! 只读文件、算摘要、压缩分片的脚本完全用不到键盘，却没必要被一个倒计时窗口拦住。
//! 因此脚本路径的触发点只有一处：`$.type_str` 的实现里调 [`StandbyGate::ensure_ready`]，
//! 在**首次**输出前弹出待命窗口并阻塞等待 —— 纯计算脚本不会被待命窗口打扰。
//!
//! `worker.rs` 的 Send/SendRaw/DeployType 一定会敲键盘，所以在任务启动时就 arm（急切）。
//!
//! # 焦点交接（待命窗口为什么必须自己拿焦点）
//!
//! 待命的语义是「在注入键盘事件之前，让用户把焦点移到目标窗口（输入框）」。
//! 所以 [`StandbyGate::arm`] 一定会 `show()` + `set_focus()`：只有待命窗口自己持有焦点，
//! 「失焦」才等价于「用户已经把焦点交给了目标窗口」。反过来，一个可见但没有焦点的待命窗口
//! 是没意义的 —— 它永远等不到那次失焦，用户会一直卡在倒计时里。
//!
//! 两条路径（worker 的急切待命 / 脚本的惰性待命）**收尾方式不同**，所以 [`StandbyGate::arm`]
//! 只管窗口与事件；超时的任务级收尾由构造 gate 时传入的 `on_cancel` 决定。
//!
//! # 锁定与自动重锁
//!
//! 一次待命成功 = **锁定**一个目标窗口（pid + 窗口标识，见 [`crate::focus`]）。
//! 之后每次输出前 [`StandbyGate::ensure_ready`] 都会（节流后）校验这个锁：
//!
//! * 焦点没变（或平台探测不可用）→ 直接放行，不再打扰用户；
//! * 焦点换了窗口 → 停手 50ms 复查（吸收通知/我们自己弹框造成的瞬时抖动），仍不同就
//!   **自动重新待命**并更新锁 —— 用户重新确认后，输出从断点继续。
//!
//! 脚本还可以用 `$.request_focus("提示")` 显式要求一轮新的确认（见
//! [`StandbyGate::request_focus`]），提示语会显示在待命窗口上。
//!
//! # 暂停检测
//!
//! 用户可能要点好几次才能把焦点切到位。待命窗口上的「暂停检测」只是抑制「失焦 = 已确认」
//! 这一个信号（并冻结倒计时），取消/热键照常可用；「恢复检测」把焦点交回待命窗口并重置为
//! 完整的一轮倒计时 —— 于是**下一次失焦仍然等于「用户点了目标窗口」**，回到原来的逻辑。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use clipbeam_scripting::HostError;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::cancel::CancellationToken as WorkerCancel;
use crate::focus::{self, FocusProbe, FocusSignature};
use crate::scripting::CancelHook;
use crate::worker::TaskKind;

/// 门闩放行前的等待粒度：取消（Esc）最多阻塞这么久。
const WAIT_SLICE: Duration = Duration::from_millis(100);

/// 失焦后等系统把焦点真正切过去，再去记录目标窗口。
const FOCUS_SETTLE: Duration = Duration::from_millis(20);

/// 打字过程中最多这么频繁地探测焦点（打字循环每个字符都会调 `ensure_ready`，靠它节流）。
const FOCUS_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// 探测到焦点变化后先停手、再复查一次的延迟（吸收瞬时抖动）。
const FOCUS_RECHECK_DELAY: Duration = Duration::from_millis(50);

/// 待命窗口失焦检测的开关（前端「暂停检测 / 恢复检测」按钮）。
///
/// 跨线程共享：前端点按钮 → Tauri 命令写状态 + 唤醒；`arm` 的驱动任务读状态。
/// 用「状态 + 唤醒」而不是「事件」：通知可以合并甚至丢失，读到最新状态即可。
pub struct StandbyControl {
    active: AtomicBool,
    paused: AtomicBool,
    wake: tokio::sync::Notify,
}

impl Default for StandbyControl {
    fn default() -> Self {
        Self::new()
    }
}

impl StandbyControl {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// 一轮待命开始：标记进行中并复位为「未暂停」（每轮都从原逻辑开始）。
    pub fn begin(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
    }

    /// 一轮待命结束。
    pub fn end(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        // `notify_one` 会留存一个许可：即使此刻驱动任务不在等，也能在下一轮循环里读到状态
        self.wake.notify_one();
    }

    /// 当前是否处于「暂停检测」。
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 设置暂停状态，返回**生效后的「是否暂停」**（调用方直接拿它当按钮状态用）。
    ///
    /// 没有进行中的待命时不做任何改动、返回 `false`（= 未暂停）：这一轮已经结束了，
    /// 前端不该因为一次迟到的点击把按钮停在「已暂停」上。
    pub fn set_paused(&self, paused: bool) -> bool {
        if !self.active.load(Ordering::SeqCst) {
            return false;
        }
        self.paused.store(paused, Ordering::SeqCst);
        self.wake.notify_one();
        // 注意返回的是「生效后的状态」，不是「这次调用是否生效」——
        // 两者在 `paused == false` 时相反，前端拿它就够了。
        paused
    }
}

/// 待命倒计时的策略：**暂停时挂起，恢复时重新给满**。
///
/// 单独抽出来是为了能单测 —— 「暂停期间不该倒计时」这条语义写错过一次
/// （当时阻塞侧另有一个固定上限，用户一暂停，任务过一会儿照样被取消）。
struct StandbyClock {
    timeout: Duration,
    deadline: Instant,
    paused: bool,
}

impl StandbyClock {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            deadline: Instant::now() + timeout,
            paused: false,
        }
    }

    /// 剩余时间；`None` 表示正在暂停（不限时）。
    fn remaining(&self, now: Instant) -> Option<Duration> {
        if self.paused {
            return None;
        }
        Some(self.deadline.saturating_duration_since(now))
    }

    fn is_paused(&self) -> bool {
        self.paused
    }

    /// 暂停：不再计时。
    fn pause(&mut self) {
        self.paused = true;
    }

    /// 恢复：从此刻起重新给满一轮。
    fn resume(&mut self) {
        self.paused = false;
        self.deadline = Instant::now() + self.timeout;
    }
}

/// 门闩的共享状态：是否已锁定、锁定的目标、上次校验时间。
#[derive(Default)]
struct GateState {
    ready: bool,
    target: Option<FocusSignature>,
    checked_at: Option<Instant>,
}

/// 跨线程的「已放行」标志 + 阻塞等待。
///
/// 一侧是 Worker 的脚本/打字线程（阻塞等待），另一侧是 Tauri 窗口事件与异步任务。
/// 用 `(Mutex, Condvar)` 而不是异步通道：输出侧是阻塞调用，没有异步运行时可用。
/// 这一层**不依赖 Tauri**，因此可以单测。
#[derive(Default)]
struct ReadyFlag {
    inner: Arc<(Mutex<GateState>, Condvar)>,
}

impl ReadyFlag {
    fn is_ready(&self) -> bool {
        self.inner.0.lock().unwrap().ready
    }

    fn target(&self) -> Option<FocusSignature> {
        self.inner.0.lock().unwrap().target.clone()
    }

    /// 锁定：记录用户确认的目标，并标记「刚校验过」。
    fn mark_ready(&self, target: Option<FocusSignature>) {
        let (lock, signal) = &*self.inner;
        {
            let mut state = lock.lock().unwrap();
            state.ready = true;
            state.target = target;
            state.checked_at = Some(Instant::now());
        }
        signal.notify_all();
    }

    /// 解除锁定（焦点变了 / 脚本显式要求重新确认）。
    fn clear(&self) {
        let (lock, signal) = &*self.inner;
        {
            let mut state = lock.lock().unwrap();
            state.ready = false;
            state.target = None;
            state.checked_at = None;
        }
        signal.notify_all();
    }

    /// 是否到了下一次探测时间（节流）。
    fn due(&self, now: Instant) -> bool {
        due(self.inner.0.lock().unwrap().checked_at, now)
    }

    /// 记下「刚校验过」。
    fn touch(&self, now: Instant) {
        self.inner.0.lock().unwrap().checked_at = Some(now);
    }

    /// 等待锁定，**不负责弹出待命窗口**（弹窗由 [`StandbyGate::arm`] 负责）。
    ///
    /// 返回 `Ok(())` 表示可以输出；`Err(HostError::Cancelled)` 表示任务被取消
    /// （Esc / 托盘 / 「取消」按钮，或待命超时收尾时设置令牌）。
    ///
    /// **这里刻意不设自己的超时**：超时只属于 [`StandbyGate::arm`] 的驱动任务 —— 它知道
    /// 「暂停检测」有没有生效（暂停期间不该倒计时），也有失败时的兜底分支。两套时钟会互相
    /// 打架：曾经这里有固定的 12 秒上限，用户一按「暂停检测」，任务过一会儿仍会被取消。
    fn wait_ready(&self, cancel: &WorkerCancel) -> Result<(), HostError> {
        let (lock, signal) = &*self.inner;
        let mut ready = lock.lock().unwrap();

        while !ready.ready {
            if cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            let (next, _timeout) = signal
                .wait_timeout(ready, WAIT_SLICE)
                .expect("待命门闩的锁不会中毒");
            ready = next;
        }
        Ok(())
    }
}

/// 待命阶段的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandbyOutcome {
    /// 待命窗口失焦 = 用户把焦点移到了目标窗口 → 可以开始注入键盘事件。
    ///
    /// `target` 是失焦瞬间（settle 之后）探测到的目标窗口；`None` 表示平台探测不可用
    /// （或待命窗口没能弹出）—— 后续按「不校验焦点」处理。
    Ready { target: Option<FocusSignature> },
    /// 到点仍没等到失焦 → 按取消处理（收尾由调用方决定）。
    TimedOut,
    /// 任务在待命期间被用户取消（Esc / 托盘 / 按钮）→ 本轮就此结束。
    ///
    /// 此时调用方**不需要**再跑收尾：取消路径（`WorkerState::cancel`）已经处理过了。
    /// 这个分支必须有：暂停期间没有超时，只有它能让驱动任务及时退出（否则下一轮待命的
    /// 暂停开关会被这个滞留的驱动复位）。
    Cancelled,
}

/// 焦点校验的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// 焦点没变（或没法判断）：继续输出。
    Unchanged,
    /// 焦点换了窗口：需要重新待命。
    Changed,
    /// 探测不可用：按未变化处理（fail-open，行为等同不检测）。
    Unavailable,
}

/// 纯判定：把「记录过的目标」与「当前探测结果」变成结论（便于单测）。
fn evaluate(recorded: Option<&FocusSignature>, probe: FocusProbe) -> Verdict {
    match (recorded, probe) {
        // 没记录过目标（平台探测不可用 / 待命窗口没弹出来）：不阻止输出
        (None, _) => Verdict::Unchanged,
        (_, FocusProbe::Unavailable) => Verdict::Unavailable,
        (Some(recorded), FocusProbe::Target(current)) => {
            if recorded.same_target(&current) {
                Verdict::Unchanged
            } else {
                Verdict::Changed
            }
        }
        // 焦点回到我们自己窗口（待命/主/进度/脚本窗口，或我们弹的系统框）：锁定被破坏
        (Some(_), FocusProbe::SelfApp) => Verdict::Changed,
    }
}

/// 是否到了下一次探测时间（节流）。
fn due(checked_at: Option<Instant>, now: Instant) -> bool {
    match checked_at {
        None => true,
        Some(at) => now.saturating_duration_since(at) >= FOCUS_CHECK_INTERVAL,
    }
}

/// 待命门闩：一个任务/一次脚本一个，负责「锁定一个目标窗口」并守住这个锁。
pub struct StandbyGate {
    app: AppHandle,
    cancel: WorkerCancel,
    /// 超时/异常时的任务级收尾（只做「取消 + 告知用户」，任务生命周期由调用方自己管）。
    on_cancel: CancelHook,
    kind: TaskKind,
    hotkey: String,
    /// 显示在待命窗口上的提示语（`$.request_focus` 传入，空 = 用默认文案）。
    hint: Mutex<Option<String>>,
    ctl: Arc<StandbyControl>,
    flag: ReadyFlag,
}

impl StandbyGate {
    /// 新建一个未锁定的门闩。
    pub fn new(
        app: AppHandle,
        cancel: WorkerCancel,
        on_cancel: CancelHook,
        kind: TaskKind,
        hotkey: String,
    ) -> Self {
        let ctl = app
            .state::<crate::worker::WorkerState>()
            .standby_ctl
            .clone();
        Self {
            app,
            cancel,
            on_cancel,
            kind,
            hotkey,
            hint: Mutex::new(None),
            ctl,
            flag: ReadyFlag::default(),
        }
    }

    /// 是否已经锁定目标窗口。
    pub fn is_ready(&self) -> bool {
        self.flag.is_ready()
    }

    /// 记录「用户已确认这个目标窗口」。
    pub fn confirm(&self, target: Option<FocusSignature>) {
        self.flag.mark_ready(target);
    }

    /// 超时/异常：取消任务并跑调用方的收尾。
    pub fn abort(&self) {
        self.cancel.cancel();
        (self.on_cancel)();
    }

    /// `$.request_focus` 用：显式要求一轮新的焦点确认，`hint` 显示在待命窗口上。
    ///
    /// 无条件重开一轮（哪怕焦点没变）—— 这就是「声明需要新焦点」的语义。
    /// 提示语会被记住，后续自动重锁也沿用它。
    pub fn request_focus(self: &Arc<Self>, hint: &str) -> Result<(), HostError> {
        let hint = hint.trim();
        *self.hint.lock().unwrap() = if hint.is_empty() {
            None
        } else {
            Some(hint.to_string())
        };
        self.flag.clear();
        self.run_round()
    }

    /// 输出前调用：确保锁还在（未锁定就待命；锁定被破坏就自动重新待命）。
    ///
    /// 调用非常频繁（每个字符一次），内部用 [`FOCUS_CHECK_INTERVAL`] 节流。
    pub fn ensure_ready(self: &Arc<Self>) -> Result<(), HostError> {
        if self.is_ready() {
            if !self.flag.due(Instant::now()) {
                return Ok(());
            }
            let recorded = self.flag.target();
            match evaluate(recorded.as_ref(), focus::probe()) {
                Verdict::Unchanged | Verdict::Unavailable => {
                    self.flag.touch(Instant::now());
                    return Ok(());
                }
                Verdict::Changed => {
                    // 先停手再复查一次：系统通知、我们自己的确认框都会造成瞬时失焦，
                    // 不该为它们弹一次待命窗口。
                    std::thread::sleep(FOCUS_RECHECK_DELAY);
                    let recorded = self.flag.target();
                    if evaluate(recorded.as_ref(), focus::probe()) != Verdict::Changed {
                        self.flag.touch(Instant::now());
                        return Ok(());
                    }
                    if let Some(recorded) = &recorded {
                        log::info!(
                            "[standby] 目标窗口焦点已变化（原目标 pid={} 「{}」），重新请求用户确认",
                            recorded.pid(),
                            recorded.label()
                        );
                    }
                    self.flag.clear();
                }
            }
        }

        self.run_round()
    }

    /// 弹一轮待命窗口并等结果：放行则锁定，超时/异常则取消任务。
    ///
    /// 阻塞当前（打字）线程，直到用户确认、取消或超时 —— 与 `$.type_str` 的阻塞语义一致。
    fn run_round(self: &Arc<Self>) -> Result<(), HostError> {
        let hint = self.hint.lock().unwrap().clone();
        let outcome = self.arm(hint);

        let gate = self.clone();
        tauri::async_runtime::spawn(async move {
            match outcome.await {
                Ok(StandbyOutcome::Ready { target }) => {
                    if let Some(target) = &target {
                        log::debug!(
                            "[standby] 已锁定目标窗口：pid={} 「{}」",
                            target.pid(),
                            target.label()
                        );
                    }
                    gate.confirm(target);
                }
                // 驱动任务异常结束（发送端被丢）也按超时处理
                Ok(StandbyOutcome::TimedOut) | Err(_) => gate.abort(),
                // 用户已经取消：取消路径自己做了收尾，这里只结束本轮
                Ok(StandbyOutcome::Cancelled) => {}
            }
        });

        self.flag.wait_ready(&self.cancel)
    }

    /// 弹出待命窗口，并把「失焦（+20ms settle）/ 暂停 / 超时」变成一次性结果。
    ///
    /// 只负责**窗口与事件**：显示、发 `standby-config`（含提示语）、注册失焦监听、
    /// 倒计时、隐藏窗口、发 `standby-start` / `standby-cancel`。
    ///
    /// **超时由这里兜底**（`worker::STANDBY_TIMEOUT_SECS`，暂停期间挂起）：待命窗口被关掉、
    /// 失焦事件丢失这类情况最终都会走到超时分支，因此阻塞侧不需要再设第二个时钟。
    ///
    /// 待命窗口缺失（极少见）时**立即放行**：不因为弹不出窗口就把任务卡死。
    pub fn arm(&self, hint: Option<String>) -> tokio::sync::oneshot::Receiver<StandbyOutcome> {
        // 容量 1：连续失焦不排队，驱动侧取一个就够（要支持暂停后回到等待，所以不能用 oneshot）
        let (blur_tx, mut blur_rx) = tokio::sync::mpsc::channel::<()>(1);
        self.ctl.begin();

        if let Some(window) = self.app.get_webview_window("standby") {
            let _ = window.show();
            let _ = window.set_focus();
            // 只发给待命窗口本身：`emit` 会广播给所有窗口，而各窗口跑的是同一套 Vue 应用，
            // 广播可能让别的窗口也收到本不属于它的事件（见 lib.rs 里 tray-menu 的同类修复）。
            let _ = self.app.emit_to(
                "standby",
                "standby-config",
                serde_json::json!({ "kind": self.kind, "hotkey": self.hotkey, "hint": hint }),
            );

            window.on_window_event(move |event| {
                if let WindowEvent::Focused(false) = event {
                    let _ = blur_tx.try_send(());
                }
            });
        }

        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        let app = self.app.clone();
        let ctl = self.ctl.clone();
        let cancel = self.cancel.clone();
        tauri::async_runtime::spawn(async move {
            let mut clock =
                StandbyClock::new(Duration::from_secs(crate::worker::STANDBY_TIMEOUT_SECS));

            let outcome = loop {
                // `None` = 正在暂停：这个分支不参与（用户可以慢慢切焦点，取消/热键始终可用）
                let left = clock.remaining(Instant::now());
                tokio::select! {
                    // 待命窗口不存在时没人持有发送端，这里会立刻得到 `None` —— 同「没弹出来」，
                    // 直接放行（见上面的 fail-open 说明）。
                    blur = blur_rx.recv() => {
                        if blur.is_none() {
                            break StandbyOutcome::Ready { target: None };
                        }
                        if clock.is_paused() {
                            continue;
                        }
                        // 给系统一点时间把焦点真正切过去，再去读「焦点落在哪儿」
                        tokio::time::sleep(FOCUS_SETTLE).await;
                        match focus::probe() {
                            FocusProbe::Target(target) => break StandbyOutcome::Ready { target: Some(target) },
                            // 探测不可用：按「没弹出来」放行（fail-open）
                            FocusProbe::Unavailable => break StandbyOutcome::Ready { target: None },
                            // 焦点还在我们自己窗口上：不算确认，把焦点拿回来继续等
                            FocusProbe::SelfApp => {
                                if let Some(window) = app.get_webview_window("standby") {
                                    let _ = window.set_focus();
                                }
                                continue;
                            }
                        }
                    }
                    _ = ctl.wake.notified() => {
                        if ctl.is_paused() {
                            clock.pause();
                        } else {
                            // 恢复检测：回到「下一次失焦即确认」，倒计时重新给满
                            clock.resume();
                        }
                    }
                    _ = tokio::time::sleep_until((Instant::now() + left.unwrap_or_default()).into()), if left.is_some() => {
                        break StandbyOutcome::TimedOut;
                    }
                    // 取消感知（Esc / 托盘 / 按钮）：暂停时没有超时，只有这里能让驱动退出，
                    // 否则滞留的驱动会在下一轮待命里把暂停开关复位。
                    _ = tokio::time::sleep(WAIT_SLICE) => {
                        if cancel.is_cancelled() {
                            break StandbyOutcome::Cancelled;
                        }
                    }
                }
            };

            ctl.end();
            if let Some(window) = app.get_webview_window("standby") {
                let _ = window.hide();
            }
            // 同样只发给待命窗口：这两个事件只用于收起/提示它自己
            let _ = app.emit_to(
                "standby",
                match outcome {
                    StandbyOutcome::Ready { .. } => "standby-start",
                    StandbyOutcome::TimedOut | StandbyOutcome::Cancelled => "standby-cancel",
                },
                (),
            );
            let _ = outcome_tx.send(outcome);
        });

        outcome_rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(pid: i32, key: &str) -> FocusSignature {
        // 测试里只用 pid + key 比较，label 随意
        FocusSignature::new(pid, "测试窗口", key)
    }

    #[test]
    fn wait_returns_immediately_after_mark_ready() {
        let flag = ReadyFlag::default();
        assert!(!flag.is_ready());
        flag.mark_ready(None);
        assert!(flag.is_ready());

        let cancel = WorkerCancel::new();
        let started = Instant::now();
        flag.wait_ready(&cancel).expect("已放行应当直接返回");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "已放行时不应等待：{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn wait_unblocks_when_marked_from_another_thread() {
        let flag = Arc::new(ReadyFlag::default());
        let cancel = WorkerCancel::new();

        let flag_clone = flag.clone();
        let waiter = std::thread::spawn(move || {
            let started = Instant::now();
            flag_clone.wait_ready(&cancel).expect("放行后应当成功");
            started.elapsed()
        });

        std::thread::sleep(Duration::from_millis(120));
        flag.mark_ready(None);

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
        let flag = Arc::new(ReadyFlag::default());
        let cancel = WorkerCancel::new();

        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            cancel_clone.cancel();
        });

        let started = Instant::now();
        let result = flag.wait_ready(&cancel);
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

    #[test]
    fn clear_unlocks_again() {
        let flag = ReadyFlag::default();
        flag.mark_ready(Some(signature(42, "10,20,300,400")));
        assert!(flag.is_ready());
        flag.clear();
        assert!(!flag.is_ready(), "解除锁定后应当重新等待");
        assert!(flag.target().is_none());
    }

    #[test]
    fn evaluate_follows_lock_semantics() {
        let recorded = signature(42, "10,20,300,400");
        let same = signature(42, "10,20,300,400");
        // 同一个窗口，但标题被改了（浏览器切标签、文档改名）：label 不参与比较
        let same_window_new_title = FocusSignature::new(42, "改过标题的记事本", "10,20,300,400");
        let other_window = signature(42, "11,20,300,400");
        let other_app = signature(43, "10,20,300,400");

        // 没记录 / 探测不可用 → fail-open
        assert_eq!(
            evaluate(None, FocusProbe::Target(same.clone())),
            Verdict::Unchanged
        );
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::Unavailable),
            Verdict::Unavailable
        );
        // 同一个窗口 → 不变
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::Target(same)),
            Verdict::Unchanged
        );
        // 标题变了但 pid/key 一样 → 仍算同一个（label 不参与比较）
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::Target(same_window_new_title)),
            Verdict::Unchanged
        );
        // 换了窗口 / 换了应用 → 变了
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::Target(other_window)),
            Verdict::Changed
        );
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::Target(other_app)),
            Verdict::Changed
        );
        // 焦点回到我们自己 → 变了
        assert_eq!(
            evaluate(Some(&recorded), FocusProbe::SelfApp),
            Verdict::Changed
        );
    }

    #[test]
    fn focus_check_is_throttled() {
        let now = Instant::now();
        assert!(due(None, now), "还没校验过就应当立刻校验");
        assert!(!due(Some(now), now), "刚校验过不应再校验");
        assert!(
            !due(Some(now), now + FOCUS_CHECK_INTERVAL / 2),
            "节流窗口内不应再校验"
        );
        assert!(due(Some(now), now + FOCUS_CHECK_INTERVAL), "到点应当再校验");
    }

    #[test]
    fn set_paused_reports_effective_state() {
        let ctl = StandbyControl::new();
        // 没有进行中的待命：不改动，回答「未暂停」
        assert!(!ctl.set_paused(true), "没有进行中的待命时应当返回未暂停");
        assert!(!ctl.is_paused());

        ctl.begin();
        assert!(ctl.set_paused(true), "暂停应当返回 true");
        assert!(ctl.is_paused());
        // 恢复必须返回 false —— 返回「这次调用是否生效」会让前端以为自己还在暂停
        assert!(!ctl.set_paused(false), "恢复应当返回 false");
        assert!(!ctl.is_paused());

        ctl.end();
        assert!(
            !ctl.set_paused(true),
            "待命结束后不应再能暂停，且回答未暂停"
        );
    }

    #[test]
    fn standby_clock_suspends_while_paused() {
        let mut clock = StandbyClock::new(Duration::from_secs(10));
        assert!(clock.remaining(Instant::now()).is_some());
        assert!(!clock.is_paused());

        clock.pause();
        assert!(clock.is_paused());
        // 暂停后再久都不该倒计时：用户想切多久就切多久
        assert!(
            clock
                .remaining(Instant::now() + Duration::from_secs(3600))
                .is_none(),
            "暂停期间不该有剩余时间（= 不该超时）"
        );

        clock.resume();
        assert!(!clock.is_paused());
        let left = clock.remaining(Instant::now()).expect("恢复后应当重新计时");
        assert!(
            left > Duration::from_secs(9) && left <= Duration::from_secs(10),
            "恢复应当重新给满一轮：{left:?}"
        );
    }

    #[test]
    fn standby_clock_counts_down_when_running() {
        let clock = StandbyClock::new(Duration::from_secs(10));
        let now = Instant::now();
        let left = clock.remaining(now).expect("未暂停时应当有剩余时间");
        assert!(left <= Duration::from_secs(10));
        assert!(
            clock
                .remaining(now + Duration::from_secs(11))
                .expect("未暂停")
                .is_zero(),
            "到点后剩余时间应当归零（交给调用方收尾）"
        );
    }

    #[test]
    fn begin_resets_paused_state() {
        let ctl = StandbyControl::new();
        ctl.begin();
        ctl.set_paused(true);
        ctl.end();
        ctl.begin();
        assert!(!ctl.is_paused(), "新一轮待命应当从未暂停开始");
    }
}
