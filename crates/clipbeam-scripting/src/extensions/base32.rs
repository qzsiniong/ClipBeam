//! `$.base32` 系列：RFC4648 Base32 编解码。
//!
//! 名字就是编码规则，不用布尔参数（`base32(data, true, false)` 这种调用没人读得懂）：
//!
//! | 函数 | 字母表 | 填充 |
//! |---|---|---|
//! | `base32` | 大写 | 带 `=` |
//! | `base32_nopad` | 大写 | 无 |
//! | `base32_lower` | 小写 | 带 `=` |
//! | `base32_lower_nopad` | 小写 | 无 |
//!
//! 解码器与编码器**一一对应**，并且严格按名字校验：`base32_lower_decode` 遇到大写、
//! `base32_nopad_decode` 遇到 `=` 都会报错。这样「编码 → 解码」的往返一定是闭合的，
//! 解错格式时也不会静默给出一个看似合理的结果。
//!
//! 与 `src-tauri::protocol` 的关系：协议层的 `b32_encode_upper` 就是本模块的
//! `base32_nopad`（同一套无填充盘表），`b32_decode` 大小写都接受。

use data_encoding::{BASE32, BASE32_NOPAD};
use rquickjs::{ArrayBuffer, Ctx, Exception, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes, output_buffer};

/// 小写字母表：先把大写结果算出来再转换，保证两套盘表逐字符对应。
fn lower(text: String) -> String {
    text.to_ascii_lowercase()
}

/// 按指定盘表解码。
///
/// `expected` 是错误文案里的格式名；`lowercase` 为真时要求输入全小写
/// （盘表用大写那份，先把输入规范化）。
fn decode<'js>(
    ctx: &Ctx<'js>,
    text: &str,
    encoding: data_encoding::Encoding,
    expected: &str,
    lowercase: bool,
) -> QjsResult<ArrayBuffer<'js>> {
    let normalized = if lowercase {
        if text.chars().any(|ch| ch.is_ascii_uppercase()) {
            return Err(Exception::throw_message(
                ctx,
                &format!("{expected} 解码不接受大写字母：{text:?}"),
            ));
        }
        text.to_ascii_uppercase()
    } else {
        text.to_string()
    };

    let bytes = encoding
        .decode(normalized.as_bytes())
        .map_err(|err| Exception::throw_message(ctx, &format!("{expected} 解码失败：{err}")))?;
    output_buffer(ctx, bytes)
}

extension! {
    /// `$.base32` 系列。
    pub struct Base32Extension;
    "base32" => js_base32,
        "base32(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "RFC4648 Base32 编码：大写、带 `=` 填充";
    "base32_nopad" => js_base32_nopad,
        "base32_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "RFC4648 Base32 编码：大写、不带填充";
    "base32_lower" => js_base32_lower,
        "base32_lower(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "RFC4648 Base32 编码：小写、带 `=` 填充";
    "base32_lower_nopad" => js_base32_lower_nopad,
        "base32_lower_nopad(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "RFC4648 Base32 编码：小写、不带填充";
    "base32_decode" => js_base32_decode,
        "base32_decode(text: string) -> ArrayBuffer",
        "Base32 解码：要求大写且带填充（与 base32 对应）";
    "base32_nopad_decode" => js_base32_nopad_decode,
        "base32_nopad_decode(text: string) -> ArrayBuffer",
        "Base32 解码：要求大写且无填充（与 base32_nopad 对应）";
    "base32_lower_decode" => js_base32_lower_decode,
        "base32_lower_decode(text: string) -> ArrayBuffer",
        "Base32 解码：要求小写且带填充（与 base32_lower 对应）";
    "base32_lower_nopad_decode" => js_base32_lower_nopad_decode,
        "base32_lower_nopad_decode(text: string) -> ArrayBuffer",
        "Base32 解码：要求小写且无填充（与 base32_lower_nopad 对应）";
}

/// `base32(data) -> string`
fn js_base32<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE32.encode(&input_bytes(&ctx, data)?))
}

/// `base32_nopad(data) -> string`
fn js_base32_nopad<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(BASE32_NOPAD.encode(&input_bytes(&ctx, data)?))
}

/// `base32_lower(data) -> string`
fn js_base32_lower<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(lower(BASE32.encode(&input_bytes(&ctx, data)?)))
}

/// `base32_lower_nopad(data) -> string`
fn js_base32_lower_nopad<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    Ok(lower(BASE32_NOPAD.encode(&input_bytes(&ctx, data)?)))
}

/// `base32_decode(text) -> ArrayBuffer`
fn js_base32_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(&ctx, &text, BASE32, "base32（大写带填充）", false)
}

/// `base32_nopad_decode(text) -> ArrayBuffer`
fn js_base32_nopad_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(
        &ctx,
        &text,
        BASE32_NOPAD,
        "base32_nopad（大写无填充）",
        false,
    )
}

/// `base32_lower_decode(text) -> ArrayBuffer`
fn js_base32_lower_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(&ctx, &text, BASE32, "base32_lower（小写带填充）", true)
}

/// `base32_lower_nopad_decode(text) -> ArrayBuffer`
fn js_base32_lower_nopad_decode<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    decode(
        &ctx,
        &text,
        BASE32_NOPAD,
        "base32_lower_nopad（小写无填充）",
        true,
    )
}
