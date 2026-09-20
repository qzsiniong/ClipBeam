//! `$.chunks(data, chunkSize?)`：把字节切成固定大小的分片。
//!
//! 与压缩分开是刻意的：切片与压缩是两件正交的事，绑在 `zstd` 上会让
//! 「只想切片」和「只想压缩」的脚本都别扭。
//!
//! ```js
//! const packed = $.zstd(await $.read("data.bin"))   // 完整的一帧
//! for (const part of $.chunks(packed, 1024)) { await $.type_str($.base32(part)) }
//! ```

use rquickjs::function::Opt;
use rquickjs::{Array, ArrayBuffer, Ctx, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes};

/// 不传 `chunkSize` 时的默认分片大小（字节）。
const DEFAULT_CHUNK_SIZE: usize = 1024;

extension! {
    /// `$.chunks`。
    pub struct ChunksExtension;
    "chunks" => js_chunks,
        "chunks(data: string | ArrayBuffer | ArrayBufferView, chunkSize?: number) -> ArrayBuffer[]",
        "按 chunkSize 切片（默认 1024 字节），返回独立的分片数组";
}

/// `chunks(data, chunkSize = 1024) -> ArrayBuffer[]`
///
/// `chunkSize` 传 0 或负数时按 1 处理（避免空切片死循环）；空输入返回空数组。
fn js_chunks<'js>(
    ctx: Ctx<'js>,
    data: Value<'js>,
    chunk_size: Opt<usize>,
) -> QjsResult<Array<'js>> {
    let bytes = input_bytes(&ctx, data)?;
    let chunk_size = chunk_size.0.unwrap_or(DEFAULT_CHUNK_SIZE).max(1);

    let chunks = Array::new(ctx.clone())?;
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        chunks.set(index, ArrayBuffer::new(ctx.clone(), chunk.to_vec())?)?;
    }
    Ok(chunks)
}
