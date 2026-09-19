//! TypeScript → JavaScript 转译。
//!
//! 用 [oxc](https://oxc.rs/) 在**进程内**把 `.ts` 脚本转成等价的 JavaScript，
//! 不依赖外部 Node/tsc，符合「嵌入式运行时自包含」的定位。
//!
//! 转译只做**语法层**处理，不做 ES 版本降级（`TransformOptions::default()` 的
//! `env` 各版本开关默认关闭）：
//!
//! * 剥掉类型注解、`interface`、`type`、`as`、`implements` 等纯类型语法；
//! * 把 `enum` / `namespace` 转成运行时对象；
//! * 现代语法（可选链、`??=`、class field…）原样保留，交给 QuickJS 执行。
//!
//! 因此输出不需要任何 helper，运行时也不引入 polyfill 包。
//!
//! ## 已知限制
//!
//! * **行号会漂移**：输出是重新打印的 JS，`run_named_script` 报错里的 `行:列`
//!   指向生成代码；CLI 提供 `--emit-js` 导出生成结果便于对照（不做 source map）。
//! * 需要 helper 的语法（装饰器等）会产出 `babelHelpers.xxx` 调用，这里直接报错，
//!   避免运行期才炸。
//! * 不支持 JSX/TSX（没有 React 运行时）。

use std::borrow::Cow;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_diagnostics::{Diagnostics, NamedSource};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{HelperLoaderMode, TransformOptions, Transformer};

/// oxc 用 `babelHelpers.xxx` 标记「需要外部 helper」的产出。
const HELPER_MARKER: &str = "babelHelpers";

/// 路径是否是本模块负责转译的 TypeScript 源文件（`.ts` / `.mts` / `.cts`）。
pub fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("ts" | "mts" | "cts")
    )
}

/// 按需转译：TypeScript 返回转译后的 `String`，其它扩展名原样借用返回。
///
/// JSX/TSX 会明确报错而不是当成普通 JS 去跑（否则 QuickJS 只会给一个含义不明的语法错误）。
pub fn transpile_if_needed<'a>(source: &'a str, path: &Path) -> Result<Cow<'a, str>> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("ts" | "mts" | "cts") => Ok(Cow::Owned(transpile(source, path)?)),
        Some("tsx" | "jsx") => bail!(
            "暂不支持 JSX/TSX 脚本（{}）：运行时没有 React，请改写为 .ts 或先自行编译成 .js",
            path.display()
        ),
        _ => Ok(Cow::Borrowed(source)),
    }
}

/// 把 TypeScript 源码转成 JavaScript 源码。
///
/// `path` 只用于推断语法（`.ts`/`.mts`/`.cts`）和错误信息里显示文件名。
pub fn transpile(source: &str, path: &Path) -> Result<String> {
    // ── 1. 解析 ────────────────────────────────────────────────────────────
    let allocator = Allocator::default();

    // `.ts` 默认按「没有 import/export 就当 script」判定（ModuleKind::Unambiguous），
    // 而脚本里常常直接写顶层 await —— 强制 module 才能让顶层 await 通过解析。
    // 注意这只影响**解析**；真正执行时仍然是 QuickJS 的全局 + async 求值。
    let source_type = SourceType::from_path(path)
        .map_err(|err| anyhow!("无法识别的脚本扩展名：{err}"))?
        .with_module(true);

    let parsed = Parser::new(&allocator, source, source_type).parse();
    fail_on_diagnostics("解析", parsed.diagnostics, source, path)?;
    let mut program = parsed.program;

    // ── 2. 语义分析 ────────────────────────────────────────────────────────
    // `with_enum_eval(true)` 是 TS enum 求值必需的；多余容量按 oxc 官方示例给 2 倍，
    // 因为转译会新增作用域/符号。
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);
    fail_on_diagnostics("语义分析", semantic.diagnostics, source, path)?;
    let scoping = semantic.semantic.into_scoping();

    // ── 3. 转译 ────────────────────────────────────────────────────────────
    let mut options = TransformOptions::default();
    // helper 一律走「外部」模式：正常 TS 语法用不到 helper，真用到了（装饰器等）
    // 会在下面被检测出来报错，而不是生成一份跑不起来的代码。
    options.helper_loader.mode = HelperLoaderMode::External;

    let transformed =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    fail_on_diagnostics("转译", transformed.diagnostics, source, path)?;

    // ── 4. 生成代码 ────────────────────────────────────────────────────────
    let code = Codegen::new().build(&program).code;

    if code.contains(HELPER_MARKER) {
        bail!(
            "{} 用到了需要外部 helper 的 TypeScript 语法（例如装饰器），当前运行时暂不支持",
            path.display()
        );
    }

    Ok(code)
}

/// 把 oxc 的诊断渲染成「带文件名 + 行列 + 源码片段」的多行错误信息。
///
/// 只有出现 **error 级**诊断才算失败（警告放行），渲染时把警告也一并展示出来。
fn fail_on_diagnostics(
    stage: &str,
    diagnostics: Diagnostics,
    source: &str,
    path: &Path,
) -> Result<()> {
    if !diagnostics.has_errors() {
        return Ok(());
    }

    let named = NamedSource::new(path.display().to_string(), source);
    let rendered = diagnostics
        .into_vec()
        .into_iter()
        .map(|diagnostic| diagnostic.render_with_source_code(named.clone()))
        .collect::<Vec<_>>()
        .join("\n");

    bail!("TypeScript {stage}失败：\n{rendered}")
}
