//! 供命令行复用的引擎入口。
//!
//! 应用（`src-tauri`）的 GUI 路径复用 [`run_script`] 即可拿到完全一致的行为；
//! 区别只在注入哪个 [`ScriptHost`](clipbeam_script::ScriptHost)。

pub mod cli_host;

pub use cli_host::CliScriptHost;

use std::path::Path;
use std::time::Instant;

use clipbeam_script::{CancellationToken, ScriptHost, ScriptRuntime};

use crate::{runtime_options, ts, ts::is_typescript};

/// 一次脚本执行的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    /// 实际交给宿主的字符数（脚本没输出时是 0）。
    pub typed_chars: usize,
    /// 总耗时。
    pub elapsed_ms: u64,
}

/// 读取并按需转译脚本源码。
///
/// `.ts` / `.mts` / `.cts` 用 oxc 在进程内转译；`.js` 系列原样返回。
/// 语法错误在这里就会带着文件名与行列返回，不会拖到运行期。
pub fn load_script(path: &Path) -> Result<String, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| format!("读取脚本文件 {} 失败：{err}", path.display()))?;

    if !is_typescript(path) {
        return Ok(source);
    }

    ts::transpile(&source, path).map_err(|err| err.to_string())
}

/// 在引擎里跑一段已经准备好的脚本源码。
///
/// `name` 只影响错误信息里的文件名；`host` 决定 `$.typeStr` / `$.confirm` 落到哪儿。
pub async fn run_source(
    name: &str,
    source: &str,
    host: std::sync::Arc<dyn ScriptHost>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let runtime = ScriptRuntime::with_options(
        runtime_options()
            .script_name(name)
            .host(host)
            .cancel_token(cancel),
    )
    .await
    .map_err(|err| format!("创建脚本引擎失败：{err}"))?;

    runtime
        .run_named_script(name, source)
        .await
        .map_err(|err| err.to_string())
}

/// 读文件 → 转译（必要时）→ 执行，返回耗时与输出字符数。
///
/// 这是 CLI 与应用 GUI 共用的「跑一个脚本文件」入口。
pub async fn run_script_file(
    path: &Path,
    host: std::sync::Arc<dyn ScriptHost>,
    cancel: CancellationToken,
    typed_chars: impl Fn() -> usize,
) -> Result<RunReport, String> {
    let source = load_script(path)?;
    let name = path.to_string_lossy().into_owned();

    let started = Instant::now();
    run_source(&name, &source, host, cancel).await?;

    Ok(RunReport {
        typed_chars: typed_chars(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_script_rejects_missing_file() {
        let err = load_script(Path::new("/clipbeam/definitely/not/here.js"))
            .expect_err("缺文件应当报错");
        assert!(err.contains("读取脚本文件"), "错误信息应说明来源：{err}");
    }
}
