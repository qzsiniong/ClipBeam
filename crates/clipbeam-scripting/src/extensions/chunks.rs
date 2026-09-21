//! `$.chunks(data, chunkSize?)`：把数据切成固定大小的分片。
//!
//! - **字符串**入参：按 **Unicode 码点**切（`.chars()`），返回 `string[]`，拼接回去与原文
//!   完全一致。按码点而不是 UTF-16 码元，是为了不把 emoji 的代理对切成孤立半个字符。
//! - **二进制**入参（`ArrayBuffer` / `ArrayBufferView`）：按**字节**切，返回 `ArrayBuffer[]`。
//!
//! 与压缩分开是刻意的：切片与压缩是两件正交的事，绑在 `zstd` 上会让
//! 「只想切片」和「只想压缩」的脚本都别扭。
//!
//! ```js
//! const packed = $.zstd(await $.read("data.bin"))   // 完整的一帧
//! for (const part of $.chunks(packed, 1024)) { await $.type_str($.base32(part)) }
//!
//! for (const part of $.chunks("很长的文本…", 200)) { await $.type_str(part) }
//! ```

use rquickjs::function::Opt;
use rquickjs::{Array, ArrayBuffer, Ctx, FromJs, Result as QjsResult, String as JsString, Value};

use crate::extensions::{extension, input_bytes};

/// 不传 `chunkSize` 时的默认分片大小（字符串按码点、二进制按字节）。
const DEFAULT_CHUNK_SIZE: usize = 1024;

extension! {
    /// `$.chunks`。
    pub struct ChunksExtension;
    "chunks" => js_chunks,
        "chunks(data: string, chunkSize?: number) -> string[]; chunks(data: ArrayBuffer | ArrayBufferView, chunkSize?: number) -> ArrayBuffer[]",
        "按 chunkSize 分片（默认 1024：字符串按码点、二进制按字节），返回互相独立的分片";
}

/// `chunks(data, chunkSize = 1024) -> string[] | ArrayBuffer[]`
///
/// 字符串按 Unicode 码点切（不切坏 emoji），二进制按字节切。
/// `chunkSize` 传 0 时按 1 处理（避免空切片死循环）；空输入返回空数组。
fn js_chunks<'js>(
    ctx: Ctx<'js>,
    data: Value<'js>,
    chunk_size: Opt<usize>,
) -> QjsResult<Array<'js>> {
    let chunk_size = chunk_size.0.unwrap_or(DEFAULT_CHUNK_SIZE).max(1);

    // 字符串：按码点切，返回 string[]（`chars()` 走的是码点边界，不会切出孤立代理项）
    if data.is_string() {
        let text = JsString::from_js(&ctx, data)?.to_string()?;
        let mut chars = text.chars();
        let chunks = Array::new(ctx.clone())?;
        let mut index = 0;
        loop {
            let part: String = chars.by_ref().take(chunk_size).collect();
            if part.is_empty() {
                break;
            }
            chunks.set(index, JsString::from_str(ctx.clone(), &part)?)?;
            index += 1;
        }
        return Ok(chunks);
    }

    // 二进制：按字节切，返回 ArrayBuffer[]
    let bytes = input_bytes(&ctx, data)?;
    let chunks = Array::new(ctx.clone())?;
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        chunks.set(index, ArrayBuffer::new(ctx.clone(), chunk.to_vec())?)?;
    }
    Ok(chunks)
}
