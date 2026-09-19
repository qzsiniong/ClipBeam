//! `$.zstd(bytes, chunkSize)`：Zstandard 压缩并按固定大小切片（纯计算扩展）。
//!
//! 同步函数，返回 `ArrayBuffer[]`，方便脚本把压缩结果按需分片传输：
//!
//! ```js
//! const bytes  = await $.file("data.bin")
//! const chunks = $.zstd(bytes, 1024)   // 每个分片最多 1024 字节
//! ```

use clipbeam_script::bindings::array_buffer_to_vec;
use clipbeam_script::{CapabilitySpec, ScriptExtension};
use rquickjs::{Array, ArrayBuffer, Ctx, Exception, Function, Object, Result as QjsResult};

/// 压缩级别：1 最快、22 最高压缩率，3 是「压缩率/速度」比较均衡的取值。
///
/// zstandard 的 `CompressionLevel` 取值范围是 `-131072..=22`，负数表示 fast 模式。
/// 与 `src-tauri::protocol` 的键盘帧压缩级别保持一致。
const COMPRESSION_LEVEL: i32 = 3;

/// 不传 `chunkSize` 时的默认分片大小（字节）。
const DEFAULT_CHUNK_SIZE: usize = 1024;

/// `$.zstd`：压缩 + 切片。
pub struct ZstdExtension;

impl ScriptExtension for ZstdExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("zstd", Function::new(ctx.clone(), js_zstd)?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "zstd",
            signature: "zstd(data: ArrayBuffer, chunkSize?: number) -> ArrayBuffer[]",
            doc: "Zstandard 压缩（级别 3）并按 chunkSize 切片，默认 1024 字节",
        }]
    }
}

/// `zstd(bytes, chunkSize?) -> ArrayBuffer[]`
///
/// * `chunkSize`：每个分片的最大字节数；省略时用 [`DEFAULT_CHUNK_SIZE`]，
///   传 0 或负数时按 1 处理，避免 `chunks(0)` panic。
/// * 返回的每个分片都是一个独立的 `ArrayBuffer`，拼接起来才是完整的压缩流。
/// * 压缩失败（例如输入过大）会抛出 JS 异常，而不是静默返回空数组。
fn js_zstd<'js>(
    ctx: Ctx<'js>,
    buf: ArrayBuffer<'js>,
    chunk_size: rquickjs::function::Opt<usize>,
) -> QjsResult<Array<'js>> {
    let data = array_buffer_to_vec(&ctx, &buf)?;
    let chunk_size = chunk_size.0.unwrap_or(DEFAULT_CHUNK_SIZE).max(1);

    let options = zstandard::EncoderOptions {
        compression_level: zstandard::CompressionLevel::try_new(COMPRESSION_LEVEL)
            .map_err(|err| Exception::throw_message(&ctx, &format!("zstd 压缩级别非法：{err}")))?,
        ..Default::default()
    };

    let compressed = zstandard::encode_all_with_options(&data, options)
        .map_err(|err| Exception::throw_message(&ctx, &format!("zstd 压缩失败：{err}")))?;

    let chunks = Array::new(ctx.clone())?;
    for (index, chunk) in compressed.chunks(chunk_size).enumerate() {
        chunks.set(index, ArrayBuffer::new(ctx.clone(), chunk.to_vec())?)?;
    }
    Ok(chunks)
}
