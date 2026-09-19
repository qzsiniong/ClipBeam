//! 宿主接口：脚本与「外部世界」交互的三件事 —— 输出、确认、取消。
//!
//! 引擎本身不认识键盘、终端或 GUI：它只定义 [`ScriptHost`] 这个 trait，由使用方注入实现
//! （CLI 用终端 + enigo，GUI 用 Tauri 窗口 + Typer）。`$.typeStr` / `$.confirm` 这类能力
//! 并不属于 core，它们由使用方的 [`crate::ScriptExtension`] 实现并调用这里的接口。
//!
//! ## 传给 JS 的方式
//!
//! 宿主实现以 `Ctx::store_userdata` 存进上下文，能力实现用 [`crate::bindings::host_ctx`]
//! 取回。不往 JS 侧暴露任何对象：脚本永远看不到宿主句柄，只能通过具体能力间接使用它。

use std::sync::Arc;

use rquickjs::JsLifetime;

/// 确认类能力的回答。
///
/// * [`ConfirmChoice::Yes`] / [`ConfirmChoice::No`] 由调用方映射成布尔；
/// * [`ConfirmChoice::Abort`] 表示用户要求**终止脚本**，调用方应抛 JS 异常。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmChoice {
    /// 用户确认。
    Yes,
    /// 用户拒绝（也用于超时/无应答的兜底）。
    #[default]
    No,
    /// 用户要求中止脚本。
    Abort,
}

/// 宿主能力失败的原因。
///
/// 分类存在的原因是：上层要按类型给出不同文案（"已中止" / "已超时" / "当前环境不支持"），
/// 而不是把一句自由文本透传给用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// 任务被取消（用户按了中止键）。
    Cancelled,
    /// 等待用户回答超时。
    Timeout,
    /// 当前宿主不支持该能力（例如 headless 环境下的键盘输出）。
    Unsupported,
    /// 其它失败，附带可直接展示的说明。
    Failed(String),
}

impl HostError {
    /// 面向用户的中文说明（各能力可再加自己的前缀）。
    pub fn message(&self) -> String {
        match self {
            HostError::Cancelled => "脚本已中止".to_string(),
            HostError::Timeout => "等待用户确认超时".to_string(),
            HostError::Unsupported => "当前环境不支持该能力".to_string(),
            HostError::Failed(message) => message.clone(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for HostError {}

/// 脚本可以调用的宿主能力。
///
/// 实现必须是 `Send + Sync`：能力可能在任意线程被调用（GUI 里跑在 Worker 的阻塞线程上）。
pub trait ScriptHost: Send + Sync + 'static {
    /// 逐字符输出文本（`$.typeStr`）。
    ///
    /// `delay_ms` 是每个字符之间的间隔；实现应当在字符之间检查 [`ScriptHost::cancelled`]，
    /// 取消后返回 [`HostError::Cancelled`]。
    fn type_str(&self, text: &str, delay_ms: u64) -> Result<(), HostError>;

    /// 上报输出进度（已输出字符数 / 总字符数），供进度条展示。
    ///
    /// 默认空实现：headless 场景不需要进度。实现应当自行节流，调用可能非常频繁。
    fn progress(&self, _typed: usize, _total: usize) {}

    /// 向用户提问并等待回答（`$.confirm`）。
    ///
    /// 实现应当在**没有应答**时返回 [`ConfirmChoice::No`]（或 [`HostError::Timeout`]），
    /// 绝不能永久阻塞：脚本运行在 Worker 线程上。
    fn confirm(&self, message: &str) -> Result<ConfirmChoice, HostError>;

    /// 任务是否已被取消。
    fn cancelled(&self) -> bool {
        false
    }

    /// 脚本 `console.*` 输出的一行。
    ///
    /// `level` ∈ `{"log","info","debug","warn","error"}`；`text` 已由 `prelude.js`
    /// 格式化好（字符串原样，其余值走 `JSON.stringify`）。
    ///
    /// 默认实现写 stdout / stderr —— CLI、测试、headless 场景「不接 UI 也能看到输出」。
    /// GUI 宿主覆写它，把输出送进自己的日志面板。
    fn console(&self, level: &str, text: &str) {
        use std::io::Write;

        // 输出错误不值得打断脚本，一路忽略
        match level {
            "warn" | "error" => {
                let mut stderr = std::io::stderr();
                let _ = writeln!(stderr, "{text}");
                let _ = stderr.flush();
            }
            _ => {
                let mut stdout = std::io::stdout();
                let _ = writeln!(stdout, "{text}");
                let _ = stdout.flush();
            }
        }
    }
}

/// 什么也不做的宿主：输出/确认都不可用。
///
/// [`crate::RuntimeOptions::default`] 用它，因此「没注入宿主」的运行时行为是确定的
/// （能力报 `Unsupported`），而不是 panic。
pub struct NoopHost;

impl ScriptHost for NoopHost {
    fn type_str(&self, _text: &str, _delay_ms: u64) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
        Err(HostError::Unsupported)
    }
}

/// 上下文里保存的宿主句柄（`Ctx` userdata 的载体类型）。
///
/// 只装一个 `Arc`，不含任何 JS 值，因此可以安全地实现 [`JsLifetime`]。
pub(crate) struct HostRef(pub Arc<dyn ScriptHost>);

impl std::ops::Deref for HostRef {
    type Target = dyn ScriptHost;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

// SAFETY: `HostRef` 不含任何带 `'js` 生命周期的 JS 值，与上下文生命周期无关。
unsafe impl<'js> JsLifetime<'js> for HostRef {
    type Changed<'to> = HostRef;
}

/// 上下文里保存的取消令牌（`Ctx` userdata 的载体类型）。
pub(crate) struct CancelRef(pub CancellationToken);

// SAFETY: `CancelRef` 只是一个原子布尔，不含任何带 `'js` 生命周期的 JS 值。
unsafe impl<'js> JsLifetime<'js> for CancelRef {
    type Changed<'to> = CancelRef;
}

/// 协作式取消令牌。
///
/// 内置一个原子标志，也可以挂一个**外部判断函数**：使用方往往已经有自己的取消信号
/// （例如 ClipBeam 的 Esc 中止令牌），不必在脚本里再维护一份状态 ——
/// 「自己取消或被外部取消」都算取消。
#[derive(Clone, Default)]
pub struct CancellationToken {
    flag: Arc<std::sync::atomic::AtomicBool>,
    external: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl CancellationToken {
    /// 创建一个未取消的令牌。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建一个「同时受外部信号控制」的令牌。
    ///
    /// `external` 返回 `true` 即视为已取消；它与内置标志是**或**的关系。
    pub fn watching(external: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            external: Some(Arc::new(external)),
        }
    }

    /// 标记为已取消（幂等）。
    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否已取消（内置标志或外部信号任一为真）。
    pub fn is_cancelled(&self) -> bool {
        if self.flag.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        self.external.as_ref().is_some_and(|external| external())
    }

    /// 建一个与 `self` 共享状态的句柄（取消任一即全部取消）。
    pub fn handle(&self) -> Self {
        self.clone()
    }
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}
