//! 测试替身：一个把「收到了什么请求」记下来的插件宿主。
//!
//! 启用条件：`cfg(test)`（crate 内单测）或 `test-util` feature（`tests/` 下的集成测试）——
//! 集成测试是独立 crate，看不到 `#[cfg(test)]` 的东西，所以得有个 feature 开关。
//!
//! 为什么值得单独写一份：插件能力的正确性（**权限拒绝、参数校验、错误文案**）不需要
//! Tauri、不需要图形环境，用这个假宿主就能在 CI 的任何机器上跑。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::PluginError;
use crate::host::{
    Feedback, FeedbackOutcome, PluginHost, PluginMeta, ToastLevel, TrayOutcome, TrayRequest,
    WindowNotice, WindowRequest,
};

/// 记录型插件宿主。
pub struct FakePluginHost {
    meta: PluginMeta,
    /// 收到的反馈请求（按顺序）。
    feedbacks: Mutex<Vec<Feedback>>,
    /// 收到的托盘请求（按顺序）。
    trays: Mutex<Vec<TrayRequest>>,
    /// 收到的窗口请求（按顺序）。
    windows: Mutex<Vec<WindowRequest>>,
    /// 宿主推给插件线程的窗口事件（按顺序）—— 测试里用来验证「消息真的送到了插件」。
    notices: Mutex<Vec<WindowNotice>>,
    /// `feedback` 的回答；默认 [`FeedbackOutcome::Delivered`]。
    ///
    /// 对话框场景要能模拟「用户点了次按钮」「超时」等，所以这里是可配置的。
    dialog_answer: Mutex<FeedbackOutcome>,
    /// `stopped()` 的回答。
    stopped: AtomicBool,
    /// 强制 `feedback` 返回错误（模拟宿主侧失败）。
    fail_feedback: Mutex<Option<PluginError>>,
}

impl FakePluginHost {
    /// 用一个身份建宿主。
    pub fn new(meta: PluginMeta) -> Self {
        Self {
            meta,
            feedbacks: Mutex::new(Vec::new()),
            trays: Mutex::new(Vec::new()),
            windows: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            dialog_answer: Mutex::new(FeedbackOutcome::Delivered),
            stopped: AtomicBool::new(false),
            fail_feedback: Mutex::new(None),
        }
    }

    /// 只给 id 的快捷构造（目录取 `/tmp/<id>`，测试不落盘时够用）。
    pub fn with_id(id: &str) -> Self {
        Self::new(PluginMeta {
            id: id.to_string(),
            name: id.to_string(),
            version: "0.0.0".to_string(),
            description: None,
            author: None,
            dir: PathBuf::from(format!("/tmp/{id}")),
            entry: "index.js".to_string(),
        })
    }

    /// 设定对话框的回答。
    pub fn set_dialog_answer(&self, outcome: FeedbackOutcome) {
        *self.dialog_answer.lock().unwrap() = outcome;
    }

    /// 设定 `stopped()` 的回答。
    pub fn set_stopped(&self, stopped: bool) {
        self.stopped.store(stopped, Ordering::SeqCst);
    }

    /// 让 `feedback` 一律失败（模拟宿主侧出错）。
    pub fn set_feedback_error(&self, err: Option<PluginError>) {
        *self.fail_feedback.lock().unwrap() = err;
    }

    /// 收到的全部反馈请求。
    pub fn feedbacks(&self) -> Vec<Feedback> {
        self.feedbacks.lock().unwrap().clone()
    }

    /// 收到的全部托盘请求。
    pub fn trays(&self) -> Vec<TrayRequest> {
        self.trays.lock().unwrap().clone()
    }

    /// 收到的窗口请求。
    pub fn windows(&self) -> Vec<WindowRequest> {
        self.windows.lock().unwrap().clone()
    }

    /// 宿主推给插件线程的窗口事件。
    pub fn notices(&self) -> Vec<WindowNotice> {
        self.notices.lock().unwrap().clone()
    }

    /// 收到的 toast 列表：`(level, message)`。
    pub fn toasts(&self) -> Vec<(ToastLevel, String)> {
        self.feedbacks()
            .into_iter()
            .filter_map(|request| match request {
                Feedback::Toast { level, message, .. } => Some((level, message)),
                _ => None,
            })
            .collect()
    }
}

impl PluginHost for FakePluginHost {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn feedback(&self, request: Feedback) -> Result<FeedbackOutcome, PluginError> {
        if let Some(err) = self.fail_feedback.lock().unwrap().clone() {
            return Err(err);
        }

        let answer = match &request {
            Feedback::Dialog { .. } => *self.dialog_answer.lock().unwrap(),
            // toast / 系统通知不需要用户回答
            _ => FeedbackOutcome::Delivered,
        };
        self.feedbacks.lock().unwrap().push(request);
        Ok(answer)
    }

    fn tray(&self, request: TrayRequest) -> Result<TrayOutcome, PluginError> {
        self.trays.lock().unwrap().push(request);
        Ok(TrayOutcome::Applied)
    }

    fn window(&self, request: WindowRequest) -> Result<(String, u32), PluginError> {
        self.windows.lock().unwrap().push(request.clone());
        match request {
            // 假宿主也按「标签可预测」的规则回答，好让调用方不必特判
            WindowRequest::Open { .. } => {
                let seq = 1;
                Ok((format!("plugin-window-{}-{seq}", self.meta.id), seq))
            }
            WindowRequest::Close { window_id } => Ok((window_id, 0)),
        }
    }

    fn notify_window_event(&self, notice: WindowNotice) -> Result<(), PluginError> {
        self.notices.lock().unwrap().push(notice);
        Ok(())
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// 把假宿主包成 `Arc` 的快捷方式。
pub fn fake_host(host: FakePluginHost) -> Arc<dyn PluginHost> {
    Arc::new(host)
}
