//! 协作式取消信号。
//!
//! 一个跨线程共享的「是否已被要求停下」的标记。存在的唯一理由是**让长时间等待可以被中断**：
//! `$.sleep(3000)` 会把它切成 50ms 一片逐片检查，取消后立即返回；扩展也能在动手前查一次
//! （`bindings::cancelled`）并在取消时抛「脚本已中止」（`bindings::throw_cancelled`）。
//!
//! 它有内置标志，也可以挂一个**外部判断函数**：使用方通常已经有自己的中止开关
//! （例如宿主应用的 Esc 中止令牌），不必在引擎里再维护一份 —— 「自己取消或被外部取消」都算取消。

use std::sync::Arc;

use rquickjs::JsLifetime;

/// 上下文里保存的取消信号（`Ctx` userdata 的载体类型）。
pub(crate) struct CancelRef(pub CancelSignal);

// SAFETY: `CancelRef` 只是一个原子布尔加一个回调，不含任何带 `'js` 生命周期的 JS 值。
unsafe impl<'js> JsLifetime<'js> for CancelRef {
    type Changed<'to> = CancelRef;
}

/// 协作式取消信号。
#[derive(Clone, Default)]
pub struct CancelSignal {
    flag: Arc<std::sync::atomic::AtomicBool>,
    external: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl CancelSignal {
    /// 创建一个未取消的信号。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建一个「同时受外部信号控制」的信号。
    ///
    /// `external` 返回 `true` 即视为已取消；它与内置标志是**或**的关系。
    pub fn watching(external: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            external: Some(Arc::new(external)),
        }
    }

    /// 标记为已取消（幂等）。
    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否已取消（内置标志或外部信号任一为真）。
    pub fn is_cancelled(&self) -> bool {
        if self.flag.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        self.external.as_ref().is_some_and(|external| external())
    }

    /// 建一个与 `self` 共享状态的句柄（取消任一即全部取消）。
    pub fn handle(&self) -> Self {
        self.clone()
    }
}

impl std::fmt::Debug for CancelSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelSignal")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn cancel_is_idempotent_and_shared_with_handles() {
        let signal = CancelSignal::new();
        let handle = signal.handle();
        assert!(!signal.is_cancelled());
        handle.cancel();
        assert!(signal.is_cancelled(), "共享状态：任一句柄取消即全部取消");
        signal.cancel();
        assert!(signal.is_cancelled());
    }

    #[test]
    fn external_signal_is_ored_with_the_internal_flag() {
        let external = Arc::new(AtomicBool::new(false));
        let flag = external.clone();
        let signal = CancelSignal::watching(move || flag.load(Ordering::SeqCst));

        assert!(!signal.is_cancelled());
        external.store(true, Ordering::SeqCst);
        assert!(signal.is_cancelled(), "外部信号为真即视为取消");
    }
}
