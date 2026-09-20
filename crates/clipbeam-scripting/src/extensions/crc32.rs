//! `$.crc32(data, format?)`：CRC-32（IEEE 802.3）校验和。
//!
//! 同一个 32 位整数有五种常见写法，用 `format` 选（默认 `hex`）：
//!
//! | format | 输出 | 长度 |
//! |---|---|---|
//! | `hex`（默认） | 大写十六进制 | 8 |
//! | `hex_lower` | 小写十六进制 | 8 |
//! | `base32` | Base32 大写、无填充 | 7 |
//! | `base32_lower` | Base32 小写、无填充 | 7 |
//! | `base64` | Base64 标准、无填充 | 6 |
//!
//! 字节序固定为**大端**（`to_be_bytes`），与 `src-tauri::protocol` 的二维码/分片
//! 校验字段一致；换字节序会让两端的校验和对不上。
//!
//! 注意：`hex` 默认是**大写**（与协议层的既有写法一致），这与 `$.hex`（小写）
//! 是两套故意不同的默认值 —— 前者是「校验和的惯用展示」，后者是「通用编码」。

use data_encoding::{BASE32_NOPAD, BASE64_NOPAD};
use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes};

/// 支持的格式名（也出现在错误文案里，避免两处漂移）。
const FORMATS: &str = "hex / hex_lower / base32 / base32_lower / base64";

extension! {
    /// `$.crc32`。
    pub struct Crc32Extension;
    "crc32" => js_crc32,
        "crc32(data: string | ArrayBuffer | ArrayBufferView, format?: \"hex\" | \"hex_lower\" | \"base32\" | \"base32_lower\" | \"base64\") -> string",
        "CRC-32 校验和（默认 8 位大写十六进制）";
}

/// `crc32(data, format = "hex") -> string`
fn js_crc32<'js>(ctx: Ctx<'js>, data: Value<'js>, format: Opt<String>) -> QjsResult<String> {
    let bytes = input_bytes(&ctx, data)?;

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes);
    let checksum = hasher.finalize();
    // 大端：与协议层的校验字段字节序保持一致
    let raw = checksum.to_be_bytes();

    match format.0.as_deref().unwrap_or("hex") {
        "hex" => Ok(format!("{checksum:08X}")),
        "hex_lower" => Ok(format!("{checksum:08x}")),
        "base32" => Ok(BASE32_NOPAD.encode(&raw)),
        "base32_lower" => Ok(BASE32_NOPAD.encode(&raw).to_ascii_lowercase()),
        "base64" => Ok(BASE64_NOPAD.encode(&raw)),
        other => Err(Exception::throw_message(
            &ctx,
            &format!("未知的 crc32 格式：{other:?}（可用：{FORMATS}）"),
        )),
    }
}
