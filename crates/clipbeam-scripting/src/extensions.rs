//! 能力注册表与扩展之间的公共设施。
//!
//! 每个能力一个文件（见子模块），本模块只干三件事：
//!
//! 1. 声明 [`extension!`] 宏 —— 把「注册若干个 JS 函数」+「声明它们的签名」这两件
//!    必须同步的事写在一处，漏掉一边编译就过不去（`spec` 与运行期的漂移是审查里
//!    最容易漏的一类问题）；
//! 2. 提供二进制入参/出参的公共转换（[`input_bytes`] / [`output_buffer`]）；
//! 3. [`extensions`] 汇总全部能力，供 `RuntimeOptions` 使用。

use std::sync::Arc;

use rquickjs::{
    ArrayBuffer, Ctx, Exception, FromJs, Result as QjsResult, String as JsString, Value,
};

use script_engine::ScriptExtension;

pub mod base32;
pub mod base64;
pub mod brotli;
pub mod chunks;
pub mod crc32;
pub mod data;
pub mod file;
pub mod gzip;
pub mod hex;
pub mod interaction;
pub mod lzma;
pub mod md5;
pub mod zstd;

/// 声明一个能力扩展：一次写清「挂哪些函数」与「签名是什么」。
///
/// ```ignore
/// extension! {
///     /// `$.md5(data)`：MD5 摘要。
///     pub struct Md5Extension;
///     "md5" => js_md5, "md5(data: string | ArrayBuffer) -> string", "32 位小写十六进制 MD5 摘要";
/// }
/// ```
///
/// * 函数名前的 `async` 关键字表示该函数是 `async fn`（会用 `Async` 包成返回 Promise
///   的 JS 函数）；不写就是同步函数。
///
/// 需要往上下文里存状态的扩展（例如文件扩展的「本次运行已放行的目录」）自己手写
/// `register`：宏只处理「无状态、只挂函数」这一类。
macro_rules! extension {
    (
        $(#[$attr:meta])*
        pub struct $ext:ident;
        $(
            $($kind:ident)? $target:literal => $func:path, $signature:literal, $doc:literal;
        )*
    ) => {
        $(#[$attr])*
        pub struct $ext;

        impl script_engine::ScriptExtension for $ext {
            fn register<'js>(
                &self,
                ctx: &rquickjs::Ctx<'js>,
                ns: &rquickjs::Object<'js>,
            ) -> rquickjs::Result<()> {
                $(
                    $crate::extensions::extension!(@bind ns, ctx, $target, $func, $($kind)?);
                )*
                Ok(())
            }

            fn spec(&self) -> Vec<script_engine::CapabilitySpec> {
                vec![
                    $(
                        script_engine::CapabilitySpec {
                            name: $target,
                            signature: $signature,
                            doc: $doc,
                        },
                    )*
                ]
            }
        }
    };

    (@bind $ns:ident, $ctx:ident, $target:literal, $func:path,) => {
        $ns.set($target, rquickjs::Function::new($ctx.clone(), $func)?)?;
    };

    (@bind $ns:ident, $ctx:ident, $target:literal, $func:path, async) => {
        $ns.set(
            $target,
            rquickjs::Function::new($ctx.clone(), rquickjs::prelude::Async($func))?,
        )?;
    };
}

// 子模块通过 `use crate::extensions::extension;` 使用这个宏（macro_rules 必须先定义再导出）
pub(crate) use extension;

/// 把脚本传入的「二进制参数」统一转成字节。
///
/// 支持三种形态，覆盖脚本作者的所有直觉写法：
///
/// | 传入 | 处理 |
/// |---|---|
/// | `string` | 按 UTF-8 编码（与 `TextEncoder` 一致） |
/// | `ArrayBuffer` | 直接拷贝 |
/// | `ArrayBufferView`（`Uint8Array` / `DataView` / 其它 TypedArray） | 按 `byteOffset` + `byteLength` 切片后拷贝 |
///
/// 其它类型抛 `TypeError`。**一律拷贝**：JS 侧随后怎么改这块内存都不影响我们。
pub fn input_bytes<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> QjsResult<Vec<u8>> {
    if value.is_string() {
        let text = JsString::from_js(ctx, value)?;
        return Ok(text.to_string()?.into_bytes());
    }

    if let Some(buf) = ArrayBuffer::from_value(value.clone()) {
        return script_engine::bindings::array_buffer_to_vec(ctx, &buf);
    }

    // ArrayBufferView：拿到它背后的 buffer 与视图范围。
    // 不走 `TypedArray::<u8>` 是因为视图可能是 `DataView` / `Uint16Array` 等，
    // 逐个类型匹配会漏；直接按 JS 侧的三个属性算范围对任何视图都成立。
    if let Some(object) = value.as_object() {
        let view = (
            object.get::<_, ArrayBuffer>("buffer"),
            object.get::<_, usize>("byteOffset"),
            object.get::<_, usize>("byteLength"),
        );
        if let (Ok(buffer), Ok(offset), Ok(length)) = view {
            let all = script_engine::bindings::array_buffer_to_vec(ctx, &buffer)?;
            let end = offset.saturating_add(length).min(all.len());
            return Ok(all.get(offset..end).unwrap_or_default().to_vec());
        }
    }

    Err(Exception::throw_type(
        ctx,
        "参数必须是 string / ArrayBuffer / ArrayBufferView",
    ))
}

/// 把字节包成 JS 的 `ArrayBuffer`（所有权交给引擎，零拷贝）。
pub fn output_buffer<'js>(ctx: &Ctx<'js>, bytes: Vec<u8>) -> QjsResult<ArrayBuffer<'js>> {
    ArrayBuffer::new(ctx.clone(), bytes)
}

/// 宿主错误 → JS 异常（取消统一转成「脚本已中止」，其余带上分类文案）。
pub fn throw_host_error<'js>(ctx: &Ctx<'js>, err: crate::HostError) -> rquickjs::Error {
    if err == crate::HostError::Cancelled {
        return script_engine::bindings::throw_cancelled(ctx);
    }
    Exception::throw_message(ctx, &host_error_message(&err))
}

/// 宿主错误的展示文案（取消已由 [`throw_host_error`] 提前处理）。
pub fn host_error_message(err: &crate::HostError) -> String {
    match err {
        crate::HostError::Cancelled => "脚本已中止".to_string(),
        crate::HostError::Timeout => "等待用户确认超时（按「否」处理）".to_string(),
        crate::HostError::Unsupported => "当前运行环境不支持该能力".to_string(),
        crate::HostError::Failed(message) => message.clone(),
    }
}

/// 取回上下文里的宿主；没有注入时报明确错误（而不是静默什么都不做）。
pub fn require_host<'js>(
    ctx: &Ctx<'js>,
    capability: &str,
) -> QjsResult<Arc<dyn crate::ScriptHost>> {
    crate::host::host_ctx(ctx).ok_or_else(|| {
        Exception::throw_message(
            ctx,
            &format!(
                "{capability} 不可用：当前运行环境没有注入宿主（把 ScriptHost 传给 runtime_options）"
            ),
        )
    })
}

/// ClipBeam 的全部脚本能力（按注册顺序）。`sleep` 不在其中：它是引擎提供的标准全局。
///
/// 顺序没有硬性依赖，但保持稳定可以让 `spec` 与运行期行为一致、也方便日志对照。
pub fn extensions() -> Vec<Arc<dyn ScriptExtension>> {
    vec![
        // 数据形态与编解码
        Arc::new(data::DataExtension),
        Arc::new(chunks::ChunksExtension),
        Arc::new(hex::HexExtension),
        Arc::new(base32::Base32Extension),
        Arc::new(base64::Base64Extension),
        // 摘要
        Arc::new(md5::Md5Extension),
        Arc::new(crc32::Crc32Extension),
        // 压缩
        Arc::new(zstd::ZstdExtension),
        Arc::new(gzip::GzipExtension),
        Arc::new(brotli::BrotliExtension),
        Arc::new(lzma::LzmaExtension),
        // 文件系统
        Arc::new(file::FileExtension),
        // 宿主交互
        Arc::new(interaction::TypeStrExtension),
        Arc::new(interaction::ConfirmExtension),
    ]
}
