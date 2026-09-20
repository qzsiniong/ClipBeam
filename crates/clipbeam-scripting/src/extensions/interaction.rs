//! `$.type_str(text, delayMs?)` 与 `$.confirm(message)`：宿主交互扩展。
//!
//! 这两个能力不属于引擎：它们问的是「把文本送到哪儿」「谁来回答」。因此实现只做一件事 ——
//! 通过 [`crate::host::host_ctx`] 找到使用方注入的宿主，把动作委托出去：
//!
//! * CLI 宿主：写终端 / 读 stdin；
//! * GUI 宿主：逐键打进当前焦点窗口 / 弹确认框。
//!
//! 拿不到宿主时抛出明确错误（而不是静默什么都不做），方便定位接线问题。

use rquickjs::function::Opt;
use rquickjs::{Ctx, Result as QjsResult};

use script_engine::bindings::{cancelled, throw_cancelled};

use crate::extensions::{extension, require_host, throw_host_error};
use crate::host::ConfirmChoice;

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
