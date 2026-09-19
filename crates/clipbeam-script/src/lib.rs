//! # clipbeam-script
//!
//! ClipBeam 的**嵌入式 QuickJS 脚本引擎**：跑 JavaScript / TypeScript，并把能力交给使用方决定。
//!
//! ## 分层
//!
//! ```text
//! 使用方（例：ClipBeam 的 clipbeam-scripting crate）
//!   ├── ScriptExtension ⇒ 注入业务能力（md5 / zstd / 键盘输出 / 确认 …）
//!   └── ScriptHost      ⇒ 注入宿主交互（逐字输出、提问、取消）
//!        ▲
//! clipbeam-script（本 crate，只做通用的事）
//!   ├── ScriptRuntime / RuntimeOptions   引擎与上下文生命周期、脚本执行（支持顶层 await）
//!   ├── ts                                TypeScript → JavaScript（oxc，进程内，不依赖 tsc）
//!   ├── extension                         ScriptExtension / CapabilitySpec / 注册顺序与重名检查
//!   ├── host                              ScriptHost / HostError / ConfirmChoice / CancellationToken
//!   ├── prelude.js                        用 JS 补齐 TextDecoder / TextEncoder / console
//!   └── bindings/                         core 内置能力：$.file / $.sleep（+ 文本编解码、console 原语）
//! ```
//!
//! 依赖方向是单向的：`runtime` → `extension` → `bindings`；`bindings` 之间互不依赖，
//! 需要复用的公共逻辑（例如把 `ArrayBuffer` 拷成 `Vec<u8>`）放在 [`bindings`] 的模块根上。
//!
//! ## core 内置哪些能力
//!
//! | 内置 | 说明 |
//! |---|---|
//! | `$.file(path)` | 读真实文件，`Promise<ArrayBuffer>`（相对路径按进程工作目录解析） |
//! | `$.sleep(ms)` | 异步等待，可被 [`CancellationToken`] 取消 |
//! | `console` | `log` / `info` / `debug` → stdout，`warn` / `error` → stderr |
//! | `TextDecoder` / `TextEncoder` | 全部 WHATWG 编码标签（`gbk` / `gb18030` / `shift_jis` …） |
//!
//! **不内置**任何业务能力：摘要、压缩、键盘输出、确认都由使用方以 [`ScriptExtension`] 提供。
//!
//! ## 快速开始
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use clipbeam_script::{RuntimeOptions, ScriptRuntime};
//!
//! // 创建运行时；脚本里可以直接使用顶层 await
//! let runtime = ScriptRuntime::with_options(RuntimeOptions::default()).await?;
//! runtime
//!     .run_script("const bytes = await $.file(\"Cargo.toml\"); console.log(bytes.byteLength)")
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
//! use clipbeam_script::{CapabilitySpec, ScriptExtension};
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
//!   脚本名与行列号，例如 `Error: clipbeam-demo.js:5:21 ...`；
//! * 取消（[`CancellationToken`]）→ `$.sleep` 静默提前返回；能力动作用
//!   [`bindings::throw_cancelled`] 抛「脚本已中止」。

#![warn(missing_docs)]

pub mod bindings;
pub mod extension;
pub mod host;
pub mod runtime;
pub mod ts;

pub use extension::{BasicExtension, CapabilitySpec, ScriptExtension, NAMESPACE, NAMESPACE_ALIAS};
pub use host::{CancellationToken, ConfirmChoice, HostError, NoopHost, ScriptHost};
pub use runtime::{RuntimeOptions, ScriptRuntime};
