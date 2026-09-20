//! `console.*` 输出的落点。
//!
//! 引擎只管「这一行交给谁」，不关心对方是终端、日志面板还是测试收集器 ——
//! 这正是它唯一需要知道的"外部世界"接口。宿主交互（键盘输出、弹窗确认、文件改动确认）
//! 属于使用方，不在这里（见 crate 文档的「引擎边界」）。
//!
//! ## 传给引擎的方式
//!
//! 以 `Ctx::store_userdata` 存进上下文，`prelude.js` 里的 `console.*` 经
//! `__primitives.print` 桥到 [`ConsoleHook::write`]。不往 JS 侧暴露任何对象。

use std::sync::Arc;

use rquickjs::JsLifetime;

/// 脚本 `console.*` 一行输出的落点。
///
/// 实现必须是 `Send + Sync`：可能从引擎的任务线程上被调用。
pub trait ConsoleHook: Send + Sync + 'static {
    /// 写一行。
    ///
    /// * `level` ∈ `{"log", "info", "debug", "warn", "error"}`；
    /// * `text` 已由 `prelude.js` 格式化好（字符串原样，其余值走 `JSON.stringify`）。
    fn write(&self, level: &str, text: &str);
}

/// 默认落点：`warn` / `error` 写 stderr，其余写 stdout（逐行 flush）。
///
/// CLI、headless、测试都用它 —— 「不接 UI 也能看到输出」。
pub struct StdoutConsole;

impl ConsoleHook for StdoutConsole {
    fn write(&self, level: &str, text: &str) {
        use std::io::Write;

        // 输出错误不值得打断脚本，一路忽略
        match level {
            "warn" | "error" => {
                let mut stderr = std::io::stderr();
                let _ = writeln!(stderr, "{text}");
                let _ = stderr.flush();
            }
            _ => {
                let mut stdout = std::io::stdout();
                let _ = writeln!(stdout, "{text}");
                let _ = stdout.flush();
            }
        }
    }
}

/// 什么都不做：嵌入式/静默运行用。
pub struct NoopConsole;

impl ConsoleHook for NoopConsole {
    fn write(&self, _level: &str, _text: &str) {}
}

/// 上下文里保存的控制台钩子（`Ctx` userdata 的载体类型）。
///
/// 只装一个 `Arc`，不含任何 JS 值，因此可以安全地实现 [`JsLifetime`]。
pub(crate) struct ConsoleRef(pub Arc<dyn ConsoleHook>);

// SAFETY: `ConsoleRef` 不含任何带 `'js` 生命周期的 JS 值，与上下文生命周期无关。
unsafe impl<'js> JsLifetime<'js> for ConsoleRef {
    type Changed<'to> = ConsoleRef;
}
