//! 键盘模拟封装：逐字符输入，每键前查取消令牌、键间可配延迟。
//! send（协议 A）与 deploy（协议 C 引导包）共用。
//!
//! macOS：使用 CoreGraphics 的 CGEventKeyboardSetUnicodeString 直接发送
//! Unicode 按键事件，不依赖 TSM 键盘布局查询（可在任意线程调用，避免
//! enigo 的 `dispatch_assert_queue_fail` 崩溃）。
//! 其他平台：使用 enigo。

use enigo::{Direction, Key, Keyboard};
use log::debug;

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::keymap::{get_key_info, KeyAction};
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
    // macOS: ManuallyDrop 跳过 enigo 的 Drop——其 Drop 内有累积 sleep 逻辑
    //（每次按键 update_wait_time 累加 20ms，长文本 Drop 时会阻塞数秒甚至数分钟）。
    #[cfg(target_os = "macos")]
    enigo: std::mem::ManuallyDrop<enigo::Enigo>,
    #[cfg(not(target_os = "macos"))]
    enigo: enigo::Enigo,
}

impl Typer {
    pub fn new(cfg: &Config, cancel: CancellationToken) -> Result<Self, String> {
        let enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| format!("无法初始化键盘模拟，请检查系统的辅助功能/输入监控权限: {e}"))?;
        Ok(Self {
            delay: cfg.key_delay(),
            cancel,
            #[cfg(target_os = "macos")]
            enigo: std::mem::ManuallyDrop::new(enigo),
            #[cfg(not(target_os = "macos"))]
            enigo,
        })
    }

    /// 等待指定延迟。
    fn wait_delay(&self) {
        if self.delay > Duration::ZERO {
            std::thread::sleep(self.delay);
        }
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

    /// 模拟输入单个字符。
    ///
    /// 查 keymap 得基础字符与是否需 Shift；大写字母 / 特殊符号通过显式 Shift
    /// press/release 实现。未知字符（如非 ASCII）回退 `text()` Unicode 输入。
    pub fn send_char(&mut self, ch: char, dry_run: bool) -> Result<(), String> {
        debug!("send_char {:?}, dry_run: {}", ch, dry_run);
        match get_key_info(ch) {
            Some(KeyAction::Char {
                #[allow(unused_variables)]
                base,
                shift,
                #[allow(unused_variables)]
                mac_keycode,
            }) => {
                if shift {
                    if !dry_run {
                        let _ = self.enigo.key(Key::Shift, Direction::Press);
                    }
                    self.wait_delay();
                }
                #[cfg(target_os = "macos")]
                {
                    // macOS 直接用 raw keycode，绕过 enigo 的 get_layoutdependent_keycode
                    // （后者遍历不 break，小键盘 `.`keycode=65 覆盖主键盘 47，导致 `>`→`.`）
                    if !dry_run {
                        let _ = self.enigo.raw(mac_keycode, Direction::Click);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    if !dry_run {
                        self.enigo.key(Key::Unicode(base), Direction::Click);
                    }
                }
                if shift {
                    self.wait_delay();
                    if !dry_run {
                        let _ = self.enigo.key(Key::Shift, Direction::Release);
                    }
                }
            }
            Some(KeyAction::Return) => {
                if !dry_run {
                    let _ = self.enigo.key(Key::Return, Direction::Click);
                }
            }
            Some(KeyAction::Tab) => {
                if !dry_run {
                    let _ = self.enigo.key(Key::Tab, Direction::Click);
                }
            }
            None => {
                // 非 ASCII / 未知字符：回退 Unicode 文本输入
                if !dry_run {
                    let _ = self.enigo.text(&ch.to_string());
                }
            }
        }
        Ok(())
    }

    /// 逐字符输入。开始前与每个字符前都检查取消令牌，
    /// 因此最坏停止延迟约等于一个键间隔（默认 3ms）。
    /// `on_progress(sent, total)` 在每个字符发送后触发，可用于进度展示。
    pub fn type_str(
        &mut self,
        text: &str,
        on_progress: &mut impl FnMut(usize, usize),
    ) -> TypeResult {
        let total = text.chars().count();
        for (i, c) in text.chars().enumerate() {
            if self.cancel.is_cancelled() {
                return TypeResult::Cancelled(i);
            }
            if let Err(e) = self.send_char(c, false) {
                return TypeResult::Failed(i, format!("键盘事件发送失败: {e}"));
            }
            on_progress(i + 1, total);
            self.wait_delay();
        }
        TypeResult::Completed(total)
    }
}

impl Drop for Typer {
    fn drop(&mut self) {
        // 释放可能残留的 Shift（任务中途取消时 Shift 可能处于按下状态）
        let _ = self.enigo.key(Key::Shift, Direction::Release);
        // macOS: enigo 包在 ManuallyDrop 中，其 Drop（含累积 sleep）不会运行
        // 其他平台: enigo 正常 Drop
    }
}
