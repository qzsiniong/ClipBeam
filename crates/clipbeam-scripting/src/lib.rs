//! 脚本引擎（`script-engine`）之上的 **ClipBeam 能力集**。
//!
//! # 分工
//!
//! | 层 | 负责 |
//! |---|---|
//! | `script-engine`（引擎） | 跑 JS/TS（含顶层 await）、`console`、文本编解码、扩展机制、标准全局（`sleep` 等；不含任何业务名字） |
//! | **本 crate**（使用方） | **决定能力命名空间叫什么**（`ClipBeam` / `$`，见 [`NAMESPACE`]）；把 ClipBeam 需要的能力以 `ScriptExtension` 注入：数据形态与编解码、摘要、压缩、文件系统、宿主交互；`ScriptHost` 定义；脚本目录与内置示例；CLI 宿主 |
//! | `src-tauri`（应用） | 把引擎接到 Tauri 命令、Worker 任务、Typer、确认弹窗与文件授权弹窗上 |
//!
//! # 用法
//!
//! ```no_run
//! # async fn demo(host: std::sync::Arc<dyn clipbeam_scripting::ScriptHost>) -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use script_engine::{ConsoleHook, ScriptRuntime, StdoutConsole};
//!
//! let console: Arc<dyn ConsoleHook> = Arc::new(StdoutConsole);
//! let runtime = ScriptRuntime::with_options(clipbeam_scripting::runtime_options(host, console))
//!     .await?;
//! # let _ = runtime;
//! # Ok(()) }
//! ```
//!
//! # 新增一个能力
//!
//! 1. 在 `src/extensions/` 里加一个文件，用 `extension!` 宏同时声明「挂哪些函数」与签名；
//! 2. 在 [`extensions::extensions`] 里加一行 `Arc::new(...)`；
//! 3. 在 `src/spec/clipbeam.d.ts` 里补上类型声明（三个真源，缺一个都会被测试或类型检查抓到）。

pub mod cmd;
pub mod extensions;
pub mod host;
pub mod scripts;
pub mod spec;

use std::sync::Arc;

use script_engine::{ConsoleHook, RuntimeOptions, ScriptRuntime};

pub use extensions::extensions;
pub use host::{ConfirmChoice, FileDecision, HostError, NoopHost, ScriptHost};
pub use script_engine::ts;
pub use spec::{capabilities, capability_list, Capability, CapabilityList};

/// 能力命名空间的正式名。
///
/// **这是「引擎的匿名命名空间叫什么」的唯一出处**：配置给引擎（[`runtime_options`]）、
/// 声明给前端（[`capability_list`]）都用它。引擎自己不含任何业务名字。
pub const NAMESPACE: &str = "ClipBeam";

/// 能力命名空间的别名（与 [`NAMESPACE`] 指向同一个对象）。
pub const NAMESPACE_ALIAS: &str = "$";

/// 组装一份「带 ClipBeam 全部能力 + 指定宿主与 console 落点」的运行时配置。
///
/// * `host` 决定 `$.type_str` / `$.confirm` / 文件授权问到哪儿（CLI 是终端，GUI 是窗口）；
/// * `console` 决定 `console.*` 的落点（CLI 用 [`script_engine::StdoutConsole`]，
///   GUI 用面板缓冲）—— 它属于引擎的 [`ConsoleHook`]，与 `host` 是两个独立的东西；
/// * 能力命名空间挂成 [`NAMESPACE`] / [`NAMESPACE_ALIAS`]（名字只在本 crate 里写一次）。
///
/// ```no_run
/// # use std::sync::Arc;
/// # use script_engine::{ConsoleHook, RuntimeOptions, StdoutConsole};
/// # fn demo(host: Arc<dyn clipbeam_scripting::ScriptHost>) -> RuntimeOptions {
/// let console: Arc<dyn ConsoleHook> = Arc::new(StdoutConsole);
/// clipbeam_scripting::runtime_options(host, console)
/// # }
/// ```
pub fn runtime_options(host: Arc<dyn ScriptHost>, console: Arc<dyn ConsoleHook>) -> RuntimeOptions {
    // 宿主句柄通过 prepare 钩子存进上下文：引擎不认识「宿主」这个概念，
    // 它只提供「扩展注册前让我做点初始化」的入口。
    let host_for_ctx = host.clone();
    RuntimeOptions::default()
        .extensions(extensions())
        .console(console)
        .namespace(NAMESPACE)
        .namespace_alias(NAMESPACE_ALIAS)
        .prepare(Arc::new(move |ctx| {
            host::store_host(ctx, host_for_ctx.clone())
        }))
}

/// 创建一份「带 ClipBeam 全部能力」的运行时。
///
/// ```no_run
/// # async fn demo(host: std::sync::Arc<dyn clipbeam_scripting::ScriptHost>) -> anyhow::Result<()> {
/// # use std::sync::Arc;
/// # use script_engine::{ConsoleHook, StdoutConsole};
/// # let cancel = script_engine::CancelSignal::new();
/// let console: Arc<dyn ConsoleHook> = Arc::new(StdoutConsole);
/// let runtime = clipbeam_scripting::create_runtime(host, console, Some(cancel)).await?;
/// # let _ = runtime;
/// # Ok(()) }
/// ```
pub async fn create_runtime(
    host: Arc<dyn ScriptHost>,
    console: Arc<dyn ConsoleHook>,
    cancel: Option<script_engine::CancelSignal>,
) -> anyhow::Result<ScriptRuntime> {
    let mut options = runtime_options(host, console);
    if let Some(cancel) = cancel {
        options = options.cancel(cancel);
    }
    ScriptRuntime::with_options(options).await
}
