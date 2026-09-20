//! `$.gzip(data, level?)` / `$.gunzip(data)`：gzip（RFC1952）压缩与解压。
//!
//! 与外部工具互通：`$.gunzip` 能解 `gzip` / `gunzip` / `tar czf` 产出的 `.gz`，
//! `$.gzip` 的产物也能被它们解开（不写额外头字段）。
//!
//! 纯 Rust 实现（`flate2` 的 `rust_backend`），不引入系统 zlib。

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rquickjs::function::Opt;
use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

/// 默认压缩级别（1 最快、9 最小，6 是 zlib 的默认值）。
const DEFAULT_LEVEL: u32 = 6;

extension! {
    /// `$.gzip` / `$.gunzip`。
    pub struct GzipExtension;
    "gzip" => js_gzip,
        "gzip(data: string | ArrayBuffer | ArrayBufferView, level?: number) -> ArrayBuffer",
        "gzip 压缩（默认级别 6，范围 0-9）";
    "gunzip" => js_gunzip,
        "gunzip(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer",
        "gzip 解压";
}

/// `gzip(data, level = 6) -> ArrayBuffer`
fn js_gzip<'js>(ctx: Ctx<'js>, data: Value<'js>, level: Opt<u32>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;
    let level = level.0.unwrap_or(DEFAULT_LEVEL);
    if level > 9 {
        return Err(Exception::throw_message(
            &ctx,
            &format!("gzip 压缩级别超出范围（0-9）：{level}"),
        ));
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(level));
    encoder
        .write_all(&bytes)
        .map_err(|err| Exception::throw_message(&ctx, &format!("gzip 压缩失败：{err}")))?;
    let compressed = encoder
        .finish()
        .map_err(|err| Exception::throw_message(&ctx, &format!("gzip 压缩收尾失败：{err}")))?;

    output_buffer(&ctx, compressed)
}

/// `gunzip(data) -> ArrayBuffer`
fn js_gunzip<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;

    let mut decoder = GzDecoder::new(bytes.as_slice());
    let mut plain = Vec::new();
    decoder
        .read_to_end(&mut plain)
        .map_err(|err| Exception::throw_message(&ctx, &format!("gzip 解压失败：{err}")))?;

    output_buffer(&ctx, plain)
}
