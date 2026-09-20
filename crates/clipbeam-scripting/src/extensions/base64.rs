//! `$.base64` 系列：Base64 编解码（标准盘表与 URL-safe 盘表）。
//!
//! 与 base32 同一套命名规则，多出来的一个维度是**盘表**（用 `_url` 后缀）：
//!
//! | 函数 | 盘表 | 填充 |
//! |---|---|---|
//! | `base64` | 标准（`+` `/`） | 带 `=` |
//! | `base64_nopad` | 标准 | 无 |
//! | `base64_url` | URL-safe（`-` `_`） | 带 `=` |
//! | `base64_url_nopad` | URL-safe | 无 |
//!
//! 解码器严格按名字校验盘表与填充：`base64_url_decode` 不接受 `+` `/`，
//! `base64_nopad_decode` 不接受 `=`。
//!
//! 注意与引擎自带全局 `btoa` / `atob` 的分工：那两个是 WHATWG 语义、只处理
//! **Latin-1 字符串**；这里处理任意字节（`string` 入参按 UTF-8 编码）。

use data_encoding::{BASE64, BASE64URL, BASE64URL_NOPAD, BASE64_NOPAD};
use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

/// 按指定盘表解码；`expected` 只用于错误文案。
fn decode<'js>(
    ctx: &Ctx<'js>,
    text: &str,
    encoding: data_encoding::Encoding,
    expected: &str,
) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = encoding
        .decode(text.as_bytes())
        .map_err(|err| Exception::throw_message(ctx, &format!("{expected} 解码失败：{err}")))?;
    output_buffer(ctx, bytes)
}

extension! {
    /// `$.base64` 系列。
    pub struct Base64Extension;
    "base64" => js_base64,
        "base64(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "标准 Base64 编码：带 `=` 填充";
    "base64_nopad" => js_base64_nopad,
        "base64_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "标准 Base64 编码：不带填充";
    "base64_url" => js_base64_url,
        "base64_url(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "URL-safe Base64 编码（`-` `_`）：带 `=` 填充";
    "base64_url_nopad" => js_base64_url_nopad,
        "base64_url_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "URL-safe Base64 编码（`-` `_`）：不带填充";
    "base64_decode" => js_base64_decode,
        "base64_decode(text: string) -> ArrayBuffer",
        "标准 Base64 解码：要求带填充（与 base64 对应）";
    "base64_nopad_decode" => js_base64_nopad_decode,
        "base64_nopad_decode(text: string) -> ArrayBuffer",
        "标准 Base64 解码：要求无填充（与 base64_nopad 对应）";
    "base64_url_decode" => js_base64_url_decode,
        "base64_url_decode(text: string) -> ArrayBuffer",
        "URL-safe Base64 解码：要求带填充（与 base64_url 对应）";
    "base64_url_nopad_decode" => js_base64_url_nopad_decode,
        "base64_url_nopad_decode(text: string) -> ArrayBuffer",
        "URL-safe Base64 解码：要求无填充（与 base64_url_nopad 对应）";
}

/// `base64(data) -> string`
fn js_base64<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE64.encode(&input_bytes(&ctx, data)?))
}

/// `base64_nopad(data) -> string`
fn js_base64_nopad<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE64_NOPAD.encode(&input_bytes(&ctx, data)?))
}

/// `base64_url(data) -> string`
fn js_base64_url<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE64URL.encode(&input_bytes(&ctx, data)?))
}

/// `base64_url_nopad(data) -> string`
fn js_base64_url_nopad<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE64URL_NOPAD.encode(&input_bytes(&ctx, data)?))
}

/// `base64_decode(text) -> ArrayBuffer`
fn js_base64_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(&ctx, &text, BASE64, "base64（标准带填充）")
}

/// `base64_nopad_decode(text) -> ArrayBuffer`
fn js_base64_nopad_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(&ctx, &text, BASE64_NOPAD, "base64_nopad（标准无填充）")
}

/// `base64_url_decode(text) -> ArrayBuffer`
fn js_base64_url_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(&ctx, &text, BASE64URL, "base64_url（URL-safe 带填充）")
}

/// `base64_url_nopad_decode(text) -> ArrayBuffer`
fn js_base64_url_nopad_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(
        &ctx,
        &text,
        BASE64URL_NOPAD,
        "base64_url_nopad（URL-safe 无填充）",
    )
}
