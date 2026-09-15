//! 任务取消令牌：typer 逐键、截屏循环逐轮检查，保证中止热键触发后尽快停下。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}
