//! 脚本任务的编排：目录 CRUD、能力清单、以及「把脚本跑起来」的统一入口。
//!
//! 业务逻辑尽量留在 `clipbeam-scripting`（脚本目录、能力集、CLI 宿主），
//! 这里只做 Tauri 侧的三件事：
//!
//! 1. 把 `Result<_, String>` 风格的错误整理成前端能直接展示的文案；
//! 2. 为 GUI 注入 [`crate::scripting::TauriScriptHost`]（它同时是 `ScriptHost` 与 `ConsoleHook`）；
//! 3. 把 `HostError` 分类成「已中止 / 已超时 / 环境不支持 / 失败」。

use std::path::Path;
use std::sync::Arc;

use clipbeam_scripting::scripts::{self, ScriptMeta};
use clipbeam_scripting::{runtime_options, ts, ScriptHost};
use script_engine::{CancelSignal, ConsoleHook, ScriptRuntime};

use crate::cancel::CancellationToken as WorkerCancel;

/// 列出脚本目录里的脚本。
pub fn list() -> Result<Vec<ScriptMeta>, String> {
    scripts::list_scripts().map_err(|err| format!("读取脚本目录失败：{err}"))
}

/// 读取脚本源码。
pub fn read(name: &str) -> Result<String, String> {
    scripts::read_script(name)
}

/// 保存脚本源码。
pub fn write(name: &str, source: &str) -> Result<(), String> {
    scripts::write_script(name, source)
}

/// 删除脚本。
pub fn delete(name: &str) -> Result<(), String> {
    scripts::delete_script(name)
}

/// 确保脚本目录与内置示例存在。
pub fn seed() -> Result<usize, String> {
    scripts::ensure_seed_scripts().map_err(|err| format!("初始化脚本目录失败：{err}"))
}

/// 脚本目录的绝对路径（前端用于提示「文件放在哪」）。
pub fn dir_display() -> String {
    scripts::scripts_dir().to_string_lossy().into_owned()
}

/// 读取脚本文件（按扩展名决定是否用 oxc 转译）。
///
/// 语法错误在这里就返回，带文件名与行列，不会拖到运行期。
pub fn load(name: &str) -> Result<String, String> {
    let source = scripts::read_script(name)?;
    transpile_if_needed(name, &source)
}

/// 按扩展名转译源码（`name` 只用于判断语法与错误信息里的文件名）。
pub fn transpile_if_needed(name: &str, source: &str) -> Result<String, String> {
    ts::transpile_if_needed(source, Path::new(name))
        .map(|code| code.into_owned())
        .map_err(|err| err.to_string())
}

/// 在引擎里执行一段脚本源码（宿主、console 落点与取消信号由调用方注入）。
pub async fn run_source(
    name: &str,
    source: &str,
    host: Arc<dyn ScriptHost>,
    console: Arc<dyn ConsoleHook>,
    cancel: CancelSignal,
) -> Result<(), String> {
    let runtime = ScriptRuntime::with_options(
        runtime_options(host, console)
            .script_name(name)
            .cancel(cancel),
    )
    .await
    .map_err(|err| format!("创建脚本引擎失败：{err:#}"))?;

    runtime
        .run_named_script(name, source)
        .await
        .map_err(|err| err.to_string())
}

/// 把脚本引擎的错误整理成给用户看的一句话。
///
/// 引擎已经把 JS 异常渲染成「文件名:行:列 + 消息」，这里只做前缀标注。
pub fn describe_error(err: &str) -> String {
    if err.contains("脚本已中止") {
        return "脚本已中止".to_string();
    }
    format!("脚本执行失败：{err}")
}

/// 任务级的中止检查：把 Worker 的取消令牌桥接成引擎信号。
///
/// `WorkerState` 的令牌是仓库里既有的实现（`src/cancel.rs`），脚本引擎有自己的
/// [`CancelSignal`]。这里让引擎信号**监听** Worker 的标志位，于是 Esc 中止
/// 既能让 typer 停手，也能让 `sleep` 与能力检查立刻返回。
pub fn engine_cancel(token: &WorkerCancel) -> CancelSignal {
    let flag = token.flag();
    CancelSignal::watching(move || flag.load(std::sync::atomic::Ordering::SeqCst))
}
