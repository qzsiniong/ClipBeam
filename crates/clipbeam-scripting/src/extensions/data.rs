//! `$.bytes(data)` / `$.str(data, encoding?)`：数据形态转换。
//!
//! 这对能力解决的是「脚本作者到底拿到的是什么」这一困扰：
//!
//! * `bytes` 把 `string` / `ArrayBuffer` / `ArrayBufferView` 统一成 `ArrayBuffer`，
//!   之后所有二进制能力（摘要、压缩、编码）都不再需要各自做类型判断；
//! * `str` 反过来把字节按指定编码解成文本（默认 UTF-8），省掉手写
//!   `new TextDecoder(...).decode(...)`。

use encoding_rs::Encoding;
use rquickjs::function::Opt;
use rquickjs::{
    ArrayBuffer, Ctx, Exception, FromJs, Result as QjsResult, String as JsString, Value,
};

use crate::extensions::{extension, input_bytes, output_buffer};

extension! {
    /// `$.bytes` / `$.str`。
    pub struct DataExtension;
    "bytes" => js_bytes,
        "bytes(data: string | ArrayBuffer | ArrayBufferView) -> ArrayBuffer",
        "统一成字节：字符串按 UTF-8 编码，ArrayBuffer/视图原样拷贝";
    "str" => js_str,
        "str(data: string | ArrayBuffer | ArrayBufferView, encoding?: string) -> string",
        "把字节按 encoding 解码成文本（默认 utf-8，支持 gbk 等全部 WHATWG 标签）";
}

/// `bytes(data) -> ArrayBuffer`
fn js_bytes<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<ArrayBuffer<'js>> {
    let bytes = input_bytes(&ctx, data)?;
    output_buffer(&ctx, bytes)
}

/// `str(data, encoding = "utf-8") -> string`
///
/// 传入字符串时原样返回（不重复编解码）。
fn js_str<'js>(ctx: Ctx<'js>, data: Value<'js>, encoding: Opt<String>) -> QjsResult<String> {
    if data.is_string() {
        return JsString::from_js(&ctx, data)?.to_string();
    }

    let bytes = input_bytes(&ctx, data)?;
    let label = encoding.0.unwrap_or_else(|| "utf-8".to_string());
    let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
        return Err(Exception::throw_message(
            &ctx,
            &format!("未知的文本编码标签：{label:?}"),
        ));
    };

    // 与 TextDecoder 一致（默认 ignoreBOM = false）：编码器对应的 BOM 会被去掉
    Ok(encoding.decode_with_bom_removal(&bytes).0.into_owned())
}
