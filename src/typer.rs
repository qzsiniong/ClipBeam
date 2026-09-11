//! 键盘模拟封装：逐字符输入，每键前查取消令牌、键间可配延迟。
//! send（协议 A）与 deploy（协议 C 引导包）共用。
//!
//! macOS：使用 CoreGraphics 的 CGEventKeyboardSetUnicodeString 直接发送
//! Unicode 按键事件，不依赖 TSM 键盘布局查询（可在任意线程调用，避免
//! enigo 的 `dispatch_assert_queue_fail` 崩溃）。
//! 其他平台：使用 enigo。

use crate::cancel::CancellationToken;
use crate::config::Config;
use std::time::Duration;

#[derive(Debug)]
pub enum TypeResult {
    /// 全部输完（已尝试的字符数）。
    Completed(usize),
    /// 被中止（已尝试的字符数；可能已落入当前焦点窗口）。
    Cancelled(usize),
    /// 模拟出错（已尝试字符数，错误信息）。
    Failed(usize, String),
}

pub struct Typer {
    delay: Duration,
    cancel: CancellationToken,
    #[cfg(not(target_os = "macos"))]
    enigo: enigo::Enigo,
}

impl Typer {
    pub fn new(cfg: &Config, cancel: CancellationToken) -> Result<Self, String> {
        #[cfg(not(target_os = "macos"))]
        let enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| format!("无法初始化键盘模拟，请检查系统的辅助功能/输入监控权限: {e}"))?;
        Ok(Self {
            delay: cfg.key_delay(),
            cancel,
            #[cfg(not(target_os = "macos"))]
            enigo,
        })
    }

    /// 等待热键修饰键抬起；期间可被中止。返回 false 表示已被取消。
    pub fn wait_settle(cfg: &Config, cancel: &CancellationToken) -> bool {
        let step = Duration::from_millis(10);
        let mut waited = Duration::ZERO;
        while waited < cfg.settle() {
            if cancel.is_cancelled() {
                return false;
            }
            std::thread::sleep(step);
            waited += step;
        }
        true
    }

    /// 发送单个字符的按键事件（按下+抬起）。
    #[cfg(target_os = "macos")]
    fn send_char(&self, c: char) -> Result<(), String> {
        use core_graphics::event::{CGEvent, CGEventTapLocation};
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
        // char → UTF-16（处理 BMP 外字符的代理对）
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|_| "创建 CGEventSource 失败".to_string())?;
        let key_down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "创建 CGEvent 失败".to_string())?;
        key_down.set_string_from_utf16_unchecked(units);
        key_down.post(CGEventTapLocation::HID);
        let key_up = CGEvent::new_keyboard_event(source, 0, false)
            .map_err(|_| "创建 CGEvent 失败".to_string())?;
        key_up.set_string_from_utf16_unchecked(units);
        key_up.post(CGEventTapLocation::HID);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn send_char(&mut self, c: char) -> Result<(), String> {
        use enigo::{Direction, Key, Keyboard};
        self.enigo
            .key(Key::Unicode(c), Direction::Click)
            .map_err(|e| e.to_string())
    }

    /// 逐字符输入。开始前与每个字符前都检查取消令牌，
    /// 因此最坏停止延迟约等于一个键间隔（默认 3ms）。
    pub fn type_str(&mut self, text: &str) -> TypeResult {
        let total = text.chars().count();
        for (i, c) in text.chars().enumerate() {
            if self.cancel.is_cancelled() {
                return TypeResult::Cancelled(i);
            }
            if let Err(e) = self.send_char(c) {
                return TypeResult::Failed(i, format!("键盘事件发送失败: {e}"));
            }
            // 最后一个字符不必再睡
            if i + 1 < total && self.delay > Duration::ZERO {
                std::thread::sleep(self.delay);
            }
        }
        TypeResult::Completed(total)
    }
}
