//! TypeScript 转译的集成测试（移植自上游 JS 版本，断言保持一致）。
//!
//! 一半是纯转译测试（不需要引擎，快且能精确断言输出），
//! 一半是端到端测试（转译 → 交给 QuickJS 执行 → 校验结果）。

use std::path::{Path, PathBuf};

use script_engine::{ts, ScriptRuntime};

/// 把源码写进临时 `.ts` 文件，返回路径（转译器需要真实扩展名来判定语法）。
fn write_ts(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("script-engine-ts-{}-{name}", std::process::id()));
    std::fs::write(&path, source).expect("写入临时 TS 文件失败");
    path
}

/// 转译临时文件里的 TS 源码。
fn transpile(name: &str, source: &str) -> anyhow::Result<String> {
    let path = write_ts(name, source);
    let result = ts::transpile(source, &path);
    let _ = std::fs::remove_file(&path);
    result
}

/// 类型注解、`interface`、`type`、`as` 这些「只在编译期存在」的语法要被剥掉。
#[test]
fn strips_type_only_syntax() {
    let js = transpile(
        "strip.ts",
        r#"
        interface User { name: string; age: number }
        type Id = string | number;
        const user: User = { name: "a", age: 1 };
        const id = 1 as unknown as Id;
        function greet(u: User): string { return u.name; }
        const x: number = 42;
        x;
        "#,
    )
    .expect("转译失败");

    assert!(!js.contains("interface"), "interface 没被剥掉：\n{js}");
    assert!(!js.contains("type Id"), "type 别名没被剥掉：\n{js}");
    assert!(!js.contains(" as "), "as 断言没被剥掉：\n{js}");
    assert!(!js.contains(": number"), "类型注解没被剥掉：\n{js}");
    assert!(!js.contains(": User"), "参数类型没被剥掉：\n{js}");
    assert!(js.contains("const x = 42"), "值声明应当保留：\n{js}");
}

/// `enum` / `namespace` 是**有运行时语义**的 TS 语法，必须被转成可用的 JS。
#[test]
fn transpiles_enum_and_namespace() {
    let js = transpile(
        "enum.ts",
        r#"
        enum Color { Red, Green = 5, Blue }
        namespace Util { export const tag = "u"; }
        Color.Blue;
        "#,
    )
    .expect("转译失败");

    assert!(!js.contains("enum Color"), "enum 语法没被转译：\n{js}");
    assert!(
        !js.contains("namespace Util"),
        "namespace 语法没被转译：\n{js}"
    );
    assert!(js.contains("Color"), "enum 名字应当保留：\n{js}");
}

/// 回归测试：`.ts` 默认按「没有 import/export 就是 script」解析，
/// 而脚本里常写顶层 await —— 转译器必须强制 module 才能解析通过。
#[test]
fn allows_top_level_await_without_imports() {
    let js = transpile(
        "tla.ts",
        r#"
        const bytes: ArrayBuffer = new TextEncoder().encode("顶层 await");
        bytes.byteLength;
        "#,
    )
    .expect("顶层 await 的 TS 应当能转译");

    assert!(js.contains("await"), "顶层 await 应当保留：\n{js}");
}

/// 现代语法不做 ES 降级（引擎是 quickjs-ng，不需要 target 下沉）。
#[test]
fn does_not_downlevel_modern_syntax() {
    let js = transpile(
        "modern.ts",
        r#"
        class Box { #value: number = 1; get value(): number { return this.#value ?? 0; } }
        const box = new Box();
        box?.value;
        let n: number | undefined;
        n ??= 3;
        n;
        "#,
    )
    .expect("转译失败");

    assert!(js.contains("?."), "可选链不应被降级：\n{js}");
    assert!(js.contains("??"), "空值合并不应被降级：\n{js}");
    assert!(js.contains("#value"), "私有字段不应被降级：\n{js}");
    assert!(
        !js.contains("babelHelpers"),
        "不应产出需要 helper 的代码：\n{js}"
    );
}

/// 语法错误要报在**原始 .ts 文件**的位置上，并且信息可读。
#[test]
fn reports_syntax_error_with_location() {
    let path = write_ts("broken.ts", "const x: = 1;\n");
    let err = ts::transpile("const x: = 1;\n", &path).expect_err("语法错误应当转译失败");
    let _ = std::fs::remove_file(&path);

    let text = err.to_string();
    assert!(text.contains("解析失败"), "应当标明阶段：{text}");
    assert!(
        text.contains(&path.display().to_string()),
        "应当包含文件名：{text}"
    );
    assert!(text.contains("broken.ts"), "应当包含文件名：{text}");
}

/// 非 TS 扩展名原样返回（不复制、不改动）。
#[test]
fn leaves_javascript_untouched() {
    let source = "const a = 1;\n";
    let code = ts::transpile_if_needed(source, Path::new("demo.js")).expect("JS 不需要转译");
    assert_eq!(code, source);

    // JSX/TSX 明确报错，而不是当成普通 JS 跑到引擎里才炸
    let err =
        ts::transpile_if_needed(source, Path::new("demo.tsx")).expect_err("TSX 应当明确报不支持");
    assert!(err.to_string().contains("JSX"), "错误信息要提到 JSX：{err}");
}

/// 端到端：带类型注解和顶层 await 的 `.ts` 真的能在引擎里跑出正确结果。
///
/// 只用引擎自带的东西（`TextEncoder` + 全局 `sleep`）—— 业务能力与命名空间
/// 都属于使用方，引擎侧的测试里不该依赖它们。
#[tokio::test]
async fn end_to_end_typescript_script_runs() {
    let payload = "typescript end to end\n";
    // 先把字符串转成 JS 字面量再嵌进脚本，避免 format! 嵌套
    let text_literal = format!("{payload:?}");
    let source = format!(
        r#"
        interface Result {{ size: number }}
        await sleep(1);
        const bytes: ArrayBuffer = new TextEncoder().encode({text_literal});
        const result: Result = {{ size: bytes.byteLength }};
        result.size
        "#
    );

    let script_path = write_ts("e2e.ts", &source);
    let js = ts::transpile(&source, &script_path).expect("转译失败");

    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");
    let value: i32 = runtime.eval(&js).await.expect("脚本执行失败");

    let _ = std::fs::remove_file(&script_path);

    assert_eq!(value as usize, payload.len());
}
