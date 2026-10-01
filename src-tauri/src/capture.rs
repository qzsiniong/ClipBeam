//! 屏幕抓取与二维码解码：协议 B（二维码回剪贴板）与 `$.scan_qr()`
//! （脚本读回远程屏幕上的二维码）共用的底层。
//!
//! 之所以抽出来：这两条路径的抓取语义必须**完全一致** —— 都是「枚举所有显示器 →
//! 逐屏抓取 → 转灰度 → 超宽降采样」，否则同一张二维码在一个通道能解、在另一个
//! 通道解不出来，用户会以为是协议问题。
//!
//! 本模块只做「抓到图」与「解出一段文本」，不理解任何协议：返回的就是二维码里的
//! 原始字符串，怎么解析由调用方决定（协议 B 解成 `CB1.…` 帧，脚本解成它自己的状态）。

use image::imageops::{grayscale, resize, FilterType};
use image::GrayImage;
use rqrr::PreparedImage;
use xcap::Monitor;

/// 截屏降采样后的最大宽度（像素）。
///
/// 二维码是硬边图案，缩到 1600px 仍能可靠解码，而解码耗时随像素数增长 ——
/// 大屏上不降采样会让每次抓取慢好几倍。
pub const MAX_CAPTURE_WIDTH: u32 = 1600;

/// 截取所有显示器并逐屏转灰度；每屏宽度超过 [`MAX_CAPTURE_WIDTH`] 时降采样
///（最近邻，保留 QR 硬边）。
///
/// 个别屏截屏失败（休眠/热插拔瞬间）跳过；全部失败时返回空 `Vec`，
/// 由调用方在下一轮重试。**每轮重新枚举显示器**，自动适配运行中的热插拔。
pub fn grab_all_gray() -> Vec<GrayImage> {
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

/// 从灰度图里解出**第一条**能解出来的二维码文本。
///
/// 画面里可能同时有多个二维码（远程页面自己也会显示协议 B 的出站码），所以本函数
/// 只保证「返回一个二维码的文本」，不保证是哪一个 —— 调用方应当自己判断拿到的内容
/// 是不是它在等的（协议 B 用 `CB1.` 前缀过滤，脚本用状态 JSON 过滤）。
pub fn decode_qr_text(gray: GrayImage) -> Option<String> {
    let mut prepared = PreparedImage::prepare(gray);
    for grid in prepared.detect_grids() {
        if let Ok((_meta, content)) = grid.decode() {
            return Some(content);
        }
    }
    None
}
