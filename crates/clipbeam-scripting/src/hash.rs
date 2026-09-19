//! `$.md5(bytes)` 与 `$.base32(bytes)`：摘要与编码（纯计算扩展）。
//!
//! 都是**同步**函数：只做纯计算，不涉及 IO，返回字符串。
//! 参数统一是 `ArrayBuffer`（通常是 `await $.file(...)` 的结果或 `$.zstd(...)` 的分片）。
//!
//! base32 用 `data-encoding` 的 `BASE32_NOPAD`：RFC4648、**不带 `=` 填充**、输出小写，
//! 与 `src-tauri` 协议层的 `protocol::b32_encode_lower` 行为一致（同一套盘表约定）。

use clipbeam_script::bindings::array_buffer_to_vec;
use clipbeam_script::{CapabilitySpec, ScriptExtension};
use md5::{Digest, Md5};
use rquickjs::{ArrayBuffer, Ctx, Function, Object, Result as QjsResult};

/// 盘表：RFC4648 无填充。
const BASE32: data_encoding::Encoding = data_encoding::BASE32_NOPAD;

/// `$.md5`：32 位小写十六进制 MD5 摘要。
pub struct Md5Extension;

impl ScriptExtension for Md5Extension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("md5", Function::new(ctx.clone(), js_md5)?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "md5",
            signature: "md5(data: ArrayBuffer) -> string",
            doc: "32 位小写十六进制 MD5 摘要",
        }]
    }
}

/// `$.base32`：RFC4648 Base32 编码（无填充、小写）。
pub struct Base32Extension;

impl ScriptExtension for Base32Extension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("base32", Function::new(ctx.clone(), js_base32)?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "base32",
            signature: "base32(data: ArrayBuffer) -> string",
            doc: "RFC4648 Base32 编码，不带 `=` 填充，输出小写",
        }]
    }
}

/// `md5(bytes) -> string`：32 位小写十六进制。
///
/// `ctx` 由 `rquickjs` 自动注入（JS 侧只传 `bytes`）。
fn js_md5<'js>(ctx: Ctx<'js>, buf: ArrayBuffer<'js>) -> QjsResult<String> {
    let bytes = array_buffer_to_vec(&ctx, &buf)?;

    let mut hasher = Md5::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// `base32(bytes) -> string`：RFC4648 Base32（无填充、小写）。
///
/// 不带填充是为了直接拼进文本协议（例如自定义的分片报文）时更省字节；
/// 解码方按需自行补齐填充即可。
fn js_base32<'js>(ctx: Ctx<'js>, buf: ArrayBuffer<'js>) -> QjsResult<String> {
    let bytes = array_buffer_to_vec(&ctx, &buf)?;
    Ok(BASE32.encode(&bytes).to_lowercase())
}
