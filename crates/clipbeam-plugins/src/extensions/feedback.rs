//! 反馈能力：`$plugin.toast` / `$plugin.notify` / `$plugin.alert` / `$plugin.confirm`。
//!
//! 这几个能力问的都是「让用户看到点什么、或者问一句」——具体落到哪儿由宿主决定
//! （GUI：应用内 toast / 通知中心 / 原生对话框；测试：假宿主记下来）。
//!
//! # 同步 vs 异步是刻意的
//!
//! | 能力 | 形态 | 理由 |
//! |---|---|---|
//! | `toast` / `notify` | 同步 `void` | 只投递、不等回答 —— 插件不该为了弹个提示就停下 |
//! | `alert` / `confirm` | `async` 返回 Promise | 必须等用户回答，插件要能 `await` 拿结果 |
//!
//! # 「取消」不是错误
//!
//! `confirm` 的用户取消 / 超时返回 `null`（第三个按钮、直接关掉、超时都算），
//! 而不是抛异常 —— 取消是正常操作，不该逼插件写 `try/catch`。

use rquickjs::function::Opt;
use rquickjs::{Ctx, Result as QjsResult, Value};

use crate::context::{require_permission, throw_plugin_error};
use crate::extensions::extension;
use crate::host::{DialogButtons, DialogChoice, Feedback, FeedbackOutcome, ToastLevel};
use crate::permission::Permission;

extension! {
    /// `$plugin.toast(message, options?)`：应用内提示。
    pub struct ToastExtension;
    "toast" => js_toast,
        "toast(message: string, options?: ToastOptions) -> void",
        "在应用窗口里弹一条提示（不阻塞）；options.level 取 info / success / warning / error，\
         options.durationMs 是自动消失时间（毫秒，缺省 4000）";
}

extension! {
    /// `$plugin.notify(title, body?)`：系统通知。
    pub struct NotifyExtension;
    "notify" => js_notify,
        "notify(title: string, body?: string) -> void",
        "发一条系统通知（macOS 通知中心 / Windows Toast；其它平台静默无效果）";
}

extension! {
    /// `$plugin.alert(message, options?)`：只有一个「好」的对话框。
    pub struct AlertExtension;
    async "alert" => js_alert,
        "alert(message: string, options?: DialogOptions) -> Promise<void>",
        "弹一个系统原生提示框，等用户点「好」；options.title 是标题，options.timeoutMs 是超时（毫秒）";
}

extension! {
    /// `$plugin.confirm(message, options?)`：是 / 否 对话框。
    pub struct ConfirmExtension;
    async "confirm" => js_confirm,
        "confirm(message: string, options?: DialogOptions) -> Promise<boolean | null>",
        "弹一个系统原生确认框并等用户回答：主按钮 true、次按钮 false、\
         关闭/超时/第三个按钮 null；options.buttons 可换成自定义文案";
}

/// 从 JS 侧的 options 对象里读出几个字段。
///
/// 为什么手工取而不是定义一个 `FromJs` 结构：options 的每个字段都**可选**，
/// 而且取值要在插件上下文里立刻校验（例如 level 写错要当场报错）。
fn read_str<'js>(options: &Opt<rquickjs::Object<'js>>, key: &str) -> QjsResult<Option<String>> {
    let Some(object) = options.0.as_ref() else {
        return Ok(None);
    };
    if !object.contains_key(key)? {
        return Ok(None);
    }
    Ok(Some(object.get::<_, String>(key)?))
}

/// 读一个数字字段（毫秒）。
fn read_u64<'js>(options: &Opt<rquickjs::Object<'js>>, key: &str) -> QjsResult<Option<u64>> {
    let Some(object) = options.0.as_ref() else {
        return Ok(None);
    };
    if !object.contains_key(key)? {
        return Ok(None);
    }
    Ok(Some(object.get::<_, u64>(key)?))
}

/// `toast(message, options?) -> void`
fn js_toast<'js>(
    ctx: Ctx<'js>,
    message: String,
    options: Opt<rquickjs::Object<'js>>,
) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Feedback, "$plugin.toast")?;

    if message.trim().is_empty() {
        return Err(throw_plugin_error(
            &ctx,
            crate::PluginError::InvalidArgument("$plugin.toast 的 message 不能为空".into()),
        ));
    }

    let raw_level = read_str(&options, "level")?;
    let level =
        ToastLevel::parse(raw_level.as_deref()).map_err(|err| throw_plugin_error(&ctx, err))?;
    let duration_ms = read_u64(&options, "durationMs")?;

    context
        .host
        .feedback(Feedback::Toast {
            level,
            message,
            duration_ms,
        })
        .map_err(|err| throw_plugin_error(&ctx, err))?;

    Ok(())
}

/// `notify(title, body?) -> void`
fn js_notify<'js>(ctx: Ctx<'js>, title: String, body: Opt<String>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Notification, "$plugin.notify")?;

    if title.trim().is_empty() {
        return Err(throw_plugin_error(
            &ctx,
            crate::PluginError::InvalidArgument("$plugin.notify 的 title 不能为空".into()),
        ));
    }

    context
        .host
        .feedback(Feedback::Notify {
            title,
            body: body.0.filter(|text| !text.is_empty()),
        })
        .map_err(|err| throw_plugin_error(&ctx, err))?;

    Ok(())
}

/// `alert(message, options?) -> Promise<void>`
async fn js_alert<'js>(
    ctx: Ctx<'js>,
    message: String,
    options: Opt<rquickjs::Object<'js>>,
) -> QjsResult<()> {
    let buttons = read_buttons(&ctx, &options, DialogButtons::Ok)?;
    // alert 只关心「用户看过了」，任何回答都算成功
    js_dialog(&ctx, "$plugin.alert", message, buttons, &options).await?;
    Ok(())
}

/// `confirm(message, options?) -> Promise<boolean | null>`
async fn js_confirm<'js>(
    ctx: Ctx<'js>,
    message: String,
    options: Opt<rquickjs::Object<'js>>,
) -> QjsResult<Value<'js>> {
    let buttons = read_buttons(&ctx, &options, DialogButtons::OkCancel)?;
    let choice = js_dialog(&ctx, "$plugin.confirm", message, buttons, &options).await?;

    match choice {
        DialogChoice::Primary => Ok(Value::new_bool(ctx.clone(), true)),
        DialogChoice::Secondary => Ok(Value::new_bool(ctx.clone(), false)),
        // 第三个按钮 / 直接关掉 / 超时都是「没有回答」→ null（不是错误）
        DialogChoice::Tertiary | DialogChoice::Dismissed | DialogChoice::Timeout => {
            Ok(Value::new_null(ctx.clone()))
        }
    }
}

/// 读 `options.buttons`：支持 `"ok"` / `"okCancel"` 与 `{ primary, secondary?, tertiary? }`。
fn read_buttons<'js>(
    ctx: &Ctx<'js>,
    options: &Opt<rquickjs::Object<'js>>,
    fallback: DialogButtons,
) -> QjsResult<DialogButtons> {
    let Some(object) = options.0.as_ref() else {
        return Ok(fallback);
    };
    if !object.contains_key("buttons")? {
        return Ok(fallback);
    }

    let raw: Value = object.get("buttons")?;

    if let Some(text) = raw.as_string() {
        let text = text.to_string()?;
        return match text.as_str() {
            "ok" => Ok(DialogButtons::Ok),
            "okCancel" => Ok(DialogButtons::OkCancel),
            other => Err(throw_plugin_error(
                ctx,
                crate::PluginError::InvalidArgument(format!(
                    "不认识 buttons={other:?}：可用 \"ok\" / \"okCancel\"，或传 {{ primary, secondary, tertiary }}"
                )),
            )),
        };
    }

    let Some(custom) = raw.as_object() else {
        return Err(throw_plugin_error(
            ctx,
            crate::PluginError::InvalidArgument(
                "buttons 必须是 \"ok\" / \"okCancel\" 或含 primary 的对象".into(),
            ),
        ));
    };

    // primary 必填：自定义按钮至少要有一个主按钮，否则用户没有「确定」可点
    if !custom.contains_key("primary")? {
        return Err(throw_plugin_error(
            ctx,
            crate::PluginError::InvalidArgument("buttons 对象必须带 primary（主按钮文案）".into()),
        ));
    }

    Ok(DialogButtons::Custom {
        primary: custom.get::<_, String>("primary")?,
        secondary: if custom.contains_key("secondary")? {
            Some(custom.get::<_, String>("secondary")?)
        } else {
            None
        },
        tertiary: if custom.contains_key("tertiary")? {
            Some(custom.get::<_, String>("tertiary")?)
        } else {
            None
        },
    })
}

/// `alert` / `confirm` 的公共实现：鉴权 → 校验 → 交给宿主 → 取回答。
async fn js_dialog<'js>(
    ctx: &Ctx<'js>,
    capability: &str,
    message: String,
    buttons: DialogButtons,
    options: &Opt<rquickjs::Object<'js>>,
) -> QjsResult<DialogChoice> {
    let context = require_permission(ctx, Permission::SystemDialog, capability)?;

    if message.trim().is_empty() {
        return Err(throw_plugin_error(
            ctx,
            crate::PluginError::InvalidArgument(format!("{capability} 的 message 不能为空")),
        ));
    }

    let title = read_str(options, "title")?;
    let timeout_ms = read_u64(options, "timeoutMs")?;

    let outcome = context
        .host
        .feedback(Feedback::Dialog {
            title,
            message,
            buttons,
            timeout_ms,
        })
        .map_err(|err| throw_plugin_error(ctx, err))?;

    Ok(match outcome {
        FeedbackOutcome::Chosen { choice } => choice,
        // 宿主没给选择（实现只投递不回答）→ 按「没有回答」处理，别假装用户点了「好」
        FeedbackOutcome::Delivered => DialogChoice::Dismissed,
    })
}
