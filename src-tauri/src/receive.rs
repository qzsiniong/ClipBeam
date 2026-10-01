//! 远程 → 宿主机（Q 通道的 `QBA` 剪贴板消息）：xcap 轮询截屏（所有显示器）→
//! rqrr 解码二维码 → 按批次键 `(magic, total, crc)` 组包 → 整体 CRC 校验 →
//! 写本机剪贴板。
//!
//! 只处理 `QBA`（剪贴板载荷）；`QBB`（控制消息，文件传输的反馈）由脚本的
//! `$.scan_qr()` 自己解析，不走这里。
//!
//! 批次键里的 `crc` 是**整条消息**的摘要（同批各帧一致），所以「换了一批数据」
//! 表现为 crc 变化 → 整批重来。若重组后 CRC 不符（某帧解错），这里**丢弃该批并
//! 继续扫描**，而不是报错退出 —— 页面在轮播，下一轮就能补齐。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use rqrr::PreparedImage;

use crate::cancel::CancellationToken;
use crate::capture::grab_all_gray;
use crate::config::Config;
use crate::protocol::{b32_decode, parse_q_frame, QFrame, Q_MAGIC_CLIP};

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

/// 一个接收批次：同一组轮播二维码里所有帧共享 `(magic, total, crc)`。
struct Batch {
    crc: String,
    total: usize,
    frames: HashMap<usize, String>,
}

impl Batch {
    fn new(f: &QFrame) -> Self {
        let mut frames = HashMap::new();
        frames.insert(f.index, f.payload.clone());
        Self {
            crc: f.crc.clone(),
            total: f.total,
            frames,
        }
    }

    /// 帧是否属于当前批次（批次键相同）。
    fn matches(&self, f: &QFrame) -> bool {
        self.crc == f.crc && self.total == f.total
    }

    fn ingest(&mut self, f: QFrame) {
        self.frames.entry(f.index).or_insert(f.payload);
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
        let shots = grab_all_gray();
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
                let Some(frame) = parse_q_frame(&content) else {
                    continue;
                };
                // 只收剪贴板载荷；控制消息（QBB）是脚本那边的反馈通道
                if frame.magic != Q_MAGIC_CLIP {
                    continue;
                }
                match &mut batch {
                    // guard 中不可可变借用：先用不可变 matches 判定，arm 体内再 ingest。
                    Some(b) if b.matches(&frame) => b.ingest(frame),
                    _ => batch = Some(Batch::new(&frame)),
                }
                if let Some(b) = &batch {
                    on_progress(b.got(), b.total);
                    if b.got() == b.total {
                        match finalize(cfg, b) {
                            Finalize::Done(report) => return report,
                            // 这批有坏帧：丢掉重来，页面还在轮播
                            Finalize::Discard => {
                                batch = None;
                                on_progress(0, 0);
                            }
                        }
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

/// `finalize` 的结局。
#[derive(Debug)]
enum Finalize {
    /// 已经写进剪贴板，可以收工。
    Done(RecvReport),
    /// 这批数据有问题（缺帧/CRC 不符）：**丢弃并继续扫描**，等下一轮轮播补齐。
    ///
    /// 不直接报错退出是有意的：二维码在页面上循环播放，某帧偶尔解码失败是常态，
    /// 让用户重按热键才能恢复就说不过去了。
    Discard,
}

fn finalize(cfg: &Config, b: &Batch) -> Finalize {
    // 按序号拼包（缺帧在「还没收齐」之外理论上不会发生，防御一下）。
    let mut joined = String::new();
    for i in 0..b.total {
        match b.frames.get(&i) {
            Some(chunk) => joined.push_str(chunk),
            None => return Finalize::Discard,
        }
    }
    // crc 是整条消息的摘要，在这里一次验完
    if crate::protocol::q_payload_crc(&joined) != b.crc {
        return Finalize::Discard;
    }
    let raw = match b32_decode(&joined) {
        Ok(v) => v,
        Err(e) => return Finalize::Done(RecvReport::Error(e)),
    };
    if raw.len() > cfg.max_text_bytes() {
        return Finalize::Done(RecvReport::Error(format!(
            "接收数据 {} 字节超过上限 {} KB；可在设置中调大",
            raw.len(),
            cfg.max_text_kb
        )));
    }
    let text = match String::from_utf8(raw) {
        Ok(t) => t,
        Err(e) => return Finalize::Done(RecvReport::Error(format!("UTF-8 解码失败: {e}"))),
    };
    let text_bytes = text.len();

    match Clipboard::new() {
        Ok(mut cb) => {
            if let Err(e) = cb.set_text(text) {
                return Finalize::Done(RecvReport::Error(format!("写本机剪贴板失败: {e}")));
            }
        }
        Err(e) => return Finalize::Done(RecvReport::Error(format!("无法访问本机剪贴板: {e}"))),
    }
    Finalize::Done(RecvReport::Done {
        frames: b.total,
        text_bytes,
    })
}

/// 接收前的可读性预检：拿不到屏幕时给出授权提示，而不是让用户在超时里干等。
///
/// 用 `Monitor::all()` 直接探测（而不是抓一次图）：这里只要「能不能枚举到显示器」，
/// 抓图成功与否是下一轮循环的容错范围。
fn probe_monitor() -> Result<(), String> {
    let monitors = xcap::Monitor::all().map_err(|e| {
        format!("无法访问屏幕（macOS 请在「系统设置 → 隐私与安全 → 屏幕录制」授权后重试）: {e}")
    })?;
    if monitors.is_empty() {
        return Err("没有可用显示器".into());
    }
    Ok(())
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
    use crate::protocol::{b32_encode_upper, build_q_frame, parse_q_frame, q_payload_crc};

    /// 按字符切分整条 payload（与页面的分帧方式一致）。
    fn chunk_str(s: &str, size: usize) -> Vec<String> {
        s.as_bytes()
            .chunks(size)
            .map(|c| String::from_utf8(c.to_vec()).unwrap())
            .collect()
    }

    /// 组一条完整消息的全部帧（同批各帧共享同一个 crc）。
    fn frames_of(text: &str, chunk: usize) -> (String, Vec<String>) {
        let payload = b32_encode_upper(text.as_bytes());
        let crc = q_payload_crc(&payload);
        let chunks = chunk_str(&payload, chunk);
        let total = chunks.len();
        let frames = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| build_q_frame(Q_MAGIC_CLIP, total, i, &crc, c))
            .collect();
        (crc, frames)
    }

    /// 乱序、重复、以及换批次的收集过程。
    #[test]
    fn batch_collects_dedupes_and_switches() {
        let text = "批次组包测试 hello 🌏🌏";
        let (_, frames) = frames_of(text, 8);
        let total = frames.len();
        assert!(total > 1, "测试数据应当切成多帧");

        // 先喂一个别的批次的帧（另一个 total + 另一个 crc）
        let other = build_q_frame(Q_MAGIC_CLIP, 99, 0, "ABCDEFG", "AAAA");
        let mut b = Batch::new(&parse_q_frame(&other).unwrap());
        assert_eq!(b.total, 99);

        // 再喂本批的第一帧 → 整批切换
        let f0 = parse_q_frame(&frames[0]).unwrap();
        assert!(!b.matches(&f0));
        b = Batch::new(&f0);

        // 乱序 + 重复喂入剩余帧
        for i in (1..total).rev() {
            let f = parse_q_frame(&frames[i]).unwrap();
            assert!(b.matches(&f));
            b.ingest(f.clone());
            b.ingest(f); // 重复帧
        }
        assert_eq!(b.got(), total);

        let cfg = Config::default();
        match finalize(&cfg, &b) {
            Finalize::Done(RecvReport::Done { frames, .. }) => assert_eq!(frames, total),
            other => panic!("应完成，实际: {other:?}"),
        }
    }

    /// 某帧内容被改坏（但批次键不变）→ 整体 CRC 不符 → **丢弃该批**，而不是报错退出。
    #[test]
    fn corrupted_batch_is_discarded_not_an_error() {
        let (_, frames) = frames_of("abc", 8);
        let mut b = Batch::new(&parse_q_frame(&frames[0]).unwrap());

        // 篡改该帧的 payload，保持 (total, crc) 不变
        let original = b.frames.get(&0).unwrap().clone();
        let flipped = if let Some(stripped) = original.strip_prefix('M') {
            format!("N{}", stripped)
        } else {
            format!("M{}", &original[1..])
        };
        b.frames.insert(0, flipped);

        let cfg = Config::default();
        assert!(
            matches!(finalize(&cfg, &b), Finalize::Discard),
            "坏批应当被丢弃以便重扫"
        );
    }

    /// 缺帧时也不能当成完成。
    #[test]
    fn incomplete_batch_is_discarded() {
        let (_, frames) = frames_of("abcdefghijklmnop", 4);
        let mut b = Batch::new(&parse_q_frame(&frames[0]).unwrap());
        b.total = frames.len(); // 人为声明更多帧但只喂一帧
        let cfg = Config::default();
        assert!(matches!(finalize(&cfg, &b), Finalize::Discard));
    }
}
