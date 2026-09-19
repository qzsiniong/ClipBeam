//! 脚本引擎（clipbeam-script）之上的 **ClipBeam 能力集**。
//!
//! # 分工
//!
//! | 层 | 负责 |
//! |---|---|
//! | `clipbeam-script`（引擎） | 跑 JS/TS、基础能力（`$.file` / `$.sleep` / `console` / 文本编解码）、扩展机制 |
//! | **本 crate**（使用方） | 把 ClipBeam 需要的能力以 `ScriptExtension` 注入：`md5` / `base32` / `zstd` / `typeStr` / `confirm`；脚本目录与内置示例；CLI 宿主 |
//! | `src-tauri`（应用） | 把引擎接到 Tauri 命令、Worker 任务、Typer 与确认弹窗上 |
//!
//! # 用法
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use clipbeam_script::{RuntimeOptions, ScriptRuntime};
//! use clipbeam_scripting::extensions;
//!
//! let runtime = ScriptRuntime::with_options(
//!     RuntimeOptions::default().extensions(extensions()),
//! )
//! .await?;
//! # let _ = runtime;
//! # Ok(()) }
//! ```
//!
//! # 新增一个能力
//!
//! 1. 在本 crate 里加一个模块，实现 `ScriptExtension`（`register` 挂函数、`spec` 声明签名）；
//! 2. 在 [`extensions`] 里加一行 `Arc::new(...)`；
//! 3. 在 `src/spec/clipbeam.d.ts` 里补上类型声明（三个真源，缺一个都会被测试或类型检查抓到）。

pub mod cmd;
pub mod compress;
pub mod hash;
pub mod scripts;
pub mod spec;
pub mod terminal;

use std::sync::Arc;

use clipbeam_script::ScriptExtension;

pub use clipbeam_script::ts;

pub use compress::ZstdExtension;
pub use hash::{Base32Extension, Md5Extension};
pub use spec::{capabilities, Capability};
pub use terminal::{ConfirmExtension, TypeStrExtension};

/// ClipBeam 的全部脚本能力（按注册顺序；`basic` 由引擎自己先注册）。
///
/// 顺序没有硬性依赖，但保持稳定可以让 `spec` 与运行期行为一致、也方便日志对照。
pub fn extensions() -> Vec<Arc<dyn ScriptExtension>> {
    vec![
        Arc::new(Md5Extension),
        Arc::new(Base32Extension),
        Arc::new(ZstdExtension),
        Arc::new(TypeStrExtension),
        Arc::new(ConfirmExtension),
    ]
}

/// 组装一份「带 ClipBeam 全部能力」的运行时配置。
///
/// 调用方再按需追加宿主与取消令牌：
///
/// ```no_run
/// # use std::sync::Arc;
/// # use clipbeam_script::RuntimeOptions;
/// # fn demo(host: Arc<dyn clipbeam_script::ScriptHost>) -> RuntimeOptions {
/// clipbeam_scripting::runtime_options().host(host)
/// # }
/// ```
pub fn runtime_options() -> clipbeam_script::RuntimeOptions {
    clipbeam_script::RuntimeOptions::default().extensions(extensions())
}
