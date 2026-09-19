//! JS 侧可见能力的公共设施。
//!
//! ## 内部命名空间 `__clipbeam`
//!
//! `TextDecoder` / `TextEncoder` / `console` 在 JS 侧是**类或对象**，更适合写在
//! `prelude.js` 里；Rust 只提供纯计算原语，挂在 `globalThis.__clipbeam` 上
//! （双下划线表示不对外），不污染 `$` / `Clipbeam` 的公开面。
//!
//! 其中 `__clipbeam.print` 的桥接放在 [`crate::extension::setup`] 里（它需要访问宿主，
//! 见「能力扩展机制」），本模块只负责纯计算的 `text` 原语。
//!
//! ## 读取二进制参数的注意事项
//!
//! 需要字节数据的绑定统一走 [`array_buffer_to_vec`]。`rquickjs` 0.13 起
//! `ArrayBuffer::as_bytes` 是 `unsafe`：JS 可能在借用期间 detach/transfer 该缓冲区，
//! 所以必须立刻拷贝一份到 Rust 侧，不能把借来的 `&[u8]` 带过任何 JS 调用。

pub mod basic;
pub mod file;
pub mod text;
pub mod timer;

use std::sync::Arc;

use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult};

use crate::host::{CancelRef, CancellationToken, HostRef, ScriptHost};

/// 读取 `ArrayBuffer` 的字节；缓冲区已被 detach 时返回 `None`。
///
/// 调用方必须**立即**消费返回的 `Vec<u8>`，不要把它变回借用传给别的 JS 调用。
pub fn array_buffer_bytes(buf: &ArrayBuffer<'_>) -> Option<Vec<u8>> {
    // SAFETY: 返回的切片只在本函数内被复制一次，期间不执行任何 JS 代码，
    // 因此不存在「借用期间被 JS detach/改写」的可能。
    unsafe { buf.as_bytes() }.map(<[u8]>::to_vec)
}

/// 把 JS 传入的 `ArrayBuffer` 拷贝成 Rust 的 `Vec<u8>`。
///
/// * `Ok(bytes)`：内容已复制，后续 JS 怎么折腾都不影响这份数据；
/// * `Err(..)`：缓冲区已被 detach（例如 JS 侧执行过 `transfer`），抛出 JS 异常。
pub fn array_buffer_to_vec<'js>(ctx: &Ctx<'js>, buf: &ArrayBuffer<'js>) -> QjsResult<Vec<u8>> {
    array_buffer_bytes(buf).ok_or_else(|| {
        Exception::throw_message(
            ctx,
            "ArrayBuffer 已被 detach（transfer 或 detached buffer），无法读取内容",
        )
    })
}

/// 取回上下文里的宿主句柄（克隆一个 `Arc`，调用方拿到的是所有权）。
///
/// 使用方的能力（`$.typeStr` / `$.confirm`）靠它访问宿主；返回 `None` 只会发生在
/// 「运行时还没注册就调用」这种顺序错误上。
pub fn host_ctx<'js>(ctx: &Ctx<'js>) -> Option<Arc<dyn ScriptHost>> {
    ctx.userdata::<HostRef>().map(|guard| guard.0.clone())
}

/// 是否已被取消；没有宿主也没有令牌时按「未取消」处理。
pub fn cancelled<'js>(ctx: &Ctx<'js>) -> bool {
    if let Some(host) = host_ctx(ctx) {
        if host.cancelled() {
            return true;
        }
    }
    cancel_ctx(ctx).is_some_and(|token| token.is_cancelled())
}

/// 取回上下文里的取消令牌（克隆，廉价）。
pub fn cancel_ctx<'js>(ctx: &Ctx<'js>) -> Option<CancellationToken> {
    ctx.userdata::<CancelRef>().map(|guard| guard.0.clone())
}

/// 脚本已被取消时抛出异常（供能力实现中途退出）。
pub fn throw_cancelled<'js>(ctx: &Ctx<'js>) -> rquickjs::Error {
    Exception::throw_message(ctx, "脚本已中止")
}
