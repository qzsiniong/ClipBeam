//! # script-engine
//!
//! 可嵌入的 **QuickJS 脚本引擎**：跑 JavaScript / TypeScript，并把能力交给使用方决定。
//!
//! ## 分层
//!
//! ```text
//! 使用方（宿主应用的能力集 crate）
//!   ├── ScriptExtension ⇒ 注入业务能力（md5 / zstd / 键盘输出 / 确认 …）
//!   └── RuntimeOptions::namespace ⇒ 给能力命名空间起名字（引擎只建匿名对象）
//!   └── ConsoleHook    ⇒ 注入 console.* 的落点（不做宿主交互）
//!        ▲
//! script-engine（本 crate，只做通用的事）
//!   ├── ScriptRuntime / RuntimeOptions   引擎与上下文生命周期、脚本执行（支持顶层 await）
//!   ├── ts                                TypeScript → JavaScript（oxc，进程内，不依赖 tsc）
//!   ├── extension                         ScriptExtension / CapabilitySpec / 注册顺序与重名检查
//!   ├── cancel                            CancelSignal（协作式取消）
//!   ├── console_hook                      ConsoleHook / StdoutConsole / NoopConsole
//!   ├── prelude.js                        用 JS 补齐 TextDecoder / TextEncoder / console
//!   └── bindings/                         标准全局（sleep、atob/btoa、定时器…）与文本编解码
//! ```
//!
//! 依赖方向是单向的：`runtime` → `extension` → `bindings`；`bindings` 之间互不依赖，
//! 需要复用的公共逻辑（例如把 `ArrayBuffer` 拷成 `Vec<u8>`）放在 [`bindings`] 的模块根上。
//!
//! ## 引擎提供什么
//!
//! 引擎**不提供任何「能力」**，也**不含任何业务名字**：
//!
//! | 类别 | 内容 |
//! |---|---|
//! | 标准全局 | `sleep(ms)`（**要 `await`**）、`setTimeout` / `setInterval` / `clearTimeout` / `clearInterval`、`atob` / `btoa`、`performance.now()`、`structuredClone`、`TextDecoder` / `TextEncoder`、`console` |
//! | Rust 侧设施 | [`ScriptExtension`] 扩展机制、[`ConsoleHook`]、[`CancelSignal`]、等待实现 [`bindings::timer::sleep`] |
//! | 命名空间 | 引擎建一个**匿名**对象，名字由 [`RuntimeOptions::namespace`] 决定（默认不挂全局） |
//! | `TextDecoder` / `TextEncoder` | 全部 WHATWG 编码标签（`gbk` / `gb18030` / `shift_jis` …） |
//! | `atob` / `btoa` | WHATWG 语义的 base64（只处理 Latin-1 字符串） |
//! | `setTimeout` / `clearTimeout` / `setInterval` / `clearInterval` | 定时器，脚本结束时统一清理 |
//! | `performance.now()` | 单调时钟（毫秒） |
//! | `structuredClone` | JSON 语义深拷贝（不支持 `Date`/`Map`/循环引用，会明确报错） |
//!
//! 判据是「**引擎补规范，但不发明业务名字**」：`setTimeout` / `atob` / `TextDecoder` 是
//! 规范里的名字，引擎补上名副其实；能力命名空间叫 `MyTool` 还是别的，由使用方配置。
//!
//! **不内置**任何业务能力：文件读写、摘要、压缩、键盘输出、确认都由使用方以
//! [`ScriptExtension`] 提供 —— 它们要么需要业务库，要么需要宿主。
//!
//! ## 快速开始
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use script_engine::{RuntimeOptions, ScriptRuntime};
//!
//! // 使用方决定能力命名空间叫什么（不配就不挂全局）
//! let options = RuntimeOptions::default()
//!     .namespace("MyTool")
//!     .namespace_alias("$");
//! let runtime = ScriptRuntime::with_options(options).await?;
//! // 脚本里可以直接使用顶层 await；sleep 是引擎提供的全局，必须 await
//! runtime
//!     .run_script("await sleep(50); console.log('done')")
//!     .await?;
//! # Ok(()) }
//! ```
//!
//! ## 如何新增一个能力
//!
//! 1. 实现 [`ScriptExtension`]：在 `register` 里把能力挂到命名空间对象 `ns` 上，
//!    并用 `spec()` 声明它的名字与签名（这份声明同时喂给 GUI 的补全）；
//! 2. 通过 [`RuntimeOptions::extension`] / [`RuntimeOptions::extensions`] 传入。
//!
//! 能力本体写成把 `Ctx<'js>` 放在**参数表第一位**的普通函数（`rquickjs` 会自动注入），
//! **不要**捕获 `Ctx` 克隆 —— 见 [`extension`] 模块文档里的 GC 引用环说明。
//!
//! ```ignore
//! use std::sync::Arc;
//! use script_engine::{CapabilitySpec, ScriptExtension};
//! use rquickjs::{Ctx, Function, Object, Result as QjsResult};
//!
//! struct Hello;
//!
//! impl ScriptExtension for Hello {
//!     fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
//!         ns.set("hello", Function::new(ctx.clone(), js_hello)?)
//!     }
//!     fn spec(&self) -> Vec<CapabilitySpec> {
//!         vec![CapabilitySpec { name: "hello", signature: "hello() -> string", doc: "打个招呼" }]
//!     }
//! }
//!
//! fn js_hello() -> String { "hello".to_string() }
//! # let _ = Arc::new(Hello);
//! ```
//!
//! ## 错误处理约定
//!
//! * 宿主函数的错误 → 转成 JS 异常（同步函数）或被拒绝的 `Promise`（异步函数），
//!   JS 侧可以 `try/catch` 或 `.catch()` 捕获；
//! * 脚本里的异常 → [`ScriptRuntime::run_script`] 返回 `Err`，错误信息里带
//!   脚本名与行列号，例如 `Error: demo.js:5:21 ...`；
//! * 取消（[`CancelSignal`]）→ `sleep` 静默提前返回；能力动作用
//!   [`bindings::throw_cancelled`] 抛「脚本已中止」。

#![warn(missing_docs)]

pub mod bindings;
pub mod cancel;
pub mod console_hook;
pub mod extension;
pub mod runtime;
pub mod ts;

pub use cancel::CancelSignal;
pub use console_hook::{ConsoleHook, NoopConsole, StdoutConsole};
pub use extension::{CapabilitySpec, ScriptExtension};
pub use runtime::{ContextSetup, RuntimeOptions, ScriptRuntime};
