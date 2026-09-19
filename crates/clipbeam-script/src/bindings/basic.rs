//! core 内置能力：`$.file` / `$.sleep`。
//!
//! 这两项被 core 保留，是因为它们不依赖任何业务库（不需要 MD5 / zstd / 宿主交互），
//! 且是「脚本能做点事」的最小集：
//!
//! * 没有 `$.file` 脚本读不到任何输入（`$.md5` 之类的扩展也就无从下手）；
//! * 没有 `$.sleep` 脚本没有任何等待手段（引擎不提供 `setTimeout`）。
//!
//! 其余能力（摘要 / 压缩 / 键盘输出 / 确认）都由使用方的 `ScriptExtension` 提供。

use rquickjs::{Ctx, Object, Result as QjsResult};

use super::{file, timer};
use crate::extension::CapabilitySpec;

/// 把内置能力挂到命名空间对象上。
pub fn register<'js>(ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
    file::register(ctx, ns)?;
    timer::register(ctx, ns)?;
    Ok(())
}

/// 内置能力的签名声明（既是文档，也是补全数据源）。
pub fn spec() -> Vec<CapabilitySpec> {
    vec![
        CapabilitySpec {
            name: "file",
            signature: "file(path: string) -> Promise<ArrayBuffer>",
            doc: "读取真实文件，返回原始字节；相对路径按进程工作目录解析，失败时 Promise 被拒绝",
        },
        CapabilitySpec {
            name: "sleep",
            signature: "sleep(ms: number) -> Promise<void>",
            doc: "异步等待若干毫秒；等待期间被中止会立即返回",
        },
    ]
}
