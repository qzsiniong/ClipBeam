//! `$.file(path)`：从真实文件系统读取文件。
//!
//! 对 JS 暴露的是**异步**接口：返回 `Promise<ArrayBuffer>`，因此 JS 侧必须 `await`：
//!
//! ```js
//! const bytes = await $.file("data.bin")   // ArrayBuffer
//! ```
//!
//! 实现走 `spawn_blocking`（而不是异步文件 IO）：既不阻塞引擎线程，也不给 core 引入
//! tokio 的 `fs` feature；同时 `spawn_blocking` 的返回值 `Vec<u8>` 是 `Send`，
//! 满足 `rquickjs::prelude::Async` 对 future 的约束。

use rquickjs::prelude::Async;
use rquickjs::{ArrayBuffer, Ctx, Exception, Function, Object, Result as QjsResult};

/// 把 `$.file` 挂到命名空间对象上。
pub fn register<'js>(ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
    ns.set("file", Function::new(ctx.clone(), Async(js_file))?)?;
    Ok(())
}

/// `$.file(path) -> Promise<ArrayBuffer>`
///
/// * `path`：文件路径；**相对路径按进程工作目录解析**。没有任何沙箱限制，
///   调用方需要自己保证脚本可信、路径可控。
/// * 成功：内容是文件原始字节（二进制安全，不做编码转换）。
/// * 失败（不存在 / 是目录 / 无权限）：返回被拒绝的 `Promise`，错误信息里带上
///   路径与系统原因，JS 侧可以 `try/catch` 或 `.catch()` 捕获。
///
/// 参数表里的 `ctx: Ctx<'js>` 由 `rquickjs` 自动注入，JS 调用时只传 `path`。
async fn js_file<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<ArrayBuffer<'js>> {
    let read_path = path.clone();
    let data = tokio::task::spawn_blocking(move || std::fs::read(&read_path))
        .await
        .map_err(|err| Exception::throw_message(&ctx, &format!("$.file 读取 {path:?} 的任务异常：{err}")))?
        .map_err(|err| Exception::throw_message(&ctx, &format!("$.file 读取 {path:?} 失败：{err}")))?;

    // ArrayBuffer::new 会把 Vec 的所有权交给 JS（由 QuickJS 负责释放），零拷贝
    ArrayBuffer::new(ctx, data)
}
