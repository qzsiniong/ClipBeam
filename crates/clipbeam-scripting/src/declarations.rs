//! 把**类型声明**与**编辑器配置**释放到脚本目录，让用户写的 `.ts` 脚本有类型提示。
//!
//! # 与插件目录同一套做法
//!
//! 脚本入口也可以是 `.ts`，但它跑在 QuickJS 里 —— **没有 DOM、也没有 Node**。
//! 编辑器默认按浏览器那套全局检查，于是报 `找不到名称 '$'`，或者在能看到声明文件时
//! 报一堆 `Duplicate identifier 'TextDecoder'` / `Cannot redeclare 'console'`。
//!
//! 解法与插件目录完全一致（共用 `script_engine::portable` 里的规则）：
//!
//! | 文件 | 作用 |
//! |---|---|
//! | `engine.d.ts` | 引擎的标准全局（`sleep` / `console` / `TextDecoder` / 定时器…） |
//! | `clipbeam.d.ts` | `ClipBeam` / `$` 能力命名空间（本 crate 的声明，**去除仓库路径**后放进来） |
//! | `tsconfig.json` | 关掉 DOM、只留 ECMAScript 标准库，并 `include` 同目录的 `.ts` / `.d.ts` |
//!
//! 三者同处一层：TypeScript 的目录级项目会把同目录的 `.d.ts` 当全局声明收进来
//! （实测确认），用户在任何编辑器里打开脚本目录就有提示。
//!
//! # 这些文件是「产物」，不是「用户数据」
//!
//! 与内置示例脚本（`scripts.rs`，已存在就不覆盖）相反：**声明与 tsconfig 每次启动都覆盖**。
//! 它们是这份二进制对外契约的类型侧表示，必须与运行期一致。

use std::io;
use std::path::Path;

use script_engine::portable::{strip_repo_only_sections, TSCONFIG_BODY};

/// 引擎标准全局的声明文件名。
const ENGINE_DECL: &str = "engine.d.ts";
/// 本 crate 能力声明的文件名。
const CLIPBEAM_DECL: &str = "clipbeam.d.ts";
/// 编辑器项目配置的文件名。
const TSCONFIG: &str = "tsconfig.json";

/// 要放进脚本目录的文件（`(文件名, 内容)`）。
pub fn declaration_files() -> Vec<(&'static str, String)> {
    vec![
        (ENGINE_DECL, script_engine::spec::ENGINE_DTS.to_string()),
        (
            CLIPBEAM_DECL,
            strip_repo_only_sections(include_str!("spec/clipbeam.d.ts")),
        ),
        (TSCONFIG, TSCONFIG_BODY.to_string()),
    ]
}

/// 确保脚本目录里有类型声明与编辑器配置；返回本次**重写**的文件数。
///
/// 覆盖而不是跳过：它们是随二进制走的产物（见模块文档）。
pub fn ensure_declarations() -> io::Result<usize> {
    ensure_declarations_in(&crate::scripts::scripts_dir())
}

/// [`ensure_declarations`] 的可指定目录版本（测试用）。
pub fn ensure_declarations_in(dir: &Path) -> io::Result<usize> {
    std::fs::create_dir_all(dir)?;

    let mut written = 0;
    for (name, content) in declaration_files() {
        std::fs::write(dir.join(name), content)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-scripting-decls-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    #[test]
    fn writes_all_three_files_and_counts_them() {
        let dir = temp_dir("write");
        let written = ensure_declarations_in(&dir).expect("写声明失败");
        assert_eq!(
            written, 3,
            "应当写 engine.d.ts / clipbeam.d.ts / tsconfig.json"
        );

        for name in [ENGINE_DECL, CLIPBEAM_DECL, TSCONFIG] {
            assert!(dir.join(name).is_file(), "{name} 应当存在");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 再次调用必须重新写（产物语义），而不是「已存在就跳过」。
    #[test]
    fn rewrites_on_every_call() {
        let dir = temp_dir("rewrite");
        ensure_declarations_in(&dir).expect("首次写失败");

        std::fs::write(dir.join(CLIPBEAM_DECL), "// 被改坏了\n").unwrap();
        ensure_declarations_in(&dir).expect("二次写失败");
        assert!(
            !std::fs::read_to_string(dir.join(CLIPBEAM_DECL))
                .unwrap()
                .contains("被改坏"),
            "声明属于随二进制走的产物，必须每次覆盖"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 移植版里不能留下指向仓库结构的引用，但 `ClipBeam` / `$` 的声明必须完整。
    #[test]
    fn portable_declaration_drops_repo_only_reference() {
        let source = include_str!("spec/clipbeam.d.ts");
        let ported = strip_repo_only_sections(source);

        assert!(
            source.contains("<reference path="),
            "源文件里本来应当有那条引用"
        );
        assert!(
            !ported.contains("<reference path="),
            "移植版不该带上仓库路径"
        );
        assert!(
            !ported.contains(script_engine::portable::PORTABLE_BEGIN)
                && !ported.contains(script_engine::portable::PORTABLE_END),
            "移植版不该留下标记行本身"
        );
        for needle in ["declare const ClipBeam", "declare const $"] {
            assert!(ported.contains(needle), "移植版缺少 {needle}");
        }
    }

    /// 端到端：把声明写进一个「用户脚本目录」，加一个用 `$` 的 `.ts`，
    /// 然后**用真正的 tsc 检查**它有提示、且写错会被抓住。
    ///
    /// 这是本模块的验收标准 —— 光把文件写出来不算数。
    /// 没有 `node_modules/.bin/tsc` 的环境（只跑 cargo 的 CI job）会跳过。
    #[test]
    fn user_directory_typechecks_and_catches_mistakes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("拿不到仓库根目录")
            .to_path_buf();
        let tsc = root.join("node_modules/.bin/tsc");
        if !tsc.is_file() {
            eprintln!("跳过：找不到 {}（先跑 pnpm install）", tsc.display());
            return;
        }

        let dir = temp_dir("tsc");
        ensure_declarations_in(&dir).expect("写声明失败");

        // 用户视角的脚本：能力 + 引擎全局 + 顶层 await
        let good = r#"
await sleep(10)
const parts = $.chunks('hello world', 5)
console.log(parts.join('|'))
const text = await $.read_text('/tmp/demo.txt', 'utf-8')
if (await $.confirm('继续吗？')) $.type_str(text, 10)
"#;
        std::fs::write(dir.join("my-script.ts"), good).unwrap();

        let first = std::process::Command::new(&tsc)
            .arg("-p")
            .arg(dir.join(TSCONFIG))
            .output()
            .expect("无法启动 tsc");
        assert!(
            first.status.success(),
            "正常脚本应当通过类型检查：\n{}\n{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        // 故意写错：`chunks` 的第二个参数是数字，给字符串必须报错
        let bad = good.replace("$.chunks('hello world', 5)", "$.chunks('hello world', 'x')");
        std::fs::write(dir.join("my-script.ts"), bad).unwrap();

        let second = std::process::Command::new(&tsc)
            .arg("-p")
            .arg(dir.join(TSCONFIG))
            .output()
            .expect("无法启动 tsc");
        assert!(
            !second.status.success(),
            "写错的脚本必须被类型检查抓住（否则声明没真正生效）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
