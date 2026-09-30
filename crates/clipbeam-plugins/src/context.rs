//! 插件上下文：宿主句柄 + 身份 + 权限 + 回调订阅表，以 `Ctx::userdata` 存进 JS 上下文。
//!
//! # 为什么走 userdata
//!
//! 与 `clipbeam-scripting` 的 `HostRef` 同一个手法：**不往 JS 侧暴露任何宿主对象**。
//! 脚本/插件永远看不到宿主句柄，只能通过具体能力间接使用它 —— 于是「插件能做什么」
//! 完全由能力清单 + 权限决定，而不是「它拿到了一个对象随便调」。
//!
//! # 为什么是一整个结构而不是分开存
//!
//! 能力实现需要一次拿齐「宿主、插件 id、权限、订阅表」。分开存意味着每个能力都要
//! 写一遍「取四次 + 其中某次缺失时报错」，而且容易漏。合成一个结构后只有一次查找
//! （[`require_context`]），拿不到就是接线错误，文案只有一处。

use std::sync::{Arc, Mutex};

use rquickjs::{Ctx, JsLifetime, Result as QjsResult};

use crate::error::PluginError;
use crate::host::{ActionRegistry, PluginHost, PluginMeta};
use crate::permission::{Permission, PermissionSet};

/// 插件运行期的共享状态。
#[derive(Default)]
pub struct PluginShared {
    /// 已声明的动作表（`menus[].id` 或接口级 key → **全局函数名**）。
    actions: ActionRegistry,
    /// 动作回调的全局函数名自增序号。
    next_action: u64,
    /// 插件内窗口 id 的自增序号（`w1` / `w2`…）。
    next_window: u64,
}

impl PluginShared {
    /// 登记一个动作回调，返回它的**全局函数名**。
    ///
    /// 调用方负责把这个名字挂成一个全局函数（`globalThis[name] = callback`）——
    /// 宿主就是按这个名字唤醒回调的（`ScriptRuntime::eval_global`）。
    ///
    /// # 为什么不在这里直接持有那个函数（`Persistent`）
    ///
    /// 曾经这么做过，结果是**进程级泄漏**：rquickjs 0.14 的 `Persistent::save` 会让被保存的
    /// JS 对象留在 `gc_obj_list` 里，运行时析构时 `JS_FreeRuntime` 直接 abort
    /// （实测：只要存一个函数就复现）。改存名字之后：
    ///
    /// * 回调本身由它在全局对象上的绑定持有（脚本自己删掉它就等于注销，是合理语义）；
    /// * 派发通道本来就只需要一个名字，少了一层 `unsafe`（原先要为一个 `!Send` 的容器
    ///   手写 `Send`/`Sync`）。
    ///
    /// 将来要支持「按 id 注销单个回调」时，仍然按名字注销即可。
    pub fn register_action(&mut self, key: &str) -> String {
        self.next_action += 1;
        let global_name = format!("__plugin_action_{}", self.next_action);
        self.actions.register(key, &global_name);
        global_name
    }

    /// 下一个插件内窗口 id 的序号。
    ///
    /// 由宿主分配而不是让插件自己造：两个窗口重名会让它们的回调互相覆盖，
    /// 而插件未必意识到这件事。
    pub fn next_window_id(&mut self) -> u64 {
        self.next_window += 1;
        self.next_window
    }

    /// 动作表（宿主在插件启用后读它，用来知道该建哪些托盘菜单项）。
    pub fn actions(&self) -> &ActionRegistry {
        &self.actions
    }
}

/// 上下文里保存的插件状态（`Ctx` userdata 的载体类型）。
pub struct PluginContext {
    /// 宿主句柄（`Arc` 共享给每次能力调用）。
    pub host: Arc<dyn PluginHost>,
    /// 身份（`host.meta()` 的副本，避免每次都要过一层 trait 调用）。
    pub meta: PluginMeta,
    /// 声明的权限（未声明的调用会被拒绝）。
    pub permissions: PermissionSet,
    /// 共享状态（回调、动作表）。
    pub shared: Arc<Mutex<PluginShared>>,
}

// SAFETY: `PluginContext` 本身不含任何带 `'js` 生命周期的 JS 值
// （回调存在 `PluginShared` 里，形态是 `Persistent`，与上下文生命周期解耦）。
unsafe impl<'js> JsLifetime<'js> for PluginContext {
    type Changed<'to> = PluginContext;
}

/// 上下文里保存的插件状态（带 `Arc`，便于在能力实现里长期持有）。
pub(crate) struct PluginCtxRef(pub Arc<PluginContext>);

// SAFETY: 同 `PluginContext`。
unsafe impl<'js> JsLifetime<'js> for PluginCtxRef {
    type Changed<'to> = PluginCtxRef;
}

/// 把插件状态存进上下文（由 `RuntimeOptions::prepare` 钩子调用，早于所有扩展注册）。
pub(crate) fn store_context<'js>(
    ctx: &Ctx<'js>,
    context: PluginContext,
) -> QjsResult<Arc<PluginContext>> {
    let shared = Arc::new(context);
    ctx.store_userdata(PluginCtxRef(shared.clone()))
        .map_err(|_| {
            rquickjs::Exception::throw_message(ctx, "插件上下文存入失败（userdata 正被借用）")
        })?;
    Ok(shared)
}

/// 取回插件状态（克隆一个 `Arc`，调用方拿到所有权）。
pub fn context<'js>(ctx: &Ctx<'js>) -> Option<Arc<PluginContext>> {
    ctx.userdata::<PluginCtxRef>().map(|guard| guard.0.clone())
}

/// 取回插件状态，拿不到就给一条**指向接线问题**的错误（而不是抛个莫名的异常）。
///
/// 拿不到只可能是「运行时没用 [`crate::runtime_options`] 建」这一种情况，
/// 所以文案直接说清怎么修。
pub fn require_context<'js>(ctx: &Ctx<'js>) -> QjsResult<Arc<PluginContext>> {
    context(ctx).ok_or_else(|| {
        rquickjs::Exception::throw_message(
            ctx,
            "插件能力不可用：当前运行环境没有注入插件上下文\
             （请用 clipbeam_plugins::runtime_options 建运行时）",
        )
    })
}

/// 取回插件状态并检查权限，一步到位。
///
/// 这是每个能力实现的第一行：**先鉴权，再干活**。
pub fn require_permission<'js>(
    ctx: &Ctx<'js>,
    permission: Permission,
    capability: &str,
) -> QjsResult<Arc<PluginContext>> {
    let context = require_context(ctx)?;
    context
        .permissions
        .require(permission, &context.meta.id, capability)
        .map_err(|err| throw_plugin_error(ctx, err))?;
    Ok(context)
}

/// 宿主错误 → JS 异常。
///
/// 集中在一处：这样「权限不够」「参数不合法」「已停用」「环境不支持」的文案
/// 只有一份，改了不会漂移。
pub fn throw_plugin_error<'js>(ctx: &Ctx<'js>, err: PluginError) -> rquickjs::Error {
    rquickjs::Exception::throw_message(ctx, &err.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_state_starts_empty() {
        let shared = PluginShared::default();
        assert!(shared.actions().is_empty());
    }

    /// 每次登记都给出**不同的**全局函数名（同一个名字会互相覆盖回调）。
    #[test]
    fn action_names_are_unique_and_registered() {
        let mut shared = PluginShared::default();
        let first = shared.register_action(crate::host::GENERIC_ACTION_KEY);
        let second = shared.register_action("hello");

        assert_ne!(first, second, "两次登记不能重名");
        assert_eq!(
            shared.actions().global_name("hello").as_deref(),
            Some(second.as_str())
        );
        assert!(
            shared.actions().resolve("anything").is_some(),
            "接口级回调兜底"
        );
    }

    #[test]
    fn context_has_no_js_lifetime_requirement() {
        // `PluginContext` 必须能在没有 JS 上下文的地方构造（宿主在启用插件前就要能拼出来）
        let meta = PluginMeta {
            id: "demo".into(),
            name: "演示".into(),
            version: "0.1.0".into(),
            description: None,
            author: None,
            dir: std::path::PathBuf::from("/tmp/demo"),
            entry: "index.js".into(),
        };
        let context = PluginContext {
            host: Arc::new(crate::test_support::FakePluginHost::new(meta.clone())),
            meta,
            permissions: PermissionSet::NONE,
            shared: Arc::new(Mutex::new(PluginShared::default())),
        };
        assert_eq!(context.meta.id, "demo");
        assert!(!context.permissions.has(Permission::Feedback));
    }
}
