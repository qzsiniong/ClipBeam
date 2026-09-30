//! 插件宿主实现：把 `$plugin` 的能力落到 Tauri 的真实 UI 上。
//!
//! # 与脚本宿主的区别
//!
//! [`crate::scripting::TauriScriptHost`] 是**脚本**的宿主（逐键打字、待命窗口、文件授权），
//! 本文件是**插件**的宿主（应用内提示、系统通知、原生对话框、托盘）。两者都实现
//! 各自的 trait，共用同一个 `console`/托盘组件，但语义不混。
//!
//! # 线程与死锁
//!
//! 插件跑在自己的线程上（见 [`crate::plugin_manager`]），所以这里可以**阻塞等 UI**：
//!
//! * toast / 系统通知：投递即返回（fire-and-forget）；
//! * 对话框：走 `tauri-plugin-dialog` 的阻塞 API（脚本侧已经在 worker 线程上这么用了），
//!   有超时时改成「另一条线程跑阻塞对话框 + `recv_timeout`」；
//! * 托盘：托盘 API 必须在**主线程**上调用，所以发一条事件回主线程执行，结果经
//!   oneshot 回来 —— **绝不阻塞主线程**（主线程阻塞住就没人处理那条事件了，必然死锁）。
//!
//! 所有等待都有上限：插件线程可以等，但不能永久等。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use clipbeam_plugins::{
    DialogButtons, DialogChoice, Feedback, FeedbackOutcome, PluginError, PluginHost, PluginMeta,
    TrayOutcome, TrayRequest,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use crate::plugin_manager::PluginManager;
use crate::tray;

/// 托盘运行时请求的事件名（由主线程上的监听者处理，见 `tray::setup_plugin_events`）。
pub const TRAY_REQUEST_EVENT: &str = "plugin-tray-request";

/// 托盘请求的等待上限：主线程忙不过来时不让插件线程一直吊着。
const TRAY_TIMEOUT_MS: u64 = 3_000;

/// 对话框默认标题（插件没给 `title` 时用）。
const DIALOG_TITLE: &str = "ClipBeam 插件";

/// 托盘请求（插件线程 → 主线程）。
///
/// 需要 `Deserialize`：它经 Tauri 事件（JSON 载荷）抵达主线程。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrayRequestMessage {
    /// 请求序号（回执靠它配对）。
    pub request_id: u64,
    /// 哪个插件发的（日志与限流用）。
    pub plugin_id: String,
    /// 请求内容。
    pub request: TrayRequest,
}

/// 托盘请求的回执（主线程 → 插件线程）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TrayRequestResult {
    /// 与请求配对的序号。
    pub request_id: u64,
    /// 成功与否。
    pub outcome: Result<TrayOutcome, PluginError>,
}

/// 挂着待回执请求的表（主线程处理完事件后按 `request_id` 找到对应的发送端）。
#[derive(Default)]
pub struct PendingTrayRequests {
    senders: std::sync::Mutex<std::collections::HashMap<u64, mpsc::Sender<TrayRequestResult>>>,
}

impl PendingTrayRequests {
    /// 登记一个等待中的请求。
    pub fn register(&self, request_id: u64) -> mpsc::Receiver<TrayRequestResult> {
        let (tx, rx) = mpsc::channel();
        self.senders.lock().unwrap().insert(request_id, tx);
        rx
    }

    /// 回执一个请求（主线程调用）。
    pub fn resolve(&self, result: TrayRequestResult) {
        let sender = self.senders.lock().unwrap().remove(&result.request_id);
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }

    /// 丢掉一个请求（超时后清理，避免表无限增长）。
    pub fn forget(&self, request_id: u64) {
        self.senders.lock().unwrap().remove(&request_id);
    }
}

/// GUI 侧插件宿主。
pub struct TauriPluginHost {
    app: AppHandle,
    meta: PluginMeta,
    /// 停用信号（由管理器在 `disable` 里置位）。
    cancel: script_engine::CancelSignal,
    /// 是否已被要求停止。
    ///
    /// 与 [`Self::cancel`] 的区别：取消信号由管理器置位、**引擎**也看它（`sleep` 会提前返回）；
    /// 这个标志留给宿主实现自己标记（例如宿主发现插件已不在 UI 上、提前拒绝新动作）。
    stopped: AtomicBool,
    /// 托盘请求序号。
    next_request: AtomicU64,
}

impl TauriPluginHost {
    /// 为一个插件创建宿主。
    pub fn new(app: AppHandle, meta: PluginMeta, cancel: script_engine::CancelSignal) -> Self {
        Self {
            app,
            meta,
            cancel,
            stopped: AtomicBool::new(false),
            next_request: AtomicU64::new(1),
        }
    }

    /// 走一条「阻塞对话框 + 超时」的路径。
    ///
    /// 为什么要另开线程：`tauri-plugin-dialog` 的 `blocking_show_with_result` 会一直堵到
    /// 用户回答。没有超时的话，一个没人理会的对话框会永久占住插件线程 —— 停用插件时
    /// 连 `Stop` 都处理不了。另开一条线程跑对话框，本线程用 `recv_timeout` 等：
    /// 超时就返回 `DialogChoice::Timeout`，对话框本身留给用户关掉。
    fn dialog_with_timeout(
        &self,
        title: Option<&str>,
        message: &str,
        buttons: MessageDialogButtons,
        timeout_ms: Option<u64>,
    ) -> DialogChoice {
        let app = self.app.clone();
        let title = title.unwrap_or(DIALOG_TITLE).to_string();
        let message = message.to_string();
        let has_timeout = timeout_ms.is_some();
        // 自定义按钮的结果按**文案**回传，因此要把对话框的原始结果带回来再映射
        let custom_labels = match &buttons {
            MessageDialogButtons::OkCancelCustom(primary, secondary) => {
                Some(vec![primary.clone(), secondary.clone()])
            }
            MessageDialogButtons::YesNoCancelCustom(primary, secondary, tertiary) => {
                Some(vec![primary.clone(), secondary.clone(), tertiary.clone()])
            }
            _ => None,
        };

        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("clipbeam-dialog-{}", self.meta.id))
            .spawn(move || {
                let result = app
                    .dialog()
                    .message(message)
                    .title(title)
                    .kind(MessageDialogKind::Info)
                    .buttons(buttons)
                    .blocking_show_with_result();
                // 接收端超时后已经被丢弃：这里吞掉发送失败
                let _ = tx.send(result);
            })
            .ok();

        let raw = if has_timeout {
            let budget = Duration::from_millis(timeout_ms.unwrap_or(TRAY_TIMEOUT_MS));
            match rx.recv_timeout(budget) {
                Ok(result) => result,
                Err(_) => return DialogChoice::Timeout,
            }
        } else {
            // 不超时：一直等（用户迟早会点）——但停用插件时也要能收手，
            // 所以按固定粒度轮询取消信号
            match self.wait_forever(rx) {
                Some(result) => result,
                None => return DialogChoice::Dismissed,
            }
        };

        // 自定义按钮：按文案反查位置；其余按钮直接用位置
        match &custom_labels {
            Some(labels) => map_custom_labels(&raw, labels),
            None => map_dialog_result(raw),
        }
    }

    /// 「不超时」的等待：轮询取消信号，停用插件时立刻返回 `None`。
    fn wait_forever(&self, rx: mpsc::Receiver<MessageDialogResult>) -> Option<MessageDialogResult> {
        loop {
            if self.stopped() {
                return None;
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(result) => return Some(result),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // 对话框线程没了（平台失败）：按「直接关掉」处理
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// 把一条托盘请求送到主线程执行，并等回执。
    fn request_tray(&self, request: TrayRequest) -> Result<TrayOutcome, PluginError> {
        let request_id = self.next_request.fetch_add(1, Ordering::SeqCst);
        let pending = self.app.state::<Arc<PendingTrayRequests>>().inner().clone();

        let rx = pending.register(request_id);
        let message = TrayRequestMessage {
            request_id,
            plugin_id: self.meta.id.clone(),
            request,
        };

        // `emit` 本身线程安全：主线程上的 `listen_any` 会按序处理
        self.app
            .emit(TRAY_REQUEST_EVENT, message)
            .map_err(|err| PluginError::Failed(format!("托盘请求发送失败：{err}")))?;

        match rx.recv_timeout(Duration::from_millis(TRAY_TIMEOUT_MS)) {
            Ok(result) => result.outcome,
            Err(_) => {
                pending.forget(request_id);
                Err(PluginError::Failed(
                    "托盘操作超时（应用主线程没有响应）".to_string(),
                ))
            }
        }
    }
}

impl PluginHost for TauriPluginHost {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn feedback(&self, request: Feedback) -> Result<FeedbackOutcome, PluginError> {
        if self.stopped() {
            return Err(PluginError::Stopped);
        }

        match request {
            Feedback::Toast {
                level,
                message,
                duration_ms,
            } => {
                // 应用内提示：发给所有窗口（主窗口与脚本窗口各显示一次；
                // 插件的动作多半是从托盘触发，那时两个窗口都可能是用户正在看的那一个）
                let _ = self.app.emit(
                    "plugin-toast",
                    PluginToast {
                        plugin_id: self.meta.id.clone(),
                        plugin_name: self.meta.name.clone(),
                        level,
                        message,
                        duration_ms,
                    },
                );
                Ok(FeedbackOutcome::Delivered)
            }

            Feedback::Notify { title, body } => {
                crate::notify::notify(&title, body.as_deref().unwrap_or(""));
                Ok(FeedbackOutcome::Delivered)
            }

            Feedback::Dialog {
                title,
                message,
                buttons,
                timeout_ms,
            } => {
                let (buttons, _) = to_dialog_buttons(&buttons);
                let choice =
                    self.dialog_with_timeout(title.as_deref(), &message, buttons, timeout_ms);
                Ok(FeedbackOutcome::Chosen { choice })
            }
        }
    }

    fn tray(&self, request: TrayRequest) -> Result<TrayOutcome, PluginError> {
        if self.stopped() {
            return Err(PluginError::Stopped);
        }
        self.request_tray(request)
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst) || self.cancel.is_cancelled()
    }
}

/// toast 事件载荷（前端渲染用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginToast {
    /// 哪个插件发的。
    pub plugin_id: String,
    /// 插件显示名（toast 上标明来源）。
    pub plugin_name: String,
    /// 等级（决定配色）。
    pub level: clipbeam_plugins::ToastLevel,
    /// 文本。
    pub message: String,
    /// 自动消失时间；`None` 用前端默认。
    pub duration_ms: Option<u64>,
}

/// 把插件的按钮组合转成群主对话框的按钮。
///
/// 返回 `(按钮组合, 是否需要按文案反查位置)`：自定义按钮的 `with_result` 会把
/// **按钮文案**回传，因此结果映射要看文案。
fn to_dialog_buttons(buttons: &DialogButtons) -> (MessageDialogButtons, bool) {
    match buttons {
        DialogButtons::Ok => (MessageDialogButtons::Ok, false),
        DialogButtons::OkCancel => (MessageDialogButtons::OkCancel, false),
        DialogButtons::Custom {
            primary,
            secondary,
            tertiary,
        } => {
            // 群主对话框的自定义按钮是「主 / 次 / 第三」三档；缺省文案必须是不同的字，
            // 否则结果里无法区分（平台要求按钮文案唯一）
            let primary_text = primary.clone();
            let secondary_text = secondary.clone().unwrap_or_else(|| "取消".to_string());
            let tertiary_text = match tertiary {
                Some(text) => text.clone(),
                // 没有第三个按钮时，把它退化成「关闭/取消」的语义：文案与次按钮不同即可
                None => String::new(),
            };

            if tertiary_text.is_empty() {
                (
                    MessageDialogButtons::OkCancelCustom(primary_text, secondary_text),
                    true,
                )
            } else {
                (
                    MessageDialogButtons::YesNoCancelCustom(
                        primary_text,
                        secondary_text,
                        tertiary_text,
                    ),
                    true,
                )
            }
        }
    }
}

/// 把群主对话框的结果映射成插件的回答。
///
/// 自定义按钮的结果按**文案**回传，所以这里要带上三个文案做反查。
fn map_dialog_result(result: MessageDialogResult) -> DialogChoice {
    match result {
        MessageDialogResult::Ok | MessageDialogResult::Yes => DialogChoice::Primary,
        MessageDialogResult::No => DialogChoice::Secondary,
        MessageDialogResult::Cancel => DialogChoice::Dismissed,
        // `Custom` 分支在已知文案时由调用方进一步映射；这里退化成第三档处理
        MessageDialogResult::Custom(_) => DialogChoice::Tertiary,
    }
}

/// 自定义按钮的结果 → 位置。
///
/// `MessageDialogButtons::*Custom` 的返回值是**按钮文案**而不是位置，所以必须拿原始文案
/// 来反查。看不到的文案（平台给了别的东西）按「直接关掉」处理 —— 不瞎猜成主按钮。
fn map_custom_labels(result: &MessageDialogResult, labels: &[String]) -> DialogChoice {
    match result {
        MessageDialogResult::Custom(text) => match labels.iter().position(|label| label == text) {
            Some(0) => DialogChoice::Primary,
            Some(1) => DialogChoice::Secondary,
            Some(_) => DialogChoice::Tertiary,
            None => DialogChoice::Dismissed,
        },
        other => map_dialog_result(other.clone()),
    }
}

/// 托盘请求的处理入口（主线程调用，见 `crate::tray::setup_plugin_events`）。
pub fn handle_tray_request(app: &AppHandle, message: TrayRequestMessage) -> TrayRequestResult {
    let plugin_dir = app
        .try_state::<PluginManager>()
        .map(|manager| manager.plugin_dir(&message.plugin_id))
        .unwrap_or(None);

    let outcome = tray::apply_plugin_tray_request(
        app,
        &message.plugin_id,
        plugin_dir.as_deref(),
        &message.request,
    );

    TrayRequestResult {
        request_id: message.request_id,
        outcome,
    }
}

/// 图标路径解析（相对插件目录 → 绝对路径），并做越界校验。
///
/// 与插件窗口的页面路径同一套规则：只允许插件目录内的相对路径。
/// 由 `$plugin.tray.setIcon` 与下一轮的插件窗口共用。
#[allow(
    dead_code,
    reason = "下一轮的插件窗口也用它；本轮托盘图标走 tray.rs 的同名解析"
)]
pub fn resolve_plugin_file(
    plugin_dir: Option<&std::path::Path>,
    relative: &str,
) -> Result<PathBuf, PluginError> {
    let Some(dir) = plugin_dir else {
        return Err(PluginError::Failed("找不到插件目录".to_string()));
    };
    crate::plugin_window::resolve_plugin_file(dir, relative).map_err(PluginError::InvalidArgument)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_buttons_fall_back_to_ok_cancel() {
        let (buttons, needs_mapping) = to_dialog_buttons(&DialogButtons::Custom {
            primary: "发送".into(),
            secondary: None,
            tertiary: None,
        });
        assert!(needs_mapping);
        assert!(matches!(
            buttons,
            MessageDialogButtons::OkCancelCustom(_, _)
        ));
    }

    #[test]
    fn custom_buttons_use_three_slots_when_tertiary_is_present() {
        let (buttons, _) = to_dialog_buttons(&DialogButtons::Custom {
            primary: "发送".into(),
            secondary: Some("取消".into()),
            tertiary: Some("稍后".into()),
        });
        assert!(matches!(
            buttons,
            MessageDialogButtons::YesNoCancelCustom(_, _, _)
        ));
    }

    #[test]
    fn plain_buttons_map_directly() {
        assert!(matches!(
            to_dialog_buttons(&DialogButtons::Ok).0,
            MessageDialogButtons::Ok
        ));
        assert!(matches!(
            to_dialog_buttons(&DialogButtons::OkCancel).0,
            MessageDialogButtons::OkCancel
        ));
    }

    /// 自定义按钮按文案反查位置：这是自定义按钮能正确映射的唯一依据。
    #[test]
    fn custom_labels_map_by_text() {
        let labels = vec!["发送".to_string(), "取消".to_string(), "稍后".to_string()];
        assert_eq!(
            map_custom_labels(&MessageDialogResult::Custom("发送".into()), &labels),
            DialogChoice::Primary
        );
        assert_eq!(
            map_custom_labels(&MessageDialogResult::Custom("取消".into()), &labels),
            DialogChoice::Secondary
        );
        assert_eq!(
            map_custom_labels(&MessageDialogResult::Custom("稍后".into()), &labels),
            DialogChoice::Tertiary
        );
        // 平台给了不认识的文案 → 按「关掉了」处理，不瞎猜成主按钮
        assert_eq!(
            map_custom_labels(&MessageDialogResult::Custom("?".into()), &labels),
            DialogChoice::Dismissed
        );
        // 平台直接返回了标准按钮 → 走普通映射
        assert_eq!(
            map_custom_labels(&MessageDialogResult::Cancel, &labels),
            DialogChoice::Dismissed
        );
    }

    #[test]
    fn dialog_result_maps_to_choice() {
        assert_eq!(
            map_dialog_result(MessageDialogResult::Ok),
            DialogChoice::Primary
        );
        assert_eq!(
            map_dialog_result(MessageDialogResult::Yes),
            DialogChoice::Primary
        );
        assert_eq!(
            map_dialog_result(MessageDialogResult::No),
            DialogChoice::Secondary
        );
        assert_eq!(
            map_dialog_result(MessageDialogResult::Cancel),
            DialogChoice::Dismissed
        );
    }
}
