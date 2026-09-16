//! 宿主机 → 远程：读本机剪贴板文本 → 组键盘帧 → enigo 逐键发送（协议 A）。

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::typer::{TypeResult, Typer};

#[derive(Debug)]
pub enum SendReport {
    /// 发送完成（帧字符数，原始文本字节数）。
    Done {
        frame_chars: usize,
        text_bytes: usize,
    },
    /// 用户中止（已发出字符数）。
    Cancelled { sent: usize },
    /// 无法发送（原因）。
    Error(String),
}

/// 执行一次发送。
///
/// 前置条件：触发时用户已把焦点切到远程页面。函数内仅等待 `settle` 让热键
/// 修饰键抬起，不做任何焦点切换。
/// `on_progress(sent, total)` 在每个字符发送后触发。
pub fn run_once(
    cfg: &Config,
    cancel: &CancellationToken,
    settle: bool,
    mut on_progress: impl FnMut(usize, usize),
) -> SendReport {
    // 1. 读剪贴板
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => return SendReport::Error(format!("无法访问本机剪贴板: {e}")),
    };
    let text = match clipboard.get_text() {
        Ok(t) => t,
        Err(_) => {
            return SendReport::Error(
                "剪贴板内容不是文本（ClipBeam v1 仅支持文本，图片/富文本请先转成文本）".into(),
            )
        }
    };
    if text.is_empty() {
        return SendReport::Error("剪贴板为空".into());
    }
    if text.len() > cfg.max_text_bytes() {
        return SendReport::Error(format!(
            "文本 {} 字节超过上限 {} KB（键盘通道耗时与长度成正比）；可在设置中调大上限",
            text.len(),
            cfg.max_text_kb
        ));
    }

    // 2. 组帧（按配置决定是否启用 zstd 压缩）
    let frame = if cfg.compress {
        crate::protocol::build_keyboard_frame_compressed(&text)
    } else {
        crate::protocol::build_keyboard_frame(&text)
    };
    let frame_chars = frame.chars().count();

    // 3. 等待修饰键抬起（可被中止）
    if settle && !Typer::wait_settle(cfg, cancel) {
        return SendReport::Cancelled { sent: 0 };
    }

    // 4. 逐键发送
    let mut typer = match Typer::new(cfg, cancel.clone()) {
        Ok(t) => t,
        Err(e) => return SendReport::Error(e),
    };
    match typer.type_str(&frame, &mut on_progress) {
        TypeResult::Completed(_) => SendReport::Done {
            frame_chars,
            text_bytes: text.len(),
        },
        TypeResult::Cancelled(sent) => SendReport::Cancelled { sent },
        TypeResult::Failed(sent, e) => SendReport::Error(format!(
            "{e}（已发出约 {sent} 个字符，远程页面可能处于半截帧，10 秒无后续会自动复位）"
        )),
    }
}
