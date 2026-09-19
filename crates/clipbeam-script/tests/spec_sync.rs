//! `spec/clipbeam.d.ts` 与运行期绑定的一致性测试。
//!
//! 类型声明是**手写**的（TypeScript 的声明文件无法从 Rust 自动生成），因此容易漂移：
//! 加了绑定却忘了写声明，或删了绑定却没删声明。这里用最直接的方式兜底 ——
//! 解析声明文件里 `Clipbeam` 的方法名，断言它们在运行期真的存在。

use clipbeam_script::ScriptRuntime;

/// 从 `src/spec/clipbeam.d.ts` 的 `interface Clipbeam { ... }` 块里抽取方法名。
///
/// 只做最简单的文本解析：找 `interface Clipbeam`，花括号配对到结束，
/// 取形如 `name(` 的行首标识符。声明文件是我们自己写的，格式受控。
///
/// 为什么是 interface 而不是对象字面量类型：使用方（clipbeam-scripting）需要
/// 用接口声明合并追加自己的能力，两边的声明必须都是 interface。
fn methods_declared_in_spec() -> Vec<String> {
    // 精确匹配带花括号的形式：`interface ClipbeamConsole` 不会被误配，
    // 文档注释里的扩展示例写的是 `… { md5(…): string }`（单行），也不会命中。
    const MARKER: &str = "interface Clipbeam {";
    let source = include_str!("../src/spec/clipbeam.d.ts");
    // 只认「行首恰好是 MARKER」的那一行：文件顶部的说明里也写了同样的字样，
    // 但那是缩进过的示例（` * interface Clipbeam { md5(...) }`），不会命中。
    let open = source
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| line.starts_with(MARKER))
        .map(|(start, _)| start + MARKER.len() - 1)
        .expect("clipbeam.d.ts 里应当有行首的 `interface Clipbeam {` 声明");

    let mut depth = 0usize;
    let mut end = None;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("`interface Clipbeam {` 花括号应当闭合");

    let mut methods = Vec::new();
    for line in source[open + 1..end].lines() {
        let trimmed = line.trim();
        // 跳过注释与空行
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let Some(paren) = trimmed.find('(') else {
            continue;
        };
        let name: String = trimmed[..paren]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
            .collect();
        if !name.is_empty() {
            methods.push(name);
        }
    }
    methods
}

/// 声明文件里的每个方法都必须在运行期的 `$` 上存在。
#[tokio::test]
async fn declared_methods_exist_at_runtime() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");
    let declared = methods_declared_in_spec();

    assert!(
        declared.contains(&"file".to_string()) && declared.contains(&"sleep".to_string()),
        "解析声明文件失败，没读到预期方法：{declared:?}"
    );

    for name in &declared {
        let present: bool = runtime
            .eval(&format!("typeof $.{name} === 'function'"))
            .await
            .expect("脚本执行失败");
        assert!(
            present,
            "clipbeam.d.ts 声明了 {name}，但运行期 $.{name} 不存在"
        );
    }
}
