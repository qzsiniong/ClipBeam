//! `$.hex` / `$.hex_upper` / `$.hex_decode`：十六进制编解码。
//!
//! 与 `$.md5` 的小写输出保持同一套写法：`hex` 小写、`hex_upper` 大写。
//! 解码只有一个入口（`hex_decode`），大小写都接受 —— 十六进制里大小写没有语义差别，
//! 不像 base32/base64 那样存在「另一种盘表」。

use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

extension! {
    /// `$.hex` 系列。
    pub struct HexExtension;
    "hex" => js_hex,
        "hex(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "十六进制编码，小写";
    "hex_upper" => js_hex_upper,
        "hex_upper(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "十六进制编码，大写";
    "hex_decode" => js_hex_decode,
        "hex_decode(text: string) -> ArrayBuffer",
        "十六进制解码（大小写均可，长度为奇数或含非法字符时报错）";
}

/// `hex(data) -> string`
fn js_hex<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(hex::encode(input_bytes(&ctx, data)?))
}

/// `hex_upper(data) -> string`
fn js_hex_upper<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(hex::encode_upper(input_bytes(&ctx, data)?))
}

/// `hex_decode(text) -> ArrayBuffer`
fn js_hex_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = hex::decode(&text)
        .map_err(|err| Exception::throw_message(&ctx, &format!("十六进制解码失败：{err}")))?;
    output_buffer(&ctx, bytes)
}
