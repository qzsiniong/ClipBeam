//! `$.typeStr(text, delayMs)` 与 `$.confirm(text)`：宿主交互扩展。
//!
//! 这两个能力不属于引擎：它们问的是「把文本送到哪儿」「谁来回答」。因此实现只做一件事 ——
//! 通过 [`clipbeam_script::bindings::host_ctx`] 找到使用方注入的宿主，把动作委托出去：
//!
//! * CLI 宿主：写终端 / 读 stdin；
//! * GUI 宿主：逐键打进当前焦点窗口 / 弹确认框。
//!
//! 拿不到宿主时抛出明确错误（而不是静默什么都不做），方便定位接线问题。

use clipbeam_script::bindings::{cancelled, host_ctx, throw_cancelled};
use clipbeam_script::{CapabilitySpec, ConfirmChoice, HostError, ScriptExtension};
use rquickjs::function::Opt;
use rquickjs::prelude::Async;
use rquickjs::{Ctx, Exception, Function, Object, Result as QjsResult};

/// `$.typeStr`：把文本交给宿主输出（GUI 下是逐键打进焦点窗口）。
pub struct TypeStrExtension;

impl ScriptExtension for TypeStrExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("typeStr", Function::new(ctx.clone(), js_type_str)?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "typeStr",
            signature: "typeStr(text: string, delayMs?: number) -> void",
            doc: "把文本交给宿主输出（GUI 下逐键打进当前焦点窗口）；被中止时抛异常",
        }]
    }
}

/// `$.confirm`：向用户提问并等待回答。
pub struct ConfirmExtension;

impl ScriptExtension for ConfirmExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("confirm", Function::new(ctx.clone(), Async(js_confirm))?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "confirm",
            signature: "confirm(message: string) -> Promise<boolean>",
            doc: "向用户提问；回答「是」为 true，超时/无应答按 false，用户中止时抛异常",
        }]
    }
}

/// `typeStr(text, delayMs = 0) -> void`
///
/// 同步阻塞（宿主负责每个字符之间的等待），因此输出顺序与脚本执行顺序严格一致。
/// 被取消时抛「脚本已中止」，让脚本立即结束。
fn js_type_str<'js>(ctx: Ctx<'js>, text: String, delay_ms: Opt<u64>) -> QjsResult<()> {
    let Some(host) = host_ctx(&ctx) else {
        return Err(Exception::throw_message(
            &ctx,
            "$.typeStr 不可用：当前运行环境没有注入宿主（把 ScriptHost 传给 RuntimeOptions::host）",
        ));
    };

    if cancelled(&ctx) {
        return Err(throw_cancelled(&ctx));
    }

    // `Opt`：JS 侧可以只传 text，delayMs 缺省为 0（Option<u64> 会要求参数个数完全匹配）
    host.type_str(&text, delay_ms.0.unwrap_or(0))
        .map_err(|err| host_error_to_js(&ctx, err))
}

/// `confirm(message) -> Promise<boolean>`
///
/// * `Yes` → `true`；`No` / 超时 → `false`；
/// * `Abort` → 抛「脚本已中止」，由脚本的 `try/catch` 或外层决定如何收场。
async fn js_confirm<'js>(ctx: Ctx<'js>, message: String) -> QjsResult<bool> {
    let Some(host) = host_ctx(&ctx) else {
        return Err(Exception::throw_message(
            &ctx,
            "$.confirm 不可用：当前运行环境没有注入宿主（把 ScriptHost 传给 RuntimeOptions::host）",
        ));
    };

    match host.confirm(&message) {
        Ok(ConfirmChoice::Yes) => Ok(true),
        Ok(ConfirmChoice::No) => Ok(false),
        Ok(ConfirmChoice::Abort) => Err(throw_cancelled(&ctx)),
        Err(err) => Err(host_error_to_js(&ctx, err)),
    }
}

/// 把宿主错误转成 JS 异常（取消 → 统一的「脚本已中止」）。
fn host_error_to_js<'js>(ctx: &Ctx<'js>, err: HostError) -> rquickjs::Error {
    if err == HostError::Cancelled {
        return throw_cancelled(ctx);
    }
    Exception::throw_message(ctx, &host_error_message(&err))
}

/// 宿主错误的展示文案（取消已由 [`host_error_to_js`] 提前处理）。
fn host_error_message(err: &HostError) -> String {
    match err {
        // 上面的分支已拦截，这里只做兜底
        HostError::Cancelled => "脚本已中止".to_string(),
        HostError::Timeout => "等待用户确认超时（按「否」处理）".to_string(),
        HostError::Unsupported => "当前运行环境不支持该能力".to_string(),
        HostError::Failed(message) => message.clone(),
    }
}
