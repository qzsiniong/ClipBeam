//! `spec/engine.d.ts` 与运行期绑定的一致性测试。
//!
//! 类型声明是**手写**的（TypeScript 的声明文件无法从 Rust 自动生成），因此容易漂移：
//! 加了绑定却忘了写声明，或删了绑定却没删声明。这里用最直接的方式兜底 ——
//! 抓出声明文件里的全局名字，断言它们在运行期真的存在。
//!
//! 注意这里**不涉及能力命名空间**：引擎不定义命名空间，命名空间的名字与类型
//! 都由使用方声明（见使用方能力集 crate 的同名测试）。

use script_engine::ScriptRuntime;

/// 从 `src/spec/engine.d.ts` 里抽取全局声明的名字。
///
/// 只认三种写法（声明文件是我们自己写的，格式受控）：
///
/// * `declare function name(`
/// * `declare const name:`
/// * `declare class name {`
///
/// 注释与 `interface` 声明都会被跳过，因此不会把辅助类型算进来。
fn globals_declared_in_spec() -> Vec<String> {
    const PREFIXES: [&str; 3] = ["declare function ", "declare const ", "declare class "];
    let source = include_str!("../src/spec/engine.d.ts");

    let mut names = Vec::new();
    for line in source.lines() {
        let Some(rest) = PREFIXES.iter().find_map(|prefix| line.strip_prefix(prefix)) else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// 声明文件里的每个全局都必须在运行期存在。
#[tokio::test]
async fn declared_globals_exist_at_runtime() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");
    let declared = globals_declared_in_spec();

    // 抽样确认解析没跑偏（少一个都说明解析规则与文件格式脱节了）
    for expected in [
        "sleep",
        "atob",
        "btoa",
        "setTimeout",
        "structuredClone",
        "performance",
    ] {
        assert!(
            declared.iter().any(|name| name == expected),
            "解析声明文件失败，没读到 {expected}：{declared:?}"
        );
    }

    for name in &declared {
        let present: bool = runtime
            .eval(&format!("typeof {name} !== 'undefined'"))
            .await
            .expect("脚本执行失败");
        assert!(present, "engine.d.ts 声明了 {name}，但运行期不存在");
    }
}

/// 反过来：引擎提供的标准全局都要写进声明文件（不能只写实现不写类型）。
#[tokio::test]
async fn every_engine_global_is_declared() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");
    let declared = globals_declared_in_spec();

    // 引擎公开承诺的标准全局（新增时必须同步 d.ts）
    let expected = [
        "sleep",
        "atob",
        "btoa",
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "performance",
        "structuredClone",
        "TextDecoder",
        "TextEncoder",
        "console",
    ];

    for name in expected {
        let present: bool = runtime
            .eval(&format!("typeof {name} !== 'undefined'"))
            .await
            .expect("脚本执行失败");
        assert!(present, "引擎应当提供 {name}");
        assert!(
            declared.contains(&name.to_string()),
            "引擎提供了 {name}，但 engine.d.ts 没有声明：{declared:?}"
        );
    }
}

/// 引擎不该在声明文件里给能力命名空间留任何痕迹（那是使用方的事）。
#[test]
fn engine_spec_declares_no_capability_namespace() {
    let source = include_str!("../src/spec/engine.d.ts");
    // 按行首匹配（`interface ScriptConsole {` 这类别的声明不该被算进来）
    let declares_namespace = source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("declare const $:")
            || line.starts_with("declare const MyTool")
            || line.starts_with("interface MyTool {")
    });
    assert!(!declares_namespace, "引擎不应该声明任何能力命名空间");
}
