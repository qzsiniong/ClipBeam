//! # clipbeam-plugins —— ClipBeam 的插件运行框架
//!
//! 在 [`script_engine`]（可嵌入的 QuickJS 引擎）之上，加一层**常驻扩展机制**：
//! 插件 = 一个目录（`plugin.json` 清单 + JS/TS 入口），由宿主发现、启停、授权，
//! 并通过 `$plugin` 能力把功能伸进宿主界面。
//!
//! ## 与「脚本」的分工
//!
//! | | 脚本（`clipbeam-scripting`） | 插件（本 crate） |
//! |---|---|---|
//! | 触发 | 用户点「运行」 | 托盘菜单 / 事件（常驻） |
//! | 生命周期 | 跑完即结束 | 常驻到被禁用 |
//! | 命名空间 | `ClipBeam` / `$` | [`NAMESPACE`] / [`NAMESPACE_ALIAS`] |
//! | 宿主接口 | `ScriptHost`（键盘注入、待命窗口…） | [`PluginHost`]（反馈、窗口、托盘…） |
//!
//! 两者**共用同一个引擎**，但能力集、命名空间与宿主接口各自独立：
//! 插件里没有「往当前焦点窗口打字」这种脚本专属语义，脚本里也不会有「创建窗口」。
//!
//! ## 分层
//!
//! ```text
//! 宿主应用（src-tauri）
//!   ├── 实现 PluginHost ⇒ 把反馈/托盘落到真实 UI
//!   ├── 管理插件线程：启用 / 停用 / 派发动作
//!   └── 读 catalog 渲染插件列表
//!        ▲
//! clipbeam-plugins（本 crate，不依赖 Tauri）
//!   ├── manifest / id / catalog   目录与清单
//!   ├── permission                声明式权限（未声明即拒绝）
//!   ├── host / context            宿主接口与上下文
//!   ├── extensions/$plugin        能力本体（toast / notify / alert / confirm / tray）
//!   └── runtime                   组装引擎运行时、跑入口、派发动作
//!        ▲
//! script-engine（引擎：跑 JS/TS、标准全局、扩展机制）
//! ```
//!
//! ## 快速开始
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use clipbeam_plugins::{PluginHost, PluginMeta, PluginRuntimeOptions};
//!
//! # fn host(meta: PluginMeta) -> Arc<dyn PluginHost> { unimplemented!() }
//! let meta = PluginMeta {
//!     id: "hello".into(),
//!     name: "示例".into(),
//!     version: "0.1.0".into(),
//!     description: None,
//!     author: None,
//!     dir: "/tmp/hello".into(),
//!     entry: "index.js".into(),
//! };
//!
//! let runtime = clipbeam_plugins::create_runtime(
//!     host(meta.clone()),
//!     PluginRuntimeOptions::new(meta),
//! )
//! # ;
//! # let _ = runtime;
//! # Ok(()) }
//! ```
//!
//! ## 新增一个 `$plugin` 能力
//!
//! 1. 在 `src/extensions/` 里加实现；直接挂在命名空间上的用 `extension!` 宏写，
//!    挂在嵌套对象上的（如 `$plugin.tray.*`）手写 `register`；
//! 2. 在 [`extensions::extensions`] 里加一行；
//! 3. 在 [`permission`] 里确认它属于哪一组权限（新能力通常要加一组）；
//! 4. 同步 `src/spec/plugins.d.ts` 的类型声明（`tests/spec_sync.rs` 会校验不漂移）。

#![warn(missing_docs)]

pub mod catalog;
pub mod context;
pub mod declarations;
pub mod error;
pub mod extensions;
pub mod host;
pub mod id;
pub mod manifest;
pub mod permission;
pub mod runtime;
pub mod seed;
pub mod spec;

#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

pub use error::PluginError;
pub use host::{
    ActionRegistry, DialogButtons, DialogChoice, Feedback, FeedbackOutcome, PluginHost, PluginMeta,
    ToastLevel, TrayOutcome, TrayRequest, WindowNotice, WindowOptions, WindowRequest,
    WindowRequestMessage, WindowResponse,
};
pub use manifest::{PluginManifest, PluginMenuItem};
pub use permission::{Permission, PermissionSet};
pub use runtime::{create_runtime, runtime_options, PluginRuntime, PluginRuntimeOptions};
pub use spec::{capabilities, capability_list, PluginCapability, PluginCapabilityList};

/// 插件能力命名空间的正式名。
///
/// **这是「引擎的匿名命名空间叫什么」的唯一出处**：配置给引擎（[`runtime_options`]）、
/// 声明给前端（`src/spec/plugins.d.ts`）也用同一对常量。
/// 引擎自己不含任何业务名字。
pub const NAMESPACE: &str = "ClipBeamPlugin";

/// 插件能力命名空间的别名（与 [`NAMESPACE`] 指向同一个对象；写起来更短）。
pub const NAMESPACE_ALIAS: &str = "$plugin";

#[cfg(test)]
mod tests {
    use super::*;

    /// 命名空间与脚本的（`ClipBeam` / `$`）必须不同：两套语义不能共用名字。
    #[test]
    fn namespace_differs_from_the_script_one() {
        assert_ne!(NAMESPACE, "ClipBeam");
        assert_ne!(NAMESPACE_ALIAS, "$");
        assert_eq!(NAMESPACE_ALIAS, "$plugin");
    }
}
