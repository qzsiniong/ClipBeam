//! `$.sleep(ms)`：异步等待，且**可被取消**。
//!
//! 这是 core 自带的最小异步能力：脚本想「等一会儿」必须用它（引擎没有 `setTimeout`）。
//!
//! 与「一次性 `tokio::time::sleep`」的区别：这里按 [`SLICE`] 分片等待，每片检查一次取消令牌。
//! 取消时立即 resolve（不抛异常）—— 这样脚本能继续跑自己的收尾逻辑，而不会卡在长睡眠里。

use std::time::Duration;

use rquickjs::prelude::Async;
use rquickjs::{Ctx, Function, Object, Result as QjsResult};

use super::cancelled;

/// 取消检查的粒度：最坏情况下一个 `$.sleep(N)` 会多等这么久才返回。
const SLICE: Duration = Duration::from_millis(50);

/// 把 `$.sleep` 挂到命名空间对象上。
pub fn register<'js>(ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
    ns.set("sleep", Function::new(ctx.clone(), Async(js_sleep))?)?;
    Ok(())
}

/// `$.sleep(ms) -> Promise<void>`
///
/// 走 tokio 定时器，不占用线程；等待期间引擎会继续驱动其他 JS 任务，
/// 因此 `Promise.all` 里的多个 `$.sleep` 是并行的。
async fn js_sleep<'js>(ctx: Ctx<'js>, ms: u64) {
    let mut left = Duration::from_millis(ms);
    while !left.is_zero() {
        if cancelled(&ctx) {
            return;
        }
        let step = SLICE.min(left);
        tokio::time::sleep(step).await;
        left -= step;
    }
}
