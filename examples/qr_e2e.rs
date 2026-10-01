//! E2E（离线部分）：解码 qrcode-generator 生成的二维码 GIF 帧 → 按批次组包 →
//! 整条消息 CRC 校验 → 还原文本。对应 Q 通道的 `QBA`（剪贴板）消息。
//! 用法：cargo run --example qr_e2e -- /tmp/cbqr/*.gif
//! 参数顺序任意（模拟截屏时帧乱序出现）。
#![allow(dead_code)]
#[path = "../src-tauri/src/protocol.rs"]
mod protocol;

use std::collections::HashMap;
use std::process::ExitCode;

use image::imageops::grayscale;
use rqrr::PreparedImage;

use protocol::{b32_decode, parse_q_frame, q_payload_crc, Q_MAGIC_CLIP};

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("没有输入帧");
        return ExitCode::FAILURE;
    }

    // 批次键 = (crc, total)，模拟 receive.rs 的收集过程；crc 是**整条消息**的摘要。
    let mut batch: Option<(String, usize, HashMap<usize, String>)> = None;
    let mut decoded_frames = 0;

    for path in &paths {
        let rgba = match image::open(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                eprintln!("打开 {path} 失败: {e}");
                return ExitCode::FAILURE;
            }
        };
        let gray = grayscale(&rgba);
        let mut prepared = PreparedImage::prepare(gray);
        let grids = prepared.detect_grids();
        for grid in grids {
            let Ok((_, content)) = grid.decode() else {
                continue;
            };
            let Some(f) = parse_q_frame(&content) else {
                continue;
            };
            // 只关心剪贴板载荷；控制消息是文件传输的反馈，不走这条路
            if f.magic != Q_MAGIC_CLIP {
                continue;
            }
            decoded_frames += 1;
            match &mut batch {
                Some((crc, total, map)) if *crc == f.crc && *total == f.total => {
                    map.entry(f.index).or_insert(f.payload);
                }
                _ => {
                    let mut map = HashMap::new();
                    map.insert(f.index, f.payload);
                    batch = Some((f.crc, f.total, map));
                }
            }
        }
    }

    let Some((crc, total, map)) = batch else {
        eprintln!("没有解码出任何 ClipBeam 帧");
        return ExitCode::FAILURE;
    };
    println!("解码 {decoded_frames} 帧，批次 {}/{}", map.len(), total);
    if map.len() != total {
        eprintln!("缺帧：{}/{}", map.len(), total);
        return ExitCode::FAILURE;
    }
    let mut joined = String::new();
    for i in 0..total {
        joined.push_str(map.get(&i).unwrap());
    }
    if q_payload_crc(&joined) != crc {
        eprintln!("整条消息 CRC 校验失败");
        return ExitCode::FAILURE;
    }
    let raw = b32_decode(&joined).expect("base32");
    let text = String::from_utf8(raw).expect("utf8");
    println!("✓ 组包成功：{} 字节", text.len());
    println!("--- 文本预览（前 120 字符）---");
    println!("{}", text.chars().take(120).collect::<String>());
    ExitCode::SUCCESS
}
