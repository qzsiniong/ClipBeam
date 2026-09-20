//! `$.brotli(data, level?)` / `$.unbrotli(data)`：Brotli 压缩与解压。
//!
//! 级别 0-11（默认 5）：0 基本不压、11 压得最狠但很慢。Web 场景的静态资源常用
//! 11，脚本里传的数据量一般不大，5 是「压得动又不明显卡」的折中。
//!
//! 纯 Rust 实现（`brotli` crate），不需要外部 `brotli` 命令。

use std::io::{Read, Write};

use brotli::{CompressorWriter, Decompressor};
use rquickjs::function::Opt;
use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

/// 默认压缩质量。
const DEFAULT_QUALITY: u32 = 5;

/// 内部缓冲区大小（Brotli 的推荐值）。
const BUFFER_SIZE: usize = 4096;

extension! {
    /// `$.brotli` / `$.unbrotli`。
    pub struct BrotliExtension;
    "brotli" => js_brotli,
        "brotli(data: string | ArrayBuffer | ArrayBufferView, level?: number) -> ArrayBuffer",
        "Brotli 压缩（默认质量 5，范围 0-11）";
    "unbrotli" => js_unbrotli,
        "unbrotli(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer",
        "Brotli 解压";
}

/// `brotli(data, level = 5) -> ArrayBuffer`
fn js_brotli<'js>(ctx: Ctx<'js>, data: Value<'js>, level: Opt<u32>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;
    let quality = level.0.unwrap_or(DEFAULT_QUALITY);
    if quality > 11 {
        return Err(Exception::throw_message(
            &ctx,
            &format!("brotli 压缩质量超出范围（0-11）：{quality}"),
        ));
    }

    let mut compressed = Vec::new();
    {
        // 参数依次是：输出、缓冲区、质量、窗口大小（22 是规范最大值）
        let mut writer = CompressorWriter::new(&mut compressed, BUFFER_SIZE, quality, 22);
        writer
            .write_all(&bytes)
            .map_err(|err| Exception::throw_message(&ctx, &format!("brotli 压缩失败：{err}")))?;
        // drop 时 flush 尾部；显式 flush 让错误更早暴露
        writer.flush().map_err(|err| {
            Exception::throw_message(&ctx, &format!("brotli 压缩收尾失败：{err}"))
        })?;
    }

    output_buffer(&ctx, compressed)
}

/// `unbrotli(data) -> ArrayBuffer`
fn js_unbrotli<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;

    let mut decoder = Decompressor::new(bytes.as_slice(), BUFFER_SIZE);
    let mut plain = Vec::new();
    decoder
        .read_to_end(&mut plain)
        .map_err(|err| Exception::throw_message(&ctx, &format!("brotli 解压失败：{err}")))?;

    output_buffer(&ctx, plain)
}
