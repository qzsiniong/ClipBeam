//! 远程 → 宿主机（协议 B）：xcap 轮询截屏（所有显示器）→ rqrr 解码二维码 →
//! 按 `(CRC, 总帧数)` 批次组包（发送端换数据则整批重来）→ CRC 校验 → 写本机剪贴板。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use image::imageops::{grayscale, resize, FilterType};
use image::GrayImage;
use rqrr::PreparedImage;
use xcap::Monitor;

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::protocol::{b32_decode, crc32, parse_qr_frame, QrFrame};

/// 截屏降采样后的最大宽度（像素）。
const MAX_CAPTURE_WIDTH: u32 = 1600;
/// 两轮截屏之间的间隔（截屏+解码本身耗时约 100~300ms，这里只补差额）。
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const SLEEP_STEP: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum RecvReport {
    /// 接收并写入剪贴板完成（帧数，原始文本字节数）。
    Done { frames: usize, text_bytes: usize },
    /// 用户中止（已收集帧数）。
    Cancelled { got: usize },
    /// 超时未收齐（已收集帧数）。
    Timeout { got: usize },
    /// 无法接收（原因）。
    Error(String),
}

/// 一个接收批次：同一张二维码循环里所有帧共享 (crc, total)。
struct Batch {
    crc: u32,
    total: usize,
    frames: HashMap<usize, String>,
}

impl Batch {
    fn new(f: &QrFrame) -> Self {
        let mut frames = HashMap::new();
        frames.insert(f.index, f.data.clone());
        Self {
            crc: f.crc,
            total: f.total,
            frames,
        }
    }
    /// 帧是否属于当前批次（批次键相同）。
    fn matches(&self, f: &QrFrame) -> bool {
        self.crc == f.crc && self.total == f.total
    }

    fn ingest(&mut self, f: QrFrame) {
        self.frames.entry(f.index).or_insert(f.data);
    }
    fn got(&self) -> usize {
        self.frames.len()
    }
}

/// 执行一次接收。`on_progress(got, total)` 每收到新帧触发。
pub fn run_once(
    cfg: &Config,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(usize, usize),
) -> RecvReport {
    // 先验证能取到显示器，给出可读错误（macOS 首次需要屏幕录制授权）。
    if let Err(e) = probe_monitor() {
        return RecvReport::Error(e);
    }

    let deadline = Instant::now() + cfg.receive_timeout();
    let mut batch: Option<Batch> = None;

    loop {
        if cancel.is_cancelled() {
            return RecvReport::Cancelled {
                got: batch.as_ref().map_or(0, |b| b.got()),
            };
        }
        if Instant::now() >= deadline {
            return RecvReport::Timeout {
                got: batch.as_ref().map_or(0, |b| b.got()),
            };
        }

        let round_start = Instant::now();
        // 截取所有显示器；个别屏偶发失败（休眠/拔插瞬间）跳过，全部失败才重试本轮。
        let shots = grab_all();
        if shots.is_empty() {
            cancelable_sleep(cancel, POLL_INTERVAL);
            continue;
        }

        for gray in shots {
            if cancel.is_cancelled() {
                return RecvReport::Cancelled {
                    got: batch.as_ref().map_or(0, |b| b.got()),
                };
            }
            let mut prepared = PreparedImage::prepare(gray);
            let grids = prepared.detect_grids();
            for grid in grids {
                if cancel.is_cancelled() {
                    return RecvReport::Cancelled {
                        got: batch.as_ref().map_or(0, |b| b.got()),
                    };
                }
                // 非 QR / 解码失败（画面里其他二维码、半截帧）一律忽略。
                let Ok((_, content)) = grid.decode() else {
                    continue;
                };
                let Some(frame) = parse_qr_frame(&content) else {
                    continue;
                };
                match &mut batch {
                    // guard 中不可可变借用：先用不可变 matches 判定，arm 体内再 ingest。
                    Some(b) if b.matches(&frame) => b.ingest(frame),
                    _ => batch = Some(Batch::new(&frame)),
                }
                if let Some(b) = &batch {
                    on_progress(b.got(), b.total);
                    if b.got() == b.total {
                        return finalize(cfg, b);
                    }
                }
            }
        }

        // 补足本轮目标间隔（可中止）。
        let elapsed = round_start.elapsed();
        if elapsed < POLL_INTERVAL {
            cancelable_sleep(cancel, POLL_INTERVAL - elapsed);
        }
    }
}

fn finalize(cfg: &Config, b: &Batch) -> RecvReport {
    // 按序号拼包（缺帧理论上不会发生，防御一下）。
    let mut joined = String::new();
    for i in 0..b.total {
        match b.frames.get(&i) {
            Some(chunk) => joined.push_str(chunk),
            None => return RecvReport::Error(format!("第 {i} 帧缺失，无法组包")),
        }
    }
    if crc32(joined.as_bytes()) != b.crc {
        return RecvReport::Error("组包 CRC 校验失败（请重新播放/接收）".into());
    }
    let raw = match b32_decode(&joined) {
        Ok(v) => v,
        Err(e) => return RecvReport::Error(e),
    };
    if raw.len() > cfg.max_text_bytes() {
        return RecvReport::Error(format!(
            "接收数据 {} 字节超过上限 {} KB；可在设置中调大",
            raw.len(),
            cfg.max_text_kb
        ));
    }
    let text = match String::from_utf8(raw) {
        Ok(t) => t,
        Err(e) => return RecvReport::Error(format!("UTF-8 解码失败: {e}")),
    };
    let text_bytes = text.len();

    match Clipboard::new() {
        Ok(mut cb) => {
            if let Err(e) = cb.set_text(text) {
                return RecvReport::Error(format!("写本机剪贴板失败: {e}"));
            }
        }
        Err(e) => return RecvReport::Error(format!("无法访问本机剪贴板: {e}")),
    }
    RecvReport::Done {
        frames: b.total,
        text_bytes,
    }
}

fn probe_monitor() -> Result<(), String> {
    let monitors = Monitor::all().map_err(|e| {
        format!("无法访问屏幕（macOS 请在「系统设置 → 隐私与安全 → 屏幕录制」授权后重试）: {e}")
    })?;
    if monitors.is_empty() {
        return Err("没有可用显示器".into());
    }
    Ok(())
}

/// 截取所有显示器并逐屏转灰度；每屏宽度超过 1600px 时降采样
///（最近邻，保留 QR 硬边）。
/// 个别屏截屏失败（休眠/热插拔瞬间）跳过；全部失败时返回空 Vec，
/// 由调用方在下一轮重试。每轮重新枚举显示器，自动适配运行中的热插拔。
fn grab_all() -> Vec<GrayImage> {
    let Ok(monitors) = Monitor::all() else {
        return Vec::new();
    };
    let mut images = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let Ok(rgba) = monitor.capture_image() else {
            continue;
        };
        let gray = grayscale(&rgba);
        if gray.width() > MAX_CAPTURE_WIDTH {
            let nw = MAX_CAPTURE_WIDTH;
            let nh = (gray.height() as u64 * nw as u64 / gray.width() as u64) as u32;
            images.push(resize(&gray, nw, nh, FilterType::Nearest));
        } else {
            images.push(gray);
        }
    }
    images
}

/// 可被中止令牌打断的睡眠。
fn cancelable_sleep(cancel: &CancellationToken, total: Duration) {
    let mut left = total;
    while left > Duration::ZERO {
        if cancel.is_cancelled() {
            return;
        }
        let step = SLEEP_STEP.min(left);
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{b32_encode_upper, build_qr_frame, chunk_base32};

    /// 用 Batch 直接模拟乱序、重复、换批次的收集过程。
    #[test]
    fn batch_collects_dedupes_and_switches() {
        let text = "批次组包测试 hello 🌏🌏";
        let payload = b32_encode_upper(text.as_bytes());
        let crc = crc32(payload.as_bytes());
        let chunks = chunk_base32(&payload, 8);
        let total = chunks.len();

        // 先喂一个别的批次的帧
        let other = build_qr_frame(99, 3, 0xDEAD_BEEF, "AAAA");
        let mut b = Batch::new(&parse_qr_frame(&other).unwrap());
        assert_eq!(b.total, 99);

        // 再喂本批的第一帧 → 整体切换
        let f0 = parse_qr_frame(&build_qr_frame(total, 0, crc, &chunks[0])).unwrap();
        assert!(!b.matches(&f0));
        b = Batch::new(&f0);

        // 乱序 + 重复喂入剩余帧
        for i in (1..total).rev() {
            let f = parse_qr_frame(&build_qr_frame(total, i, crc, &chunks[i])).unwrap();
            assert!(b.matches(&f));
            b.ingest(f.clone());
            b.ingest(f); // 重复帧
        }
        assert_eq!(b.got(), total);

        let cfg = Config::default();
        match finalize(&cfg, &b) {
            RecvReport::Done { frames, .. } => assert_eq!(frames, total),
            other => panic!("应完成，实际: {other:?}"),
        }
    }

    #[test]
    fn corrupted_batch_fails_crc() {
        let payload = b32_encode_upper("abc".as_bytes());
        let crc = crc32(payload.as_bytes());
        let f0 = parse_qr_frame(&build_qr_frame(1, 0, crc, &payload)).unwrap();
        let mut b = Batch::new(&f0);
        // 篡改首字符但保持批次键（crc/total）不变
        let flipped = if payload.starts_with('M') {
            format!("N{}", &payload[1..])
        } else {
            format!("M{}", &payload[1..])
        };
        b.frames.insert(0, flipped);
        let cfg = Config::default();
        assert!(matches!(finalize(&cfg, &b), RecvReport::Error(_)));
    }
}
