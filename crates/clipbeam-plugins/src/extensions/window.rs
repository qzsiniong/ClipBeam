//! 窗口能力：`$plugin.window.*` —— 插件开自己的窗口、往页面发消息、收页面消息。
//!
//! # 一个窗口的生命周期
//!
//! ```text
//! 插件 JS               插件线程                 主线程                    插件页面(iframe)
//! ─────────────────────────────────────────────────────────────────────────────────────
//! open(opts) ─────────► window(Open) ──────────► 建 Tauri 窗口 ──────────► 加载页面
//!   ▲                       │                       │
//!   └─ 返回 PluginWindow ◄──┘（标签/序号由宿主给）    │
//! post(id, msg) ────────► notify(Message) ──────► 前端 relay ────────────► window.onmessage
//! onMessage(id, cb) ◄─── 登记回调            ◄──── 页面 postMessage ◄─────── postMessage(payload)
//! onClosed(id, cb) ◄──── 唤醒回调            ◄──── 窗口关闭
//! ```
//!
//! # 为什么回调要按窗口登记
//!
//! 托盘动作是「一个插件一个回调」（具体哪个菜单项由 `action.id` 分）；窗口不一样：
//! **每个窗口是独立的对象**，插件自然希望「这个窗口的消息给这个回调」。所以这里按窗口 id
//! 登记（键形如 `window:<id>:message`），宿主的事件通知里也带着窗口 id
//! （[`crate::host::WindowNotice`]）。
//!
//! 窗口关闭时宿主**一定会通知**（`WindowNotice::Closed`）—— 那是插件释放回调的唯一时机，
//! 所以 `onClosed` 是这个接口里最值得写的一段。
//!
//! # 与 JS 之间怎么传 JSON
//!
//! rquickjs 0.14 **没有** Rust ↔ JS 的 serde 互转（没有 `FromJs for serde_json::Value`）。
//! 所以这里走一条最直的路：把值交给 JS 侧的 `JSON.stringify`，拿回字符串后用
//! `serde_json` 解析。好处是语义与 JS 完全一致（`undefined`、`NaN`、循环引用都按 JS 的规矩来）。
//! 载荷统一包成 `{ windowId, message }` 再序列化 —— 这样 `undefined` 不会退化成"整条消息没了"，
//! 而是「`message` 字段缺失」，Rust 侧能分辨。

use rquickjs::function::Opt;
use rquickjs::{Ctx, Function, Object, Result as QjsResult, Value};

use crate::context::{require_permission, throw_plugin_error};
use crate::host::{WindowNotice, WindowOptions, WindowRequest};
use crate::permission::Permission;

/// `$plugin.window`：自定义窗口能力。
///
/// 手写 `register`（而不是用 `extension!` 宏）：这组能力挂在嵌套对象 `window` 上
/// （`$plugin.window.open`），宏只处理「直接挂在命名空间上」那一类。
pub struct WindowExtension;

impl script_engine::ScriptExtension for WindowExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        let window = Object::new(ctx.clone())?;
        window.set("open", Function::new(ctx.clone(), js_open)?)?;
        window.set("onMessage", Function::new(ctx.clone(), js_on_message)?)?;
        window.set("onClosed", Function::new(ctx.clone(), js_on_closed)?)?;
        window.set("close", Function::new(ctx.clone(), js_close)?)?;
        window.set("post", Function::new(ctx.clone(), js_post)?)?;
        ns.set("window", window)?;
        Ok(())
    }

    fn register_nested(&self) -> Vec<(String, Vec<String>)> {
        vec![(
            "window".to_string(),
            vec![
                "open".to_string(),
                "onMessage".to_string(),
                "onClosed".to_string(),
                "close".to_string(),
                "post".to_string(),
            ],
        )]
    }

    fn spec(&self) -> Vec<script_engine::CapabilitySpec> {
        vec![
            script_engine::CapabilitySpec {
                name: "window.open",
                signature: "open(options?: WindowOptions) -> PluginWindow",
                doc: "打开一个插件窗口（页面取自插件目录，缺省 index.html）；\
                      返回 { id, label, seq }，后续 post / onMessage / close 都用 id",
            },
            script_engine::CapabilitySpec {
                name: "window.post",
                signature: "post(windowId: string, message: unknown) -> void",
                doc: "把一个 JSON 值发给窗口里的页面（页面用 window.onmessage 收）",
            },
            script_engine::CapabilitySpec {
                name: "window.onMessage",
                signature:
                    "onMessage(windowId: string, callback: (message: unknown) => void) -> void",
                doc: "登记某个窗口的页面消息回调；重复登记会覆盖",
            },
            script_engine::CapabilitySpec {
                name: "window.onClosed",
                signature: "onClosed(windowId: string, callback: () => void) -> void",
                doc: "登记某个窗口的关闭回调（用户关掉、插件请求关闭、页面异常都会触发）",
            },
            script_engine::CapabilitySpec {
                name: "window.close",
                signature: "close(windowId: string) -> void",
                doc: "关掉一个插件窗口（幂等：窗口已经不在了也不报错）",
            },
        ]
    }
}

// ── JSON 桥 ────────────────────────────────────────────────────────────────

/// 把一个 JS 值经 `JSON.stringify` 转成 Rust 侧 JSON。
///
/// 拿不到可序列化的值（`undefined` / 函数 / 循环引用）时返回 `None`。
fn value_to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> QjsResult<Option<serde_json::Value>> {
    // 直接用引擎求值好的 `JSON.stringify`：不捕获 `Ctx` 克隆（那会形成引用环，
    // 见 script-engine 的扩展文档），也不依赖 rquickjs 没有的 serde 互转。
    let stringify: Function<'js> = ctx.eval("JSON.stringify")?;
    let raw: rquickjs::Result<String> = stringify.call((value,));
    let Ok(text) = raw else {
        return Ok(None);
    };
    let parsed = serde_json::from_str(&text).map_err(|err| {
        rquickjs::Exception::throw_message(
            ctx,
            &format!(
                "消息无法解析成 JSON（{err}）：这通常意味着插件传了 undefined / 函数 / 循环引用"
            ),
        )
    })?;
    Ok(Some(parsed))
}

/// 把一个 JS 对象转成 Rust 结构（同样借道 `JSON.stringify`）。
fn object_to<'js, T: serde::de::DeserializeOwned>(
    ctx: &Ctx<'js>,
    object: Object<'js>,
    what: &str,
) -> QjsResult<T> {
    let Some(json) = value_to_json(ctx, object.into_value())? else {
        return Err(rquickjs::Exception::throw_message(
            ctx,
            &format!("{what} 不能为空"),
        ));
    };
    serde_json::from_value(json).map_err(|err| {
        rquickjs::Exception::throw_message(
            ctx,
            &format!(
                "{what} 的字段类型不对（{err}）：title 要字符串、width/height 要数字、\
                 resizable/alwaysOnTop/decorations/transparent/center 要布尔、page 要字符串"
            ),
        )
    })
}

// ── 动作表键 ────────────────────────────────────────────────────────────────

/// 动作表里的键：窗口消息。
///
/// 与托盘动作共用同一张表（[`crate::host::ActionRegistry`]）——
/// 两者都是「宿主叫醒插件里某个已登记的回调」，只是键的空间不同。
fn message_key(window_id: &str) -> String {
    format!("window:{window_id}:message")
}

/// 动作表里的键：窗口关闭。
fn closed_key(window_id: &str) -> String {
    format!("window:{window_id}:closed")
}

/// 一个窗口事件该派发到哪个键（宿主侧唤醒回调时用）。
///
/// 与上面的两个构造函数是同一份规则的两种用法：**写在一起才不会漂移**。
pub fn window_notice_key(notice: &WindowNotice) -> String {
    match notice {
        WindowNotice::Message { window_id, .. } => message_key(window_id),
        WindowNotice::Closed { window_id } => closed_key(window_id),
    }
}

// ── 能力实现 ────────────────────────────────────────────────────────────────

/// `window.open(options?) -> PluginWindow`
///
/// 窗口 id 由插件侧分配（`w1` / `w2`…）：它只是插件内的一个名字，用来在自己的回调里区分窗口。
/// 宿主用它 + 序号拼出真正的窗口标签再回给插件（`win.label`），所以插件永远不必自己拼标签。
fn js_open<'js>(ctx: Ctx<'js>, options: Opt<Object<'js>>) -> QjsResult<Object<'js>> {
    let context = require_permission(&ctx, Permission::Window, "$plugin.window.open")?;

    let options: WindowOptions = match options.0 {
        None => WindowOptions::default(),
        Some(object) => object_to(&ctx, object, "$plugin.window.open 的 options")?,
    };

    // 插件内的窗口 id 用**自增序号**：让插件自己造 id 的话，
    // 重名的窗口会把回调表覆盖掉（而它未必意识到）。
    let window_id = {
        let mut shared = context
            .shared
            .lock()
            .map_err(|_| rquickjs::Exception::throw_message(&ctx, "插件状态锁已损坏"))?;
        // 借动作表的自增序号：每次登记都会给出一个从未用过的新名字，
        // 这里只要那个序号（登记本身由 onMessage/onClosed 做）
        format!("w{}", shared.next_window_id())
    };

    let (label, seq) = context
        .host
        .window(WindowRequest::Open {
            window_id: window_id.clone(),
            options,
        })
        .map_err(|err| throw_plugin_error(&ctx, err))?;

    let win = Object::new(ctx.clone())?;
    win.set("id", window_id)?;
    win.set("label", label)?;
    win.set("seq", seq)?;
    Ok(win)
}

/// `window.post(windowId, message) -> void`
///
/// 消息经宿主转给窗口页面（页面用 `window.onmessage` 收）。
/// 载荷是 `{ windowId, message }`，其中 `message` **只在真的传了值时出现** ——
/// 于是页面侧能用 `"message" in event.data` 区分「插件发了 `undefined`」与「发了 `null`」，
/// 与 JS 里这两者的语义差保持一致。
fn js_post<'js>(ctx: Ctx<'js>, window_id: String, message: Value<'js>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Window, "$plugin.window.post")?;

    let mut payload = serde_json::Map::new();
    payload.insert(
        "windowId".to_string(),
        serde_json::Value::String(window_id.clone()),
    );
    // `value_to_json` 对 `undefined` / 函数 / 循环引用返回 `None` → 干脆不放这个键
    if let Some(message) = value_to_json(&ctx, message)? {
        payload.insert("message".to_string(), message);
    }

    context
        .host
        .notify_window_event(WindowNotice::Message {
            window_id,
            message: serde_json::Value::Object(payload),
        })
        .map_err(|err| throw_plugin_error(&ctx, err))?;
    Ok(())
}

/// `window.onMessage(windowId, callback) -> void`
fn js_on_message<'js>(ctx: Ctx<'js>, window_id: String, callback: Function<'js>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Window, "$plugin.window.onMessage")?;
    register(&ctx, &context, &message_key(&window_id), callback)
}

/// `window.onClosed(windowId, callback) -> void`
fn js_on_closed<'js>(ctx: Ctx<'js>, window_id: String, callback: Function<'js>) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Window, "$plugin.window.onClosed")?;
    register(&ctx, &context, &closed_key(&window_id), callback)
}

/// `window.close(windowId) -> void`
fn js_close<'js>(ctx: Ctx<'js>, window_id: String) -> QjsResult<()> {
    let context = require_permission(&ctx, Permission::Window, "$plugin.window.close")?;
    context
        .host
        .window(WindowRequest::Close { window_id })
        .map(|_| ())
        .map_err(|err| throw_plugin_error(&ctx, err))
}

/// 把一个回调登记进动作表（`window:*` 键），并挂成全局函数 —— 宿主就是按这个名字唤醒它的。
///
/// 与托盘的 `onAction` 同一个手法（见 `extensions/tray.rs`）：**存名字而不是存 JS 值**，
/// 因为 rquickjs 0.14 的 `Persistent` 会让 JS 对象泄漏（存一个函数就会在运行时析构时 abort）。
fn register<'js>(
    ctx: &Ctx<'js>,
    context: &crate::context::PluginContext,
    key: &str,
    callback: Function<'js>,
) -> QjsResult<()> {
    let global_name = {
        let mut shared = context
            .shared
            .lock()
            .map_err(|_| rquickjs::Exception::throw_message(ctx, "插件状态锁已损坏"))?;
        shared.register_action(key)
    };
    ctx.globals().set(global_name, callback)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_keys_are_namespaced_by_window() {
        assert_eq!(
            window_notice_key(&WindowNotice::Message {
                window_id: "w1".into(),
                message: serde_json::Value::Null,
            }),
            "window:w1:message"
        );
        assert_eq!(
            window_notice_key(&WindowNotice::Closed {
                window_id: "w1".into()
            }),
            "window:w1:closed"
        );

        // 不同窗口的键不能撞（否则两个窗口的回调会互相覆盖）
        assert_ne!(message_key("w1"), message_key("w2"));
        assert_ne!(message_key("w1"), closed_key("w1"));
    }
}
