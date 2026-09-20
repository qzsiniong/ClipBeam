//! `$.lzma(data)` / `$.unlzma(data)`：xz（LZMA2）压缩与解压。
//!
//! 用 **xz 容器**（不是裸 `.lzma`）：`xz` / `7z` / `tar -J` 都能直接解开，
//! 也自带完整性校验。纯 Rust 实现（`lzma-rs`），不链接 liblzma。
//!
//! ```js
//! const packed = $.lzma(await $.read("big.log"))
//! await $.write("big.log.xz", packed)
//! ```
//!
//! 说明：`lzma-rs` 的 `xz_compress` 不接受压缩级别参数（内部用默认预设），
//! 因此这里也没有 `level` 形参 —— 少一个「传了但没用」的坑。

use std::io::Cursor;

use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

extension! {
    /// `$.lzma` / `$.unlzma`。
    pub struct LzmaExtension;
    "lzma" => js_lzma,
        "lzma(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer",
        "xz（LZMA2）压缩，产物兼容 xz / 7z / tar -J";
    "unlzma" => js_unlzma,
        "unlzma(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer",
        "xz 解压";
}

/// `lzma(data) -> ArrayBuffer`
fn js_lzma<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;

    let mut input = Cursor::new(bytes.as_slice());
    let mut compressed = Vec::new();
    lzma_rs::xz_compress(&mut input, &mut compressed)
        .map_err(|err| Exception::throw_message(&ctx, &format!("xz 压缩失败：{err}")))?;

    output_buffer(&ctx, compressed)
}

/// `unlzma(data) -> ArrayBuffer`
fn js_unlzma<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;

    let mut input = Cursor::new(bytes.as_slice());
    let mut plain = Vec::new();
    lzma_rs::xz_decompress(&mut input, &mut plain)
        .map_err(|err| Exception::throw_message(&ctx, &format!("xz 解压失败：{err}")))?;

    output_buffer(&ctx, plain)
}
