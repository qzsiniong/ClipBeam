//! 取消感知的等待实现：`sleep(ms)`（**标准全局**，不是命名空间里的能力）。
//!
//! 引擎把它挂成全局函数（见 [`crate::extension::setup`]），脚本写成 `await sleep(ms)` ——
//! 它是 Promise，**必须 await**，否则只是发起等待、不会真的等。
//!
//! 与「一次性 `tokio::time::sleep`」的区别：这里按 [`SLICE`] 分片等待，每片检查一次取消信号。
//! 取消时立即 resolve（不抛异常）—— 这样脚本能继续跑自己的收尾逻辑，而不会卡在长睡眠里。
//!
//! 为什么是全局而不是能力：`setTimeout` / `atob` / `TextDecoder` 这些引擎补的都是规范里的
//! 名字，`sleep` 不是任何 JS 规范里的名字 —— 引擎**补规范，不发明 API 名字**；但「等一会儿」
//! 是每个脚本都会用的东西，所以由使用方在 `RuntimeOptions` 里决定能力命名空间叫什么，
//! 而 `sleep` 作为引擎自带的全局固定下来（名字由本 crate 的文档与 d.ts 一起承诺）。

use std::time::Duration;

use rquickjs::Ctx;

use super::cancelled;

/// 取消检查的粒度：最坏情况下一次 `sleep(N)` 会多等这么久才返回。
const SLICE: Duration = Duration::from_millis(50);

/// `sleep(ms) -> Promise<void>`
///
/// 走 tokio 定时器，不占用线程；等待期间引擎会继续驱动其他 JS 任务，
/// 因此 `Promise.all` 里的多个 `sleep` 是并行的。
pub async fn sleep<'js>(ctx: Ctx<'js>, ms: u64) {
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
