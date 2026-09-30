//! `$plugin.*` 能力扩展：把宿主能力挂到插件命名空间上。
//!
//! 每个能力一个文件（见子模块），本模块只做两件事：
//!
//! 1. 声明 [`extension!`] 宏 —— 把「注册若干个 JS 函数」+「声明它们的签名」这两件
//!    必须同步的事写在一处（漏掉一边编译就过不去）；
//! 2. [`extensions`] 汇总全部能力，供 [`crate::runtime_options`] 使用。
//!
//! # 每个能力实现的第一行
//!
//! 一律是 [`crate::context::require_permission`]：**先鉴权，再干活**。
//! 它同时完成「取回插件上下文」与「检查权限声明」两件事，缺上下文（运行时没按插件方式
//! 组装）与缺权限都会抛出带完整说明的异常。
//!
//! # 错误文案只有一处
//!
//! 宿主错误 → JS 异常统一走 [`crate::context::throw_plugin_error`]，
//! 于是「权限不够」「参数不合法」「环境不支持」的说法不会在多个文件里各写一份。

use std::sync::Arc;

use script_engine::ScriptExtension;

pub mod feedback;
pub mod tray;
pub mod window;

/// 声明一个能力扩展：一次写清「挂哪些函数」与「签名是什么」。
///
/// ```ignore
/// extension! {
///     /// `$plugin.toast(message)`：应用内提示。
///     pub struct ToastExtension;
///     "toast" => js_toast,
///         "toast(message: string, options?: ToastOptions) -> void",
///         "在应用窗口里弹一条提示";
/// }
/// ```
///
/// * 函数名前的 `async` 关键字表示该函数是 `async fn`（会用 `Async` 包成返回 Promise
///   的 JS 函数）；不写就是同步函数；
/// * 同步/异步的取舍是**契约的一部分**：`toast` 不阻塞（同步返回 `void`），
///   `confirm` 要等用户回答（async，返回 Promise）。
macro_rules! extension {
    (
        $(#[$attr:meta])*
        pub struct $ext:ident;
        $(
            $($kind:ident)? $target:literal => $func:path, $signature:literal, $doc:literal;
        )*
    ) => {
        $(#[$attr])*
        pub struct $ext;

        impl script_engine::ScriptExtension for $ext {
            fn register<'js>(
                &self,
                ctx: &rquickjs::Ctx<'js>,
                ns: &rquickjs::Object<'js>,
            ) -> rquickjs::Result<()> {
                $(
                    $crate::extensions::extension!(@bind ns, ctx, $target, $func, $($kind)?);
                )*
                Ok(())
            }

            fn spec(&self) -> Vec<script_engine::CapabilitySpec> {
                vec![
                    $(
                        script_engine::CapabilitySpec {
                            name: $target,
                            signature: $signature,
                            doc: $doc,
                        },
                    )*
                ]
            }
        }
    };

    (@bind $ns:ident, $ctx:ident, $target:literal, $func:path,) => {
        $ns.set($target, rquickjs::Function::new($ctx.clone(), $func)?)?;
    };

    (@bind $ns:ident, $ctx:ident, $target:literal, $func:path, async) => {
        $ns.set(
            $target,
            rquickjs::Function::new($ctx.clone(), rquickjs::prelude::Async($func))?,
        )?;
    };
}

// 子模块通过 `use crate::extensions::extension;` 使用这个宏（macro_rules 必须先定义再导出）
pub(crate) use extension;

/// `$plugin` 的全部能力（按注册顺序）。
///
/// 顺序没有硬性依赖，但保持稳定可以让 `spec`、文档与运行期行为一致。
pub fn extensions() -> Vec<Arc<dyn ScriptExtension>> {
    vec![
        // 反馈机制：应用内提示 / 系统通知 / 系统对话框
        Arc::new(feedback::ToastExtension),
        Arc::new(feedback::NotifyExtension),
        Arc::new(feedback::AlertExtension),
        Arc::new(feedback::ConfirmExtension),
        // 托盘：动作回调登记 + 运行时控制（挂点是嵌套对象 `tray`）
        Arc::new(tray::TrayExtension),
        // 窗口：开窗 / 收发消息 / 关窗（挂点是嵌套对象 `window`）
        Arc::new(window::WindowExtension),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 能力名在本命名空间内不能重复（引擎也会查，这里提前给出更清楚的失败信息）。
    #[test]
    fn capability_names_are_unique() {
        let mut names: Vec<&str> = Vec::new();
        for extension in extensions() {
            for spec in extension.spec() {
                assert!(
                    !names.contains(&spec.name),
                    "能力名重复：{}（{names:?}）",
                    spec.name
                );
                names.push(spec.name);
            }
        }
        assert!(names.contains(&"toast"));
        assert!(names.contains(&"confirm"));
        assert!(
            names.contains(&"tray.onAction"),
            "嵌套能力用带点路径：{names:?}"
        );
    }

    /// 每条能力都要有签名与说明（它们同时是文档与补全数据源）。
    #[test]
    fn specs_are_documented() {
        for extension in extensions() {
            for spec in extension.spec() {
                assert!(!spec.doc.is_empty(), "{} 缺少说明", spec.name);
                // 签名里写的是**成员名**（`onAction(...)`），能力名可能是带点的路径
                // （`tray.onAction`）—— 取最后一段来比对。
                let member = spec.name.rsplit('.').next().unwrap_or(spec.name);
                assert!(
                    spec.signature.starts_with(&format!("{member}(")),
                    "签名应当以 `{member}(` 开头：{} / {}",
                    spec.name,
                    spec.signature
                );
            }
        }
    }

    /// 嵌套能力必须同时给出 `spec`（带点路径）与 `register_nested`（挂点声明）。
    ///
    /// 少任一半，引擎的一致性校验要么误报、要么形同虚设。
    #[test]
    fn nested_capabilities_declare_their_mount_point() {
        let nested = tray::TrayExtension.register_nested();
        assert_eq!(nested.len(), 1, "托盘只有一个嵌套挂点：{nested:?}");
        assert_eq!(nested[0].0, "tray");

        let specs = tray::TrayExtension.spec();
        for spec in &specs {
            let (path, member) = spec
                .name
                .split_once('.')
                .unwrap_or_else(|| panic!("嵌套能力名应当带点：{}", spec.name));
            assert_eq!(path, nested[0].0, "挂点路径应当与 register_nested 一致");
            assert!(
                nested[0].1.iter().any(|declared| declared == member),
                "{} 没在 register_nested 里声明",
                spec.name
            );
        }
    }
}
