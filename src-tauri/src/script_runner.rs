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

/// 生成「内嵌指定文件路径」的文件传输脚本，并转译成可执行的 JS。
///
/// 做的是文本替换而不是改写 AST：目标是一行**受我们控制**的常量声明，
/// 校验「恰好出现一次」就足够安全 —— 找不到就报错，绝不悄悄跑一份错的脚本。
///
/// 为什么不把路径当参数传：脚本引擎目前没有参数通道（`run_source` 只有名字与源码），
/// 而给引擎加一条参数机制会牵动两个 crate 的公开接口。替换一行常量对用户还更透明：
/// 生成的脚本在 Console 面板里看得见它要发哪个文件。
pub fn transfer_script_with_path(path: &str) -> Result<String, String> {
    const PLACEHOLDER: &str = "const srcPath: string | null = null";

    let template = clipbeam_scripting::scripts::TRANSFER_SEED;
    let occurrences = template.matches(PLACEHOLDER).count();
    if occurrences != 1 {
        return Err(format!(
            "文件传输脚本的模板有变化（预期 1 处 `{PLACEHOLDER}`，实际 {occurrences} 处）；\
             请检查 crates/clipbeam-scripting/seed/04-file-transfer.ts"
        ));
    }

    // 路径用 JSON 序列化：反斜杠、引号、中文都能安全塞进 TS 字符串字面量
    let literal = serde_json::to_string(path).map_err(|e| format!("路径转义失败：{e}"))?;
    let with_path = template.replace(
        PLACEHOLDER,
        &format!("const srcPath: string | null = {literal}"),
    );

    transpile_if_needed(crate::commands::FILE_TRANSFER_SCRIPT, &with_path)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成的传输脚本必须**内嵌**所选路径，且能转译。
    ///
    /// 这条测试同时守住模板的形态：`04-file-transfer.ts` 里那行占位常量一旦被改名，
    /// 下面的 `expect` 会直接失败，而不是让托盘菜单在运行时才发现。
    #[test]
    fn transfer_script_embeds_the_chosen_path() {
        let js = transfer_script_with_path("/tmp/季度报告 final.pdf")
            .expect("应当能生成并转译文件传输脚本");

        assert!(
            js.contains("/tmp/季度报告 final.pdf"),
            "生成结果里应当内嵌所选路径：\n{js}"
        );
        // 占位符必须被替换干净 —— 留着它就说明脚本还会走「弹选择框」那条分支
        assert!(
            !js.contains("const srcPath: string | null = null"),
            "占位常量应当已被替换：\n{js}"
        );
    }

    /// 路径里的引号、反斜杠、换行都要能被安全转义（不能拼出一个语法错误的脚本）。
    #[test]
    fn transfer_script_escapes_hostile_paths() {
        for path in [
            r#"/tmp/a"b.pdf"#,
            r"/tmp/back\slash.pdf",
            "/tmp/line\nbreak.pdf",
        ] {
            let js = transfer_script_with_path(path)
                .unwrap_or_else(|e| panic!("路径 {path:?} 应当能生成脚本：{e}"));
            assert!(!js.is_empty());
        }
    }
}
