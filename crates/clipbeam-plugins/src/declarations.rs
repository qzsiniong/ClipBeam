//! 把**类型声明**与**编辑器配置**释放到插件目录，让用户写的 `.ts` 插件有类型提示。
//!
//! # 为什么需要这一步
//!
//! 插件的入口可以是 `.ts`，但它跑在 QuickJS 里 —— **没有 DOM、也没有 Node**。
//! 编辑器（VS Code / WebStorm）默认拿「浏览器」那套全局去检查文件，就会：
//!
//! * 报 `找不到名称 '$plugin'`（因为 `$plugin` 是全局声明，编辑器不知道它在哪）；
//! * 或者反过来，在能看到声明文件时报一堆 `Duplicate identifier 'TextDecoder'` /
//!   `Cannot redeclare block-scoped variable 'console'`（声明与 `lib.dom.d.ts` 撞名）。
//!
//! 两者都实测复现过（见本文件末尾的测试）。解法是把三样东西放进插件目录：
//!
//! | 文件 | 作用 |
//! |---|---|
//! | `engine.d.ts` | 引擎的标准全局（`sleep` / `console` / `TextDecoder` / 定时器…） |
//! | `plugins.d.ts` | `$plugin` 能力命名空间（本 crate 的声明，**去除仓库路径**后放进来） |
//! | `tsconfig.json` | 关掉 DOM、只留 ECMAScript 标准库，并 `include` 同目录的 `.ts` / `.d.ts` |
//!
//! 三者同处一层是刻意的：TypeScript 的**目录级项目**会把同目录的 `.d.ts` 当全局声明收进来
//! （实测确认），于是用户在任何编辑器里打开插件目录就有提示，不需要手工配置。
//!
//! # 这些文件是「产物」，不是「用户数据」
//!
//! 与示例插件（`seed.rs`，已存在就不覆盖）相反：**声明与 tsconfig 每次启动都覆盖**。
//! 它们是这份二进制**对外契约**的类型侧表示，必须与运行期一致 —— 用户改了它们，
//! 编辑器看到的就是一个不存在的 API 表。用户真正要写的是 `.ts` 插件本身，那个不受影响。

use std::io;
use std::path::Path;

use script_engine::portable::{strip_repo_only_sections, TSCONFIG_BODY};

/// 引擎标准全局的声明文件名。
const ENGINE_DECL: &str = "engine.d.ts";
/// 本 crate 能力声明的文件名。
const PLUGIN_DECL: &str = "plugins.d.ts";
/// 编辑器项目配置的文件名。
const TSCONFIG: &str = "tsconfig.json";

/// 要放进插件目录的文件（`(文件名, 内容)`）。
///
/// 抽成公开函数是为了让两个使用方（`clipbeam-plugins` 的这把 [`ensure_declarations_in`]）
/// 与测试都读同一份事实，而不是各自再列一遍。
pub fn declaration_files() -> Vec<(&'static str, String)> {
    vec![
        (ENGINE_DECL, script_engine::spec::ENGINE_DTS.to_string()),
        (
            PLUGIN_DECL,
            strip_repo_only_sections(include_str!("spec/plugins.d.ts")),
        ),
        (TSCONFIG, TSCONFIG_BODY.to_string()),
    ]
}

/// 确保插件目录里有类型声明与编辑器配置；返回本次**重写**的文件数。
///
/// 覆盖而不是跳过：它们是随二进制走的产物（见模块文档）。
pub fn ensure_declarations() -> io::Result<usize> {
    ensure_declarations_in(&crate::catalog::plugins_dir())
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
            "clipbeam-plugin-decls-{}-{seq}-{name}",
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
            "应当写 engine.d.ts / plugins.d.ts / tsconfig.json"
        );

        for name in [ENGINE_DECL, PLUGIN_DECL, TSCONFIG] {
            assert!(dir.join(name).is_file(), "{name} 应当存在");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 再次调用必须**重新写**（产物语义），而不是像示例那样「已存在就跳过」。
    #[test]
    fn rewrites_on_every_call() {
        let dir = temp_dir("rewrite");
        ensure_declarations_in(&dir).expect("首次写失败");

        // 模拟「用户改了声明」——下次启动必须被纠正回来
        std::fs::write(dir.join(PLUGIN_DECL), "// 被改坏了\n").unwrap();
        let written = ensure_declarations_in(&dir).expect("二次写失败");
        assert_eq!(written, 3);
        assert!(
            !std::fs::read_to_string(dir.join(PLUGIN_DECL))
                .unwrap()
                .contains("被改坏"),
            "声明属于随二进制走的产物，必须每次覆盖"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 移植版必须删掉仓库特有的 `reference`，其余内容一字不改。
    #[test]
    fn portable_declaration_drops_repo_only_reference() {
        let source = include_str!("spec/plugins.d.ts");
        let ported = strip_repo_only_sections(source);

        assert!(
            source.contains("<reference path="),
            "源文件里本来应当有那条引用（否则这个测试失去意义）"
        );
        assert!(
            !ported.contains("<reference path="),
            "移植版不该带上指向仓库结构的引用"
        );
        // 标记行本身也必须消失（留下的话就是一条没有作用的注释）
        assert!(
            !ported.contains(script_engine::portable::PORTABLE_BEGIN)
                && !ported.contains(script_engine::portable::PORTABLE_END),
            "移植版不该留下标记行本身"
        );
        // 声明本体不能少：`$plugin` 的两个全局与窗口能力都得在
        for needle in [
            "declare const $plugin",
            "declare const ClipBeamPlugin",
            "window: ClipBeamPluginWindow",
        ] {
            assert!(ported.contains(needle), "移植版缺少 {needle}");
        }
    }

    /// 被删的**只有**标记之间的那几行，标记前后的内容逐字节保留（类型是契约，不能顺手改）。
    #[test]
    fn portable_declaration_preserves_everything_else() {
        let source = include_str!("spec/plugins.d.ts");
        let ported = strip_repo_only_sections(source);

        let lines: Vec<&str> = source.lines().collect();
        let begin = lines
            .iter()
            .position(|line| line.trim() == script_engine::portable::PORTABLE_BEGIN)
            .expect("源文件里应当有开始标记");
        let end = lines
            .iter()
            .position(|line| line.trim() == script_engine::portable::PORTABLE_END)
            .expect("源文件里应当有结束标记");
        assert!(begin < end, "标记顺序不对");

        // 标记前的部分 + 标记后的部分，应当恰好等于移植结果
        let expected: String = lines[..begin]
            .iter()
            .chain(lines[end + 1..].iter())
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(ported, expected, "只该删掉标记之间的行");

        // 被删掉的确实就是那条引用
        let removed = lines[begin + 1..end].join("\n");
        assert!(
            removed.contains("<reference path="),
            "被删的应当是那条 reference：{removed}"
        );
    }

    /// 仓库根目录（从本 crate 的 `CARGO_MANIFEST_DIR` 往上两级）。
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("拿不到仓库根目录")
            .to_path_buf()
    }

    /// 找一个**本平台能真正执行**的 `tsc`；找不到返回 `None`，调用方跳过测试。
    ///
    /// 与 `clipbeam-scripting/src/declarations.rs` 里的同名函数是**故意重复**的两份：
    /// 两个 crate 之间没有共享的测试支撑 crate，为这 20 行引入一个依赖不划算。
    /// 改这里时记得同步那边。
    ///
    /// 为什么不能只用 `node_modules/.bin/tsc`：pnpm/npm 在 Windows 上会同时生成
    /// `tsc`（POSIX sh 脚本）、`tsc.cmd`、`tsc.ps1`。无扩展名那个 `is_file()` 为**真**，
    /// 于是「找不到就跳过」的兜底失效，而它是 `#!/bin/sh`、`CreateProcess` 起不来
    /// （os error 193）—— 测试直接 panic。CI 的 ubuntu job 不跑 `pnpm install`，
    /// 文件根本不存在，所以这个坑只在 Windows 上暴露。
    ///
    /// 所以**优先走 `node` + typescript 的入口脚本**：两端行为完全一致，不碰垫片的
    /// 平台差异（`node` 由 CI 的 setup-node 保证）；垫片只作为兜底。
    fn tsc_command(root: &std::path::Path) -> Option<std::process::Command> {
        let entry = root.join("node_modules/typescript/lib/tsc.js");
        if entry.is_file() {
            let mut cmd = std::process::Command::new("node");
            cmd.arg(entry);
            return Some(cmd);
        }
        let shim = if cfg!(windows) {
            root.join("node_modules/.bin/tsc.cmd")
        } else {
            root.join("node_modules/.bin/tsc")
        };
        shim.is_file().then(|| std::process::Command::new(shim))
    }

    /// 跑一次 `tsc -p <tsconfig>`。
    ///
    /// `None` = 这个环境没有可用的 tsc，**应当跳过测试**（只跑 cargo 的 CI job，
    /// 或者垫片存在但本平台起不来）。前端流水线里的 `typecheck:examples` 兜住仓库侧，
    /// 所以跳过不会丢覆盖率。
    fn run_tsc(root: &std::path::Path, project: &std::path::Path) -> Option<std::process::Output> {
        let mut cmd = tsc_command(root)?;
        match cmd.arg("-p").arg(project).output() {
            Ok(output) => Some(output),
            Err(err) => {
                eprintln!("跳过：tsc 存在但无法启动（{err}）");
                None
            }
        }
    }

    /// 四个文件都写进去之后，**用真正的 tsc 检查一次**用户视角的插件。
    ///
    /// 这是这整件事的验收标准：光「文件写出来了」不算数，
    /// 要能证明编辑器/`tsc`真的认出了 `$plugin`，并且真的会在写错时报错。
    ///
    /// 没有 `pnpm` / `tsc` 的环境（例如只跑 `cargo test` 的 CI job）会**跳过**这个测试 ——
    /// 前端那条流水线里有 `typecheck:examples` 兜住仓库侧。
    #[test]
    fn user_directory_typechecks_and_catches_mistakes() {
        let root = repo_root();
        if tsc_command(&root).is_none() {
            eprintln!("跳过：找不到可用的 tsc（先跑 pnpm install）");
            return;
        }

        let dir = temp_dir("tsc");

        // 1) 释放声明与 tsconfig（模拟应用启动时做的事）
        ensure_declarations_in(&dir).expect("写声明失败");

        // 2) 用户视角的插件：用上托盘、窗口与引擎全局
        let good = r#"
await sleep(10)
console.log('已启用')
const win = $plugin.window.open({ page: 'ui.html', width: 300 })
$plugin.window.onMessage(win.id, (message) => console.log(message))
$plugin.toast('hi', { level: 'success' })
"#;
        std::fs::write(dir.join("my-plugin.ts"), good).unwrap();

        let Some(first) = run_tsc(&root, &dir.join(TSCONFIG)) else {
            return;
        };
        assert!(
            first.status.success(),
            "正常插件应当通过类型检查：\n{}\n{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        // 3) 故意写错：等级写成 'warn'（正确值是 'warning'）——必须报错，
        //    否则说明编辑器看到的其实不是这套声明，「有提示」就是假的
        let bad = good.replace("{ level: 'success' }", "{ level: 'warn' }");
        std::fs::write(dir.join("my-plugin.ts"), bad).unwrap();

        let Some(second) = run_tsc(&root, &dir.join(TSCONFIG)) else {
            return;
        };
        assert!(
            !second.status.success(),
            "写错的插件必须被类型检查抓住（否则声明没真正生效）"
        );
        let output = String::from_utf8_lossy(&second.stdout);
        assert!(
            output.contains("PluginToastLevel") || output.contains("warn"),
            "错误信息应当指向 toast 等级：{output}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 用户目录的**真实形态**：把「生成的声明与 tsconfig」和「仓库里的示例插件入口」
    /// 放进同一个临时目录，再用真 `tsc` 检查。
    ///
    /// 与上面那条测试的区别：那条用一段手写插件验证 `$plugin` 能解析；
    /// 这条用**内置示例本身**（更复杂、用到托盘+窗口+反馈）验证 —— 示例能过，
    /// 说明用户照抄示例不会撞到类型错误。
    #[test]
    fn seeded_example_typechecks_against_generated_declarations() {
        let root = repo_root();
        if tsc_command(&root).is_none() {
            eprintln!("跳过：找不到可用的 tsc（先跑 pnpm install）");
            return;
        }

        let dir = temp_dir("seed-tsc");
        ensure_declarations_in(&dir).expect("写声明失败");

        // 示例入口原样搬进「用户目录」
        std::fs::write(
            dir.join("index.ts"),
            include_str!("../seed/hello-plugin/index.ts"),
        )
        .unwrap();

        let Some(output) = run_tsc(&root, &dir.join(TSCONFIG)) else {
            return;
        };

        assert!(
            output.status.success(),
            "内置示例应当能通过类型检查：\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
