//! `$.type_str` / `$.confirm` / `$.request_focus` / `$.pick_path`：宿主交互扩展。
//!
//! 这几个能力不属于引擎：它们问的是「把文本送到哪儿」「谁来回答」「选哪个路径」。
//! 因此实现只做一件事 —— 通过 [`crate::host::host_ctx`] 找到使用方注入的宿主，
//! 把动作委托出去：
//!
//! * CLI 宿主：写终端 / 读 stdin（选路径也是在终端里输入一行）；
//! * GUI 宿主：逐键打进当前焦点窗口 / 弹系统原生确认框与选择框 / 待命窗口。
//!
//! 拿不到宿主时抛出明确错误（而不是静默什么都不做），方便定位接线问题。

use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Result as QjsResult, String as JsString, Value};

use script_engine::bindings::{cancelled, throw_cancelled};

use crate::extensions::{extension, require_host, throw_host_error};
use crate::host::{ConfirmChoice, PickKind};
use crate::HostError;

extension! {
    /// `$.type_str`。
    pub struct TypeStrExtension;
    "type_str" => js_type_str,
        "type_str(text: string, delayMs?: number) -> void",
        "把文本交给宿主输出（GUI 下逐键打进当前焦点窗口）；被中止时抛异常";
}

extension! {
    /// `$.confirm`。
    pub struct ConfirmExtension;
    async "confirm" => js_confirm,
        "confirm(message: string) -> Promise<boolean>",
        "向用户提问；回答「是」为 true，「否」为 false，用户中止时抛异常";
}

extension! {
    /// `$.request_focus`。
    pub struct RequestFocusExtension;
    "request_focus" => js_request_focus,
        "request_focus(hint?: string) -> void",
        "请求用户把焦点切到目标窗口（GUI 下弹出待命窗口并等待确认）；hint 是显示给用户的提示";
}

extension! {
    /// `$.pick_path`。
    pub struct PickPathExtension;
    async "pick_path" => js_pick_path,
        "pick_path(prompt?: string, kind?: \"file\" | \"dir\") -> Promise<string | null>",
        "让用户挑一个文件或文件夹（prompt 是选择框标题）；取消或没有选择界面时返回 null";
}

/// `type_str(text, delayMs = 0) -> void`
///
/// 同步阻塞（宿主负责每个字符之间的等待），因此输出顺序与脚本执行顺序严格一致。
/// 被中止时抛「脚本已中止」，让脚本立即结束。
fn js_type_str<'js>(ctx: Ctx<'js>, text: String, delay_ms: Opt<u64>) -> QjsResult<()> {
    let host = require_host(&ctx, "$.type_str")?;

    if cancelled(&ctx) {
        return Err(throw_cancelled(&ctx));
    }

    // `Opt`：JS 侧可以只传 text，delayMs 缺省为 0（`Option<u64>` 会要求参数个数完全匹配）
    host.type_str(&text, delay_ms.0.unwrap_or(0))
        .map_err(|err| throw_host_error(&ctx, err))
}

/// `confirm(message) -> Promise<boolean>`
///
/// * `Yes` → `true`；`No` → `false`；
/// * `Abort` → 抛「脚本已中止」，让脚本立即结束（不是被拒绝的 Promise，避免脚本
///   `catch` 之后继续往下跑）。
async fn js_confirm<'js>(ctx: Ctx<'js>, message: String) -> QjsResult<bool> {
    let host = require_host(&ctx, "$.confirm")?;

    match host.confirm(&message) {
        Ok(ConfirmChoice::Yes) => Ok(true),
        Ok(ConfirmChoice::No) => Ok(false),
        Ok(ConfirmChoice::Abort) => Err(throw_cancelled(&ctx)),
        Err(err) => Err(throw_host_error(&ctx, err)),
    }
}

/// `request_focus(hint?) -> void`
///
/// **同步**阻塞到用户确认（或超时/中止）—— 与 [`js_type_str`] 一致：返回即表示「焦点已经在
/// 用户选定的目标窗口上」，脚本可以放心继续输出。脚本要在**中途**换一个输出目标时显式调用它
/// （例如先打进浏览器、再打进记事本）；不调用也能靠宿主的自动焦点校验兜住。
fn js_request_focus<'js>(ctx: Ctx<'js>, hint: Opt<String>) -> QjsResult<()> {
    let host = require_host(&ctx, "$.request_focus")?;

    if cancelled(&ctx) {
        return Err(throw_cancelled(&ctx));
    }

    host.request_focus(hint.0.as_deref().unwrap_or(""))
        .map_err(|err| throw_host_error(&ctx, err))
}

/// `pick_path(prompt?, kind?) -> Promise<string | null>`
///
/// * `prompt` 缺省或空串时由宿主用默认文案（「请选择文件」/「请选择文件夹」）；
/// * `kind` 只认 `"file"`（缺省）/ `"dir"`：写错了当场抛错，而不是静默按文件处理；
/// * 返回所选文件 / 文件夹的**绝对路径**；用户取消、或宿主没有选择界面（headless、
///   命令行非交互）时返回 `null` —— 取消是正常操作，不该逼脚本 `try/catch`。
async fn js_pick_path<'js>(
    ctx: Ctx<'js>,
    prompt: Opt<String>,
    kind: Opt<String>,
) -> QjsResult<Value<'js>> {
    let host = require_host(&ctx, "$.pick_path")?;

    if cancelled(&ctx) {
        return Err(throw_cancelled(&ctx));
    }

    let kind = match kind.0.as_deref() {
        None | Some("") | Some("file") => PickKind::File,
        Some("dir") => PickKind::Dir,
        Some(other) => {
            return Err(Exception::throw_message(
                &ctx,
                &format!("$.pick_path 的 kind 只支持 \"file\" / \"dir\"，收到：{other:?}"),
            ));
        }
    };

    let picked = host
        .pick_path(prompt.0.as_deref().unwrap_or(""), kind)
        .map_err(|err| throw_host_error(&ctx, err))?;

    // 显式造 `null`：rquickjs 把 `Option::None` 映射成 `undefined`，而「没选到」在
    // JS 侧的语义是 `null`（`p === null` 才是判断取消的写法）
    let Some(path) = picked else {
        return Ok(Value::new_null(ctx.clone()));
    };

    match path.to_str() {
        Some(text) => Ok(JsString::from_str(ctx.clone(), text)?.into_value()),
        None => Err(throw_host_error(
            &ctx,
            HostError::Failed(format!("所选路径不是有效的 UTF-8：{}", path.display())),
        )),
    }
}
