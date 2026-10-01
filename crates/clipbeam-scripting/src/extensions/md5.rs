//! `$.md5(data, format?)`：MD5 摘要（纯计算扩展）。
//!
//! 同步函数，入参是任意二进制形态（见 [`crate::extensions::input_bytes`]）。
//! 格式名与 [`crate::extensions::crc32`] **完全对齐**：
//!
//! | format | 输出 | 长度 |
//! |---|---|---|
//! | `hex`（默认） | 小写十六进制 | 32 |
//! | `hex_upper` | 大写十六进制 | 32 |
//! | `base32` | Base32 大写、无填充 | 26 |
//! | `base32_lower` | Base32 小写、无填充 | 26 |
//! | `base64` | Base64 标准、无填充 | 22 |
//!
//! # 一处刻意的差异：`hex` 的大小写
//!
//! `crc32` 的 `hex` 是**大写**，`md5` 的 `hex` 是**小写**。原因是历史默认值：
//! `$.md5` 一直返回小写十六进制，改掉会破坏既有脚本。两者都提供显式的
//! `hex_upper` 消除歧义。
//!
//! # 为什么需要 `base32` / `base32_lower`
//!
//! 传输协议的指纹字段要求「只含 `[a-z2-7]`」，而十六进制含 `0`/`1`（会撞上帧的
//! 定界符），所以必须先把摘要的**原始 16 字节**编成 base32。
//! 过去脚本里手写 `base32_lower_nopad(hex_decode(md5(x)))` —— 那条组合先被误解成
//! 「对 hex 串做 base32」（得到 52 字符、且含 `0`/`1`），绕过一次。把它收进能力，
//! 「base32 的是原始字节」这条约定就由能力本身保证，不再依赖注释提醒。
//!
//! MD5 早已不适合做安全用途；这里保留它是因为传输协议与「文件指纹」场景都在用，
//! 只求两端算法一致。

use data_encoding::{BASE32_NOPAD, BASE64_NOPAD};
use md5::{Digest, Md5};
use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes};

/// 支持的格式名（也出现在错误文案里，避免两处漂移）。
const FORMATS: &str = "hex / hex_upper / base32 / base32_lower / base64";

extension! {
    /// `$.md5`。
    pub struct Md5Extension;
    "md5" => js_md5,
        "md5(data: string | ArrayBuffer | ArrayBufferView, format?: \"hex\" | \"hex_upper\" | \"base32\" | \"base32_lower\" | \"base64\") -> string",
        "MD5 摘要（默认 32 位小写十六进制；base32 变体用于协议指纹字段）";
}

/// `md5(data, format = "hex") -> string`
fn js_md5<'js>(ctx: Ctx<'js>, data: Value<'js>, format: Opt<String>) -> QjsResult<String> {
    let bytes = input_bytes(&ctx, data)?;

    let mut hasher = Md5::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();

    match format.0.as_deref().unwrap_or("hex") {
        "hex" => Ok(hex::encode(digest)),
        "hex_upper" => Ok(hex::encode_upper(digest)),
        "base32" => Ok(BASE32_NOPAD.encode(&digest)),
        "base32_lower" => Ok(BASE32_NOPAD.encode(&digest).to_ascii_lowercase()),
        "base64" => Ok(BASE64_NOPAD.encode(&digest)),
        other => Err(Exception::throw_message(
            &ctx,
            &format!("未知的 md5 格式：{other:?}（可用：{FORMATS}）"),
        )),
    }
}
