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

    /// 暴露底层标志位（共享同一个原子变量）。
    ///
    /// 用途：把「应用自己的取消信号」桥接到脚本引擎的令牌上 ——
    /// `script_runner::engine_cancel()` 用它把侧线程变成同一个信号源，
    /// 这样 Esc 中止既能让 typer 停手，也能让 `sleep` / 能力检查立刻返回。
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}
