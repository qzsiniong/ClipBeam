//! `$.md5(data)`：MD5 摘要（纯计算扩展）。
//!
//! 同步函数，入参是任意二进制形态（见 [`crate::extensions::input_bytes`]），
//! 返回 **32 位小写十六进制**。
//!
//! MD5 早已不适合做安全用途，这里保留它是因为 ClipBeam 的传输协议与
//! 「文件指纹」场景都在用它（与 `$._` 无关，只求两端算法一致）。

use md5::{Digest, Md5};
use rquickjs::{Ctx, Result as QjsResult, Value};

use crate::extensions::{extension, input_bytes};

extension! {
    /// `$.md5`。
    pub struct Md5Extension;
    "md5" => js_md5,
        "md5(data: string | ArrayBuffer | ArrayBufferView) -> string",
        "32 位小写十六进制 MD5 摘要";
}

/// `md5(data) -> string`：32 位小写十六进制。
fn js_md5<'js>(ctx: Ctx<'js>, data: Value<'js>) -> QjsResult<String> {
    let bytes = input_bytes(&ctx, data)?;

    let mut hasher = Md5::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}
