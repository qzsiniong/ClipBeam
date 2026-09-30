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
    TrayOutcome, TrayRequest, WindowNotice, WindowRequest, WindowResponse,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use crate::plugin_manager::PluginManager;
use crate::plugin_window::OpenWindow;
use crate::tray;

/// 托盘运行时请求的事件名（由主线程上的监听者处理，见 `tray::setup_plugin_events`）。
pub const TRAY_REQUEST_EVENT: &str = "plugin-tray-request";

/// 窗口请求的事件名（同样由主线程上的监听者处理）。
pub const WINDOW_REQUEST_EVENT: &str = "plugin-window-request";

/// 窗口请求的等待上限。
///
/// 比托盘长一点：建窗要等系统分配资源，而插件线程等这几秒没有代价
/// （它与应用主线程是分开的）。
const WINDOW_TIMEOUT_MS: u64 = 5_000;

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

/// 窗口请求（插件线程 → 主线程）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRequestMessage {
    /// 请求序号（回执靠它配对）。
    pub request_id: u64,
    /// 哪个插件发的。
    pub plugin_id: String,
    /// 请求内容。
    pub request: WindowRequest,
}

/// 窗口请求的回执（主线程 → 插件线程）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRequestResult {
    /// 与请求配对的序号。
    pub request_id: u64,
    /// 成功时是 `(窗口标签, 序号)`。
    pub outcome: Result<Option<(String, u32)>, PluginError>,
}

/// 挂着待回执的窗口请求。
///
/// 与 [`PendingTrayRequests`] 是同一个手法、同一个理由：窗口动作只能在**主线程**做，
/// 插件线程发一条事件出去、用 `recv_timeout` 等回执；主线程处理完按 `request_id` 找回发送端。
#[derive(Default)]
pub struct PendingWindowRequests {
    senders: std::sync::Mutex<std::collections::HashMap<u64, mpsc::Sender<WindowRequestResult>>>,
}

impl PendingWindowRequests {
    /// 登记一个等待中的请求。
    pub fn register(&self, request_id: u64) -> mpsc::Receiver<WindowRequestResult> {
        let (tx, rx) = mpsc::channel();
        self.senders.lock().unwrap().insert(request_id, tx);
        rx
    }

    /// 回执一个请求（主线程调用）。
    pub fn resolve(&self, result: WindowRequestResult) {
        let sender = self.senders.lock().unwrap().remove(&result.request_id);
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }

    /// 丢掉一个请求（超时后清理）。
    pub fn forget(&self, request_id: u64) {
        self.senders.lock().unwrap().remove(&request_id);
    }
}

/// 当前打开的插件窗口（登记在册，供路由与停用清理用）。
///
/// **只有主线程写**（窗口请求与关闭事件都发生在主线程），其它线程只读。
#[derive(Default)]
pub struct PluginWindows {
    inner: std::sync::Mutex<Vec<OpenWindow>>,
    /// 窗口序号：全局自增，保证标签不重复。
    next_slot: AtomicU64,
}

impl PluginWindows {
    /// 分配一个序号并登记一个窗口，返回登记结果。
    pub fn open(
        &self,
        plugin_id: &str,
        plugin_window_id: &str,
        title: &str,
        page: &str,
    ) -> OpenWindow {
        let slot = self.next_slot.fetch_add(1, Ordering::SeqCst) as u32 + 1;
        let window = OpenWindow {
            label: crate::plugin_window::plugin_window_label(plugin_id, slot),
            plugin_window_id: plugin_window_id.to_string(),
            plugin_id: plugin_id.to_string(),
            page: page.to_string(),
            title: title.to_string(),
            opened_at_ms: now_ms(),
        };
        self.inner.lock().unwrap().push(window.clone());
        window
    }

    /// 按标签取一个窗口。
    pub fn by_label(&self, label: &str) -> Option<OpenWindow> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|window| window.label == label)
            .cloned()
    }

    /// 按「插件 + 插件侧窗口 id」取一个窗口。
    pub fn by_plugin_window_id(&self, plugin_id: &str, window_id: &str) -> Option<OpenWindow> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|window| window.plugin_id == plugin_id && window.plugin_window_id == window_id)
            .cloned()
    }

    /// 摘掉一个窗口，返回它（没有则 `None`）。
    pub fn remove(&self, label: &str) -> Option<OpenWindow> {
        let mut windows = self.inner.lock().unwrap();
        let index = windows.iter().position(|window| window.label == label)?;
        Some(windows.remove(index))
    }

    /// 摘掉某个插件的**全部**窗口（停用/重载插件时用）。
    pub fn remove_all_of(&self, plugin_id: &str) -> Vec<OpenWindow> {
        let mut windows = self.inner.lock().unwrap();
        let taken: Vec<OpenWindow> = windows
            .iter()
            .filter(|window| window.plugin_id == plugin_id)
            .cloned()
            .collect();
        windows.retain(|window| window.plugin_id != plugin_id);
        taken
    }

    /// 当前窗口标签（诊断用；单测拿它断言登记表的行为）。
    ///
    /// `cfg_attr`：lib target 里没有测试代码，clippy 会把「只有测试用」当成死代码；
    /// 测试构建下则照常检查（所以它不会真的腐烂）。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "只在单测里用到：登记表的行为由测试钉住")
    )]
    pub fn labels(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|window| window.label.clone())
            .collect()
    }

    /// 是否一个窗口都没有。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "只在单测里用到：登记表的行为由测试钉住")
    )]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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

impl TauriPluginHost {
    /// 按插件侧窗口 id 找到宿主登记的窗口（进程内所有插件共用一个表，所以按 id 找即可）。
    fn window_by_plugin_id(&self, window_id: &str) -> Option<OpenWindow> {
        self.app
            .try_state::<Arc<PluginWindows>>()
            .and_then(|windows| windows.by_plugin_window_id(&self.meta.id, window_id))
    }

    /// 把一条窗口事件交给插件线程的回调（走 `PluginCommand`）。
    fn deliver_notice(&self, notice: WindowNotice) -> Result<(), PluginError> {
        let Some(manager) = self.app.try_state::<PluginManager>() else {
            return Err(PluginError::Failed("插件管理器不可用".to_string()));
        };
        manager
            .deliver_window_notice(&self.meta.id, notice)
            .map_err(PluginError::Failed)
    }

    /// 把一条窗口请求送到主线程执行，并等回执。
    ///
    /// 与 [`TauriPluginHost::request_tray`] 的区别只有「谁在主线程上干活」：
    /// 窗口要建要拆，只能主线程做；超时上限也更宽一点。
    fn request_window(&self, request: WindowRequest) -> Result<(String, u32), PluginError> {
        let request_id = self.next_request.fetch_add(1, Ordering::SeqCst);
        let pending = self
            .app
            .state::<Arc<PendingWindowRequests>>()
            .inner()
            .clone();

        let rx = pending.register(request_id);
        let message = WindowRequestMessage {
            request_id,
            plugin_id: self.meta.id.clone(),
            request,
        };

        self.app
            .emit(WINDOW_REQUEST_EVENT, message)
            .map_err(|err| PluginError::Failed(format!("窗口请求发送失败：{err}")))?;

        match rx.recv_timeout(Duration::from_millis(WINDOW_TIMEOUT_MS)) {
            Ok(result) => match result.outcome {
                // 关窗的回执没有标签，插件侧也不需要
                Ok(Some((label, seq))) => Ok((label, seq)),
                Ok(None) => Ok((String::new(), 0)),
                Err(err) => Err(err),
            },
            Err(_) => {
                pending.forget(request_id);
                Err(PluginError::Failed(
                    "窗口操作超时（应用主线程没有响应）".to_string(),
                ))
            }
        }
    }
}

/// 在主线程上装好窗口请求的处理者（与 `tray::setup_plugin_events` 同一个手法）。
///
/// **主线程上绝不能同步等**：这条事件的处理器本身就在主线程运行，它若阻塞自己，
/// 就没人来处理这条事件了（必然死锁）。所以这里只做「建窗 / 拆窗 + 回执」。
pub fn setup_window_events(app: &AppHandle) {
    use tauri::Listener;

    let handle = app.clone();
    app.listen(WINDOW_REQUEST_EVENT, move |event| {
        let payload = event.payload();
        let Ok(message) = serde_json::from_str::<WindowRequestMessage>(payload) else {
            log::warn!("收到无法解析的窗口请求：{payload}");
            return;
        };
        let _ = handle_window_request(&handle, message);
    });
}

/// 处理一次窗口请求（**主线程**上调用）。
///
/// 建窗这一步只登记 + 建出真正的 webview 窗口；页面里的内容由前端 relay 决定，
/// 所以这里不需要知道插件页面长什么样。
pub fn handle_window_request(app: &AppHandle, message: WindowRequestMessage) -> WindowResponse {
    let pending = app.state::<Arc<PendingWindowRequests>>().inner().clone();
    let windows = app.state::<Arc<PluginWindows>>().inner().clone();

    let outcome = match &message.request {
        WindowRequest::Open { window_id, options } => {
            open_window(app, &windows, &message.plugin_id, window_id, options)
        }
        WindowRequest::Close { window_id } => {
            close_plugin_window(app, &windows, &message.plugin_id, window_id)
        }
    };

    let response = WindowResponse {
        request_id: message.request_id,
        outcome,
    };
    pending.resolve(WindowRequestResult {
        request_id: response.request_id,
        outcome: response.outcome.clone(),
    });
    response
}

/// 建一个插件窗口。
fn open_window(
    app: &AppHandle,
    windows: &PluginWindows,
    plugin_id: &str,
    plugin_window_id: &str,
    options: &clipbeam_plugins::WindowOptions,
) -> Result<Option<(String, u32)>, PluginError> {
    // 插件名用于缺省标题
    let plugin_name = app
        .try_state::<PluginManager>()
        .map(|manager| {
            manager
                .list()
                .into_iter()
                .find(|info| info.id == plugin_id)
                .map(|info| info.name)
                .unwrap_or_else(|| plugin_id.to_string())
        })
        .unwrap_or_else(|| plugin_id.to_string());

    let planned = crate::plugin_window::plan_window(
        &WindowRequest::Open {
            window_id: plugin_window_id.to_string(),
            options: options.clone(),
        },
        &plugin_name,
    )?;

    let crate::plugin_window::PlannedWindowAction::Open { window_id, config } = planned else {
        unreachable!("这里只可能是 Open");
    };

    // 先确认页面真的在插件目录里：否则窗口会打开成一片空白，
    // 而原因只出现在 webview 的 404 里 —— 插件作者看不到。
    let plugin_dir = app
        .try_state::<PluginManager>()
        .and_then(|manager| manager.plugin_dir(plugin_id));
    let Some(plugin_dir) = plugin_dir else {
        return Err(PluginError::Failed(format!(
            "找不到插件 {plugin_id:?} 的目录"
        )));
    };
    crate::plugin_window::resolve_plugin_file(&plugin_dir, &config.page)
        .map_err(PluginError::InvalidArgument)?;

    let window = windows.open(plugin_id, &window_id, &config.title, &config.page);
    let slot = window
        .label
        .rsplit('-')
        .next()
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(0);

    if let Err(err) = build_webview_window(app, &window, &config) {
        // 建失败要立刻把登记撤掉，否则会留下一个「在册但不存在」的窗口
        windows.remove(&window.label);
        return Err(PluginError::Failed(format!("创建窗口失败：{err}")));
    }

    log::info!(
        "插件 {plugin_id} 打开了窗口 {}（页面 {}）",
        window.label,
        config.page
    );
    Ok(Some((window.label, slot)))
}

/// 关一个插件窗口；返回 `Ok(None)` 表示「本来就不在」（幂等）。
fn close_plugin_window(
    app: &AppHandle,
    windows: &PluginWindows,
    plugin_id: &str,
    plugin_window_id: &str,
) -> Result<Option<(String, u32)>, PluginError> {
    let Some(window) = windows.by_plugin_window_id(plugin_id, plugin_window_id) else {
        // 幂等：插件可能在页面已经关掉之后才调 close
        return Ok(None);
    };

    if let Some(webview) = app.get_webview_window(&window.label) {
        let _ = webview.close();
    }
    windows.remove(&window.label);
    Ok(None)
}

/// 在**主线程**上建出 Tauri 窗口。
///
/// 窗口加载的是**应用自己的**一个路由（`index.html#/plugin-window?...`），
/// 而不是插件页面本身 —— 那个路由再把插件页面放进沙箱 iframe（见 `plugin_window.rs`）。
/// 这条链路让插件页面拿不到宿主的 IPC。
fn build_webview_window(
    app: &AppHandle,
    window: &OpenWindow,
    config: &crate::plugin_window::WindowConfig,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let url = crate::plugin_window::window_route_for(window);
    let mut builder = WebviewWindowBuilder::new(app, &window.label, WebviewUrl::App(url.into()))
        .title(&config.title)
        .inner_size(config.size.width as f64, config.size.height as f64)
        .resizable(config.resizable)
        .always_on_top(config.always_on_top)
        .decorations(config.decorations)
        .transparent(config.transparent);

    // `center()` 是「居中」这个动作本身，不接受布尔参数；插件没要求时就交给系统摆放
    if config.center {
        builder = builder.center();
    }

    builder.build().map(|_| ()).map_err(|err| err.to_string())
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

    fn window(&self, request: WindowRequest) -> Result<(String, u32), PluginError> {
        if self.stopped() {
            return Err(PluginError::Stopped);
        }
        self.request_window(request)
    }

    /// 把一个窗口事件交给插件线程上的回调。
    ///
    /// 两个方向共用这一个方法：
    ///
    /// * **页面 → 插件**：把消息交给插件登记的回调（`window:<id>:message`）；
    /// * **插件 → 页面**：本方法收到的是 `WindowNotice::Message`，它其实是**发给页面**的，
    ///   所以这里要按方向分派 —— 见下面的分支。
    fn notify_window_event(&self, notice: WindowNotice) -> Result<(), PluginError> {
        if self.stopped() {
            return Err(PluginError::Stopped);
        }

        match &notice {
            // 插件 → 页面：直接 emit 给那个窗口；窗口不在了就当作已关闭
            WindowNotice::Message { window_id, message } => {
                let Some(window) = self.window_by_plugin_id(window_id) else {
                    return Err(PluginError::Failed(format!(
                        "窗口 {window_id:?} 已经不在（可能已关闭）"
                    )));
                };
                self.app
                    .emit_to(&window.label, "plugin-window-message", message.clone())
                    .map_err(|err| {
                        // 窗口没了：告诉插件它已经关了 —— 这是插件清理回调的唯一时机
                        let _ = self.deliver_notice(WindowNotice::Closed {
                            window_id: window_id.clone(),
                        });
                        PluginError::Failed(format!("窗口已关闭（{err}）"))
                    })
            }
            // 宿主 → 插件：交给插件线程的回调
            WindowNotice::Closed { .. } => self.deliver_notice(notice),
        }
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

    /// 窗口登记表：标签可预测、按插件能整批摘掉、id 不重复。
    ///
    /// 这张表是「页面发来的消息该给谁」的唯一依据，所以它的行为要钉住。
    #[test]
    fn window_registry_tracks_labels_and_owners() {
        let windows = PluginWindows::default();
        assert!(windows.is_empty());

        let first = windows.open("plugin-a", "w1", "面板", "index.html");
        let second = windows.open("plugin-a", "w2", "面板 2", "ui/panel.html");
        let other = windows.open("plugin-b", "w1", "别的", "index.html");

        // 标签形如 plugin-window-<插件 id>-<序号>，且全局唯一
        assert_eq!(first.label, "plugin-window-plugin-a-1");
        assert_eq!(second.label, "plugin-window-plugin-a-2");
        assert_eq!(other.label, "plugin-window-plugin-b-3");
        assert_eq!(windows.labels().len(), 3, "三个窗口都该在册");

        // 两种查找方式
        assert_eq!(
            windows.by_label("plugin-window-plugin-a-2").unwrap().page,
            "ui/panel.html"
        );
        assert_eq!(
            windows.by_plugin_window_id("plugin-a", "w2").unwrap().label,
            "plugin-window-plugin-a-2"
        );
        assert!(windows.by_label("nope").is_none());
        assert!(
            windows.by_plugin_window_id("plugin-b", "w2").is_none(),
            "不同插件的同名窗口 id 不该互相命中"
        );

        // 整批摘掉某个插件的窗口：另一个插件不受影响
        let taken = windows.remove_all_of("plugin-a");
        assert_eq!(taken.len(), 2);
        assert_eq!(
            windows.labels(),
            vec!["plugin-window-plugin-b-3".to_string()]
        );
        assert!(
            windows.remove_all_of("plugin-a").is_empty(),
            "再摘一次应当是空的（幂等）"
        );

        // 单个摘除
        assert!(windows.remove("plugin-window-plugin-b-3").is_some());
        assert!(windows.remove("plugin-window-plugin-b-3").is_none());
        assert!(windows.is_empty());
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
