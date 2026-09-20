//! `$.zstd(data, level?)`：Zstandard 压缩（**只压缩，不切片**）。
//!
//! ```js
//! const packed = $.zstd(await $.read("data.bin"))     // 完整的一帧
//! const parts  = $.chunks(packed, 1024)               // 要分片就自己切
//! ```
//!
//! 切片交给 [`crate::extensions::chunks`]：压缩与切片是两件正交的事，绑在一起
//! 会让「只想压缩」的脚本也被迫接受一个默认分片尺寸。
//!
//! 级别与 `src-tauri::protocol` 的键盘帧压缩保持一致（默认 3），这样同一份输入
//! 在脚本侧与协议侧压出来的帧可以互换。
//!
//! 没有对应的解压函数：脚本侧只需要「压出去」，解压是宿主/协议层的职责
//! （`zstandard` 的帧格式自带校验，协议层用 `decode_all` 解）。

use rquickjs::function::Opt;
use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

/// 默认压缩级别：3 是「压缩率/速度」比较均衡的取值。
const DEFAULT_LEVEL: i32 = 3;

extension! {
    /// `$.zstd`。
    pub struct ZstdExtension;
    "zstd" => js_zstd,
        "zstd(data: string | ArrayBuffer | ArrayBufferView, level?: number) -> ArrayBuffer",
        "Zstandard 压缩（默认级别 3）；只压缩不分片，分片用 $.chunks";
}

/// `zstd(data, level = 3) -> ArrayBuffer`
///
/// `level` 的取值范围是 zstandard 的 `-131072..=22`（负数表示 fast 模式），
/// 越界会抛异常而不是静默退回默认值。
fn js_zstd<'js>(ctx: Ctx<'js>, data: Value<'js>, level: Opt<i32>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;
    let level = level.0.unwrap_or(DEFAULT_LEVEL);

    let options = zstandard::EncoderOptions {
        compression_level: zstandard::CompressionLevel::try_new(level)
            .map_err(|err| Exception::throw_message(&ctx, &format!("zstd 压缩级别非法：{err}")))?,
        ..Default::default()
    };

    let compressed = zstandard::encode_all_with_options(&bytes, options)
        .map_err(|err| Exception::throw_message(&ctx, &format!("zstd 压缩失败：{err}")))?;

    output_buffer(&ctx, compressed)
}
