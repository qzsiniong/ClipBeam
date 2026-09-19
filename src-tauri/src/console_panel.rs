//! 脚本 Console 面板的后端：有上限的环形缓冲 + 事件推送。
//!
//! # 为什么放在后端而不是前端
//!
//! * 脚本窗口可能被关掉再打开：日志留在后端，重开后面板还能恢复；
//! * 脚本疯狂打印时，前端内存不受控 —— 缓冲在这里截断到 [`ConsoleBuffer::CAPACITY`]；
//! * 输出不只来自 `console.*`（还有运行开始/结束、脚本异常、任务中止），
//!   统一由后端按同一格式写进同一个流。
//!
//! # 事件协议
//!
//! | 事件 | payload | 说明 |
//! |---|---|---|
//! | `script-console` | [`ConsoleLine`] | 新增一行 |
//! | `script-console-clear` | 无 | 缓冲被清空（面板挂载时也会用 `get_script_console` 取全量） |

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

/// 一行脚本控制台输出（前端渲染单位，也是事件 payload）。
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ConsoleLine {
    /// 单调递增序号：前端可以据此排序/去重，清空后继续递增（不重置）。
    pub seq: u64,
    /// `log` / `info` / `debug` / `warn` / `error`（另有后端写入的运行状态行沿用同名字段）。
    pub level: String,
    /// 已格式化的文本（脚本侧 `console.*` 已由 `prelude.js` 处理成字符串）。
    pub text: String,
    /// 写入时间（epoch 毫秒）。
    pub ts: u64,
}

/// 把一行输出送到前端（便于单测替换，不必启动 Tauri 运行时）。
pub type ConsoleEmitter = Arc<dyn Fn(&str, &ConsoleLine) + Send + Sync>;

/// 有上限的脚本控制台缓冲。
pub struct ConsoleBuffer {
    lines: Mutex<VecDeque<ConsoleLine>>,
    next_seq: AtomicU64,
    emitter: ConsoleEmitter,
}

impl ConsoleBuffer {
    /// 最多保留多少行：脚本狂打印时截断，保证内存与渲染都有界。
    pub const CAPACITY: usize = 1000;

    /// 用 AppHandle 建一个缓冲（事件直接 `emit` 给所有窗口）。
    pub fn new(app: AppHandle) -> Arc<Self> {
        Self::with_emitter(Arc::new(move |event, line| {
            let _ = app.emit(event, line.clone());
        }))
    }

    /// 用自定义 emitter 建一个缓冲（单测用）。
    pub fn with_emitter(emitter: ConsoleEmitter) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(VecDeque::with_capacity(64)),
            next_seq: AtomicU64::new(1),
            emitter,
        })
    }

    /// 追加一行并推送给前端。
    ///
    /// 超出 [`ConsoleBuffer::CAPACITY`] 时淘汰最旧的行（`seq` 不回溯）。
    pub fn push(&self, level: &str, text: &str) {
        let line = ConsoleLine {
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            level: level.to_string(),
            text: text.to_string(),
            ts: now_ms(),
        };

        {
            let mut lines = self.lines.lock().unwrap();
            lines.push_back(line.clone());
            while lines.len() > Self::CAPACITY {
                lines.pop_front();
            }
        }

        (self.emitter)("script-console", &line);
    }

    /// 取当前全部行（脚本窗口挂载或重新打开时用）。
    pub fn snapshot(&self) -> Vec<ConsoleLine> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    /// 清空缓冲并广播。
    pub fn clear(&self) {
        self.lines.lock().unwrap().clear();
        // 事件没有 payload，用一行空内容承载（前端只认事件名）
        let marker = ConsoleLine {
            seq: self.next_seq.load(Ordering::Relaxed),
            level: "info".to_string(),
            text: String::new(),
            ts: now_ms(),
        };
        (self.emitter)("script-console-clear", &marker);
    }
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的事件收集器：(收到的 (事件名, 行), emitter)。
    type Collected = Arc<Mutex<Vec<(String, ConsoleLine)>>>;

    /// 收集事件的测试 emitter。
    fn collector() -> (Collected, ConsoleEmitter) {
        let events: Collected = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let emitter: ConsoleEmitter = Arc::new(move |event, line| {
            sink.lock().unwrap().push((event.to_string(), line.clone()));
        });
        (events, emitter)
    }

    #[test]
    fn push_keeps_order_and_emits_events() {
        let (events, emitter) = collector();
        let buffer = ConsoleBuffer::with_emitter(emitter);

        buffer.push("log", "一");
        buffer.push("error", "二");

        let lines = buffer.snapshot();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "一");
        assert_eq!(lines[0].level, "log");
        assert_eq!(lines[1].text, "二");
        assert!(lines[0].seq < lines[1].seq, "seq 应当递增");

        let received = events.lock().unwrap().clone();
        assert_eq!(received.len(), 2);
        assert!(received.iter().all(|(event, _)| event == "script-console"));
    }

    #[test]
    fn capacity_drops_oldest_lines() {
        let (_, emitter) = collector();
        let buffer = ConsoleBuffer::with_emitter(emitter);

        let total = ConsoleBuffer::CAPACITY + 25;
        for index in 0..total {
            buffer.push("log", &format!("line-{index}"));
        }

        let lines = buffer.snapshot();
        assert_eq!(lines.len(), ConsoleBuffer::CAPACITY, "应当截断到容量上限");
        assert_eq!(
            lines[0].text,
            format!("line-{}", total - ConsoleBuffer::CAPACITY),
            "应当淘汰最旧的行"
        );
        assert_eq!(lines.last().unwrap().text, format!("line-{}", total - 1));
    }

    #[test]
    fn clear_empties_buffer_but_keeps_seq_increasing() {
        let (events, emitter) = collector();
        let buffer = ConsoleBuffer::with_emitter(emitter);

        buffer.push("log", "旧");
        let before = buffer.snapshot()[0].seq;
        buffer.clear();
        assert!(buffer.snapshot().is_empty(), "清空后不应有残留");
        buffer.push("log", "新");

        let after = buffer.snapshot()[0].seq;
        assert!(
            after > before,
            "seq 必须继续递增（前端据此去重）：{before} -> {after}"
        );

        let cleared: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|(event, _)| event == "script-console-clear")
            .map(|(event, _)| event.clone())
            .collect();
        assert_eq!(cleared.len(), 1, "清空应当广播一次事件");
    }
}
