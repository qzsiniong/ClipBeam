//! `TextDecoder` / `TextEncoder` 的 Rust 侧原语。
//!
//! 这两个类本身在 `prelude.js` 里用 JavaScript 定义 —— `new X()` 的类语义、
//! `encoding`/`fatal`/`ignoreBOM` getter、以及按规范抛 `RangeError` / `TypeError`，
//! 放在 JS 里最自然。这里只提供三块纯计算能力：
//!
//! | 原语 | 作用 |
//! |---|---|
//! | `encodingName(label)` | 把 WHATWG 标签规范化成编码名，未知返回 `null` |
//! | `decodeText(bytes, label, fatal, ignoreBom)` | 字节 → 字符串，严格模式失败返回 `null` |
//! | `encodeText(text)` | 字符串 → UTF-8 字节（`TextEncoder` 规范固定 UTF-8） |
//!
//! 「返回 `null` 而不是抛异常」是刻意的：错误类型（`RangeError` / `TypeError`）由 JS
//! 侧决定，Rust 只负责报告「没有这个编码」「数据非法」这两个事实。

use encoding_rs::Encoding;
use rquickjs::{
    ArrayBuffer, Ctx, Function, Object, Result as QjsResult, String as JsString, Value,
};

use super::array_buffer_bytes;

/// 把三个原语挂到命名空间对象上（实际挂在内部对象 `__clipbeam` 上）。
pub fn register<'js>(ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
    ns.set(
        "encodingName",
        Function::new(ctx.clone(), js_encoding_name)?,
    )?;
    ns.set("decodeText", Function::new(ctx.clone(), js_decode_text)?)?;
    ns.set("encodeText", Function::new(ctx.clone(), js_encode_text)?)?;
    Ok(())
}

/// `encodingName(label) -> string | null`
///
/// 用 Encoding Standard 的标签表做匹配（大小写不敏感、忽略首尾 ASCII 空白），
/// 返回编码的规范名（如 `UTF-8`、`GBK`、`Shift_JIS`）。JS 侧再把规范名转小写作为
/// `TextDecoder.prototype.encoding` 的值。
fn js_encoding_name<'js>(ctx: Ctx<'js>, label: String) -> QjsResult<Value<'js>> {
    match Encoding::for_label(label.as_bytes()) {
        Some(encoding) => Ok(JsString::from_str(ctx, encoding.name())?.into_value()),
        None => Ok(Value::new_null(ctx)),
    }
}

/// `decodeText(bytes, label, fatal, ignoreBom) -> string | null`
///
/// * 未知标签 → `null`（调用方已经用 `encodingName` 校验过，这里只是兜底）；
/// * `fatal = true` 且遇到非法字节序列 → `null`，由 JS 抛 `TypeError`；
/// * `ignoreBom = false`（默认）时按 BOM 嗅探并去掉 BOM，与 WHATWG 行为一致。
fn js_decode_text<'js>(
    ctx: Ctx<'js>,
    buf: ArrayBuffer<'js>,
    label: String,
    fatal: bool,
    ignore_bom: bool,
) -> QjsResult<Value<'js>> {
    let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
        return Ok(Value::new_null(ctx));
    };

    // 缓冲区已被 detach 时返回 null，JS 侧统一按 TypeError 处理
    let Some(bytes) = array_buffer_bytes(&buf) else {
        return Ok(Value::new_null(ctx));
    };

    // 按需去掉 BOM：与 encoding_rs::Encoding::decode 的 BOM 嗅探保持一致，
    // 这样 fatal 与非 fatal 两条路径对同一个输入的前缀处理是一样的。
    let (encoding, payload): (&Encoding, &[u8]) = if ignore_bom {
        (encoding, &bytes)
    } else {
        match Encoding::for_bom(&bytes) {
            Some((bom_encoding, bom_len)) => (bom_encoding, &bytes[bom_len..]),
            None => (encoding, &bytes),
        }
    };

    let text = if fatal {
        // 严格模式：非法序列直接判失败，不做 U+FFFD 替换
        match encoding.decode_without_bom_handling_and_without_replacement(payload) {
            Some(text) => text,
            None => return Ok(Value::new_null(ctx)),
        }
    } else {
        encoding.decode_without_bom_handling(payload).0
    };

    Ok(JsString::from_str(ctx, &text)?.into_value())
}

/// `encodeText(text) -> ArrayBuffer`：UTF-8 编码（无 BOM）。
///
/// JS 字符串转 Rust `String` 时，孤立代理项会被替换成 U+FFFD，正好符合
/// `TextEncoder` 对 USVString 的要求。
fn js_encode_text<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<ArrayBuffer<'js>> {
    ArrayBuffer::new(ctx, text.into_bytes())
}
