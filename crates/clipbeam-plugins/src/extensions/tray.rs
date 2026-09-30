//! 托盘能力：`$plugin.tray.onAction` / `$plugin.tray.setTooltip` / `$plugin.tray.setBadge`。
//!
//! # 动作菜单由清单声明，回调在入口里登记
//!
//! `plugin.json` 的 `menus` 决定**菜单长什么样**（宿主在启用插件时一次性建好），
//! `$plugin.tray.onAction(cb)` 决定**点击时执行什么**：
//!
//! ```js
//! $plugin.tray.onAction((action) => {
//!   // action.id 对应 plugin.json 里的 menus[].id
//! })
//! ```
//!
//! 两边必须都写：
//!
//! * 只写清单不写回调 → 点菜单时 [`crate::runtime::PluginRuntime::eval_action`] 报
//!   「没有登记动作」，而不是「点了没反应」；
//! * 只写回调不写清单 → 没有菜单项，回调不会被动到。
//!
//! # 一个回调管所有菜单项
//!
//! 这里把回调登记为**接口级**（[`GENERIC_ACTION_KEY`]）：插件加菜单项不需要改 JS ——
//! 具体做哪件事由载荷里的 `action.id` 决定。宿主派发时先找具体 id、再回落到接口级，
//! 所以将来支持「一个菜单项一个回调」的写法也不用改这里。

use rquickjs::function::Opt;
use rquickjs::{Ctx, Function, Object, Result as QjsResult};

use crate::context::{require_permission, throw_plugin_error};
use crate::host::{TrayRequest, GENERIC_ACTION_KEY};
use crate::permission::Permission;

/// `$plugin.tray`：托盘能力。
///
/// 手写 `register` 而不是用 `extension!` 宏：这组能力挂在**嵌套对象** `tray` 上
/// （`$plugin.tray.onAction`），宏只处理「直接挂在命名空间上」这一类。
/// 与 `clipbeam-scripting` 里 `file.rs` 手写 `register` 是同一个理由。
pub struct TrayExtension;

impl script_engine::ScriptExtension for TrayExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        let tray = Object::new(ctx.clone())?;
        tray.set("onAction", Function::new(ctx.clone(), js_on_action)?)?;
        tray.set("setTooltip", Function::new(ctx.clone(), js_set_tooltip)?)?;
        tray.set("setBadge", Function::new(ctx.clone(), js_set_badge)?)?;
        ns.set("tray", tray)?;
        Ok(())
    }

    /// 声明嵌套挂点：`tray` 对象上有这三个成员。
    ///
    /// 引擎拿它去校验 `spec()` 里那三条 `tray.x` 是否真的注册了
    /// （否则引擎只会在命名空间的自有属性名里找 `"tray.onAction"`，永远找不到）。
    fn register_nested(&self) -> Vec<(String, Vec<String>)> {
        vec![(
            "tray".to_string(),
            vec![
                "onAction".to_string(),
                "setTooltip".to_string(),
                "setBadge".to_string(),
            ],
        )]
    }

    fn spec(&self) -> Vec<script_engine::CapabilitySpec> {
        vec![
            script_engine::CapabilitySpec {
                name: "tray.onAction",
                signature: "onAction(callback: (action: { id: string }) => void) -> void",
                doc:
                    "登记托盘菜单动作的回调：菜单项被点击时调用 callback，action.id 是 menus[].id；\
                      重复调用会覆盖上一次登记",
            },
            script_engine::CapabilitySpec {
                name: "tray.setTooltip",
                signature: "setTooltip(text: string) -> void",
                doc: "设置托盘图标的鼠标悬停提示",
            },
            script_engine::CapabilitySpec {
                name: "tray.setBadge",
                signature: "setBadge(text: string | null) -> void",
                doc: "设置托盘徽标文字（macOS 菜单栏）；传 null 清除",
            },
        ]
    }
}

/// `tray.onAction(callback) -> void`
fn js_on_action<'js>(ctx: Ctx<'js>, callback: Function<'js>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Tray, "$plugin.tray.onAction")?;

    let global_name = {
        let mut shared = context
            .shared
            .lock()
            .map_err(|_| rquickjs::Exception::throw_message(&ctx, "插件状态锁已损坏"))?;
        shared.register_action(GENERIC_ACTION_KEY)
    };

    // 挂成全局函数：宿主派发时按名字唤醒（`eval_global(global_name, action)`）。
    // 回调本身由这个绑定持有 —— 插件自己删掉它就等于注销。
    ctx.globals().set(global_name, callback)?;
    Ok(())
}

/// `tray.setTooltip(text) -> void`
fn js_set_tooltip<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Tray, "$plugin.tray.setTooltip")?;
    if text.trim().is_empty() {
        return Err(throw_plugin_error(
            &ctx,
            crate::PluginError::InvalidArgument("$plugin.tray.setTooltip 的 text 不能为空".into()),
        ));
    }
    context
        .host
        .tray(TrayRequest::SetTooltip { text })
        .map_err(|err| throw_plugin_error(&ctx, err))?;
    Ok(())
}

/// `tray.setBadge(text) -> void`（`null` 清除）
///
/// 参数收 `Value` 而不是 `Opt<String>`：`null` 是**合法输入**（清除徽标），
/// 而 `Option<String>` 的 `FromJs` 不认 `null`（会报 "converting null into string"）。
fn js_set_badge<'js>(ctx: Ctx<'js>, text: Opt<rquickjs::Value<'js>>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Tray, "$plugin.tray.setBadge")?;

    let text = match text.0 {
        // 省略、null、undefined 都表示「清除」
        None => None,
        Some(value) if value.is_null() || value.is_undefined() => None,
        Some(value) => Some(
            value
                .as_string()
                .ok_or_else(|| {
                    throw_plugin_error(
                        &ctx,
                        crate::PluginError::InvalidArgument(
                            "$plugin.tray.setBadge 的参数必须是字符串或 null".into(),
                        ),
                    )
                })?
                .to_string()?,
        ),
    };

    context
        .host
        .tray(TrayRequest::SetBadge { text })
        .map_err(|err| throw_plugin_error(&ctx, err))?;
    Ok(())
}
