//! 把声明文件「移植」到**用户数据目录**时用的共用规则。
//!
//! # 背景
//!
//! 脚本与插件的入口都可以是 `.ts`，但用户是在自己的数据目录里写它们
//! （`<配置目录>/ClipBeam/scripts/` 与 `.../plugins/`）。那里没有我们的仓库，
//! 所以使用方要把「引擎的标准全局」与「自己的能力」两组声明**拷进那个目录**，
//! 编辑器才认得出 `$` / `$plugin` / `console` / `sleep` 这些全局。
//!
//! 移植时有两件事必须做对，且**两个使用方完全一样**，因此放在这里：
//!
//! 1. **去掉只对仓库目录结构成立的行**：源声明里有一条 `reference` 指向
//!    `script-engine/src/spec/engine.d.ts`，只对仓库内的相对路径成立；
//!    平铺的用户目录里那条路径指不到任何东西，留着会让编辑器报 `File ... not found`。
//!    用 [`PORTABLE_BEGIN`] / [`PORTABLE_END`] 标出这一段并删掉。
//! 2. **给用户目录一份编辑器配置**：TypeScript 默认按「浏览器」检查
//!    （`lib.dom.d.ts`），而脚本/插件运行环境**没有 DOM** ——
//!    与 DOM 撞名的全局（`console` / `TextDecoder` / `performance`）会报
//!    `Duplicate identifier` / `Cannot redeclare`。`lib` 只留 ECMAScript 才能消掉这类噪音。
//!
//! 判据：**除了被删的那几行，声明本体一字不改** —— 类型是对外契约，
//! 移植过程不该顺手改它（[`strip_repo_only_sections`] 只做删行）。

/// 仓库内特有段落的开始标记（含本行会被删除）。
pub const PORTABLE_BEGIN: &str = "// PORTABLE-BEGIN";

/// 仓库内特有段落的结束标记（含本行会被删除）。
pub const PORTABLE_END: &str = "// PORTABLE-END";

/// 删掉 [`PORTABLE_BEGIN`] / [`PORTABLE_END`] 之间的行，其余内容原样保留。
///
/// 行尾统一补换行：源文件是按行读进来的，重新拼回去时保证以换行收尾。
pub fn strip_repo_only_sections(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut skipping = false;

    for line in source.lines() {
        if line.trim() == PORTABLE_BEGIN {
            skipping = true;
            continue;
        }
        if line.trim() == PORTABLE_END {
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

/// 写给用户数据目录的编辑器配置（`tsconfig.json` 的正文）。
///
/// `lib` 只有 `ESNext`、`types` 为空是**必须**的，理由见模块文档。
/// `include` 用 `["*.ts", "*.d.ts"]`：用户的 `.ts` 与同目录的声明都要进项目，
/// 而 TypeScript 的目录级项目会把同目录的 `.d.ts` 当全局声明收进来 ——
/// 于是「把文件放进那个目录」本身就是全部的配置步骤。
pub const TSCONFIG_BODY: &str = r#"{
  // 这个文件由 ClipBeam 自动生成（每次启动覆盖），不要手改。
  //
  // 它的唯一目的：让本目录下的 `.ts` 在编辑器里有类型提示，且与运行环境一致 ——
  // 脚本与插件都跑在 QuickJS 里，**没有 DOM、没有 Node**。
  "compilerOptions": {
    // 与运行时不符的东西一律不引入：
    //   * lib 只留 ECMAScript（加了 DOM 会与 engine.d.ts 里的 console / TextDecoder 撞名）
    //   * types 为空（不拉 @types/node）
    "target": "ES2022",
    "lib": ["ESNext"],
    "types": [],

    // 入口是「全局脚本」：能力命名空间与 console / sleep 都是全局声明，不需要 import
    "moduleDetection": "force",
    "module": "ESNext",
    "moduleResolution": "bundler",

    // 严格模式：让能力调用的类型错误在编辑器里就暴露
    "strict": true,
    "noEmit": true,
    "forceConsistentCasingInFileNames": true,
    "skipLibCheck": true
  },
  // .d.ts 是能力声明的真源（同目录，TypeScript 会当作全局声明收进来）
  "include": ["**/*.ts", "*.d.ts"]
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_the_marked_section() {
        let source = "a\n// PORTABLE-BEGIN\nb\nc\n// PORTABLE-END\nd\n";
        assert_eq!(strip_repo_only_sections(source), "a\nd\n");

        // 没有标记时一字不改（行尾换行会被规范成换行）
        assert_eq!(strip_repo_only_sections("x\ny\n"), "x\ny\n");
    }

    #[test]
    fn tsconfig_body_is_valid_jsonc_and_dom_free() {
        let stripped: String = TSCONFIG_BODY
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: serde_json::Value =
            serde_json::from_str(&stripped).expect("tsconfig 必须是合法 JSON");

        assert_eq!(
            parsed["compilerOptions"]["lib"],
            serde_json::json!(["ESNext"])
        );
        assert_eq!(parsed["compilerOptions"]["types"], serde_json::json!([]));
        assert_eq!(parsed["compilerOptions"]["noEmit"], serde_json::json!(true));
        assert_eq!(parsed["include"], serde_json::json!(["*.ts", "*.d.ts"]));
    }
}
