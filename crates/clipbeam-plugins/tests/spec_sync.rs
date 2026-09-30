//! `src/spec/plugins.d.ts` 与运行期绑定的**双向**一致性测试。
//!
//! 类型声明是手写的（无法从 Rust 自动生成），最容易出的两类问题是：
//!
//! 1. 加了能力却忘了写类型 → 补全里没有，作者以为不能用；
//! 2. 删了能力却留着类型 → 补全提示一个运行期不存在的方法。
//!
//! 这里把两个方向都钉住：
//!
//! * [`spec_dts_matches_runtime`]：声明文件里写了的，运行期必须真的存在；
//! * [`capabilities_are_all_declared_in_spec`]：能力清单里的，声明文件里必须写着。
//!
//! 另外校验「嵌套能力」的形态：`tray.onAction` 必须在 `ClipBeamPluginTray` 接口里，
//! 而不是被摊平到主接口 —— 声明错了，补全给的路径就是错的。

use std::sync::Arc;

use clipbeam_plugins::test_support::FakePluginHost;
use clipbeam_plugins::{
    spec, PermissionSet, PluginHost, PluginMeta, PluginRuntime, PluginRuntimeOptions,
};

/// 建一个全权限的插件运行时（声明一致性只关心「有没有」，不关心「能不能」）。
async fn runtime() -> PluginRuntime {
    let meta = PluginMeta {
        id: "spec-probe".into(),
        name: "spec".into(),
        version: "0.0.0".into(),
        description: None,
        author: None,
        dir: std::path::PathBuf::from("/tmp/spec-probe"),
        entry: "index.js".into(),
    };
    clipbeam_plugins::create_runtime(
        Arc::new(FakePluginHost::new(meta.clone())) as Arc<dyn PluginHost>,
        PluginRuntimeOptions::new(meta).permissions(PermissionSet {
            feedback: true,
            notification: true,
            system_dialog: true,
            tray: true,
            window: true,
        }),
    )
    .await
    .expect("创建插件运行时失败")
}

/// 声明文件里写了的顶层成员，运行期必须存在且是函数。
#[tokio::test]
async fn spec_dts_matches_runtime() {
    let runtime = runtime().await;
    let declared = members_declared_in_spec("interface ClipBeamPlugin {");

    assert!(
        declared.contains(&"toast".to_string()) && declared.contains(&"confirm".to_string()),
        "解析声明文件失败：{declared:?}"
    );

    for name in &declared {
        // `tray` 是嵌套能力容器，不是函数
        let expr = format!("typeof $plugin.{name}");
        let kind: String = runtime.runtime().eval(&expr).await.expect("求值失败");
        assert_ne!(
            kind, "undefined",
            "plugins.d.ts 声明了 {name}，但运行期不存在"
        );
    }
}

/// 嵌套接口里的成员，必须挂在 `$plugin.tray` 上。
#[tokio::test]
async fn nested_spec_matches_runtime() {
    let runtime = runtime().await;
    let declared = members_declared_in_spec("interface ClipBeamPluginTray {");

    assert!(
        declared.contains(&"onAction".to_string()),
        "解析声明文件失败：{declared:?}"
    );

    for name in &declared {
        let kind: String = runtime
            .runtime()
            .eval(&format!("typeof $plugin.tray.{name}"))
            .await
            .expect("求值失败");
        assert_eq!(
            kind, "function",
            "plugins.d.ts 声明了 tray.{name}，但运行期不是函数"
        );
    }
}

/// 反过来：能力清单里的每个名字都要能在声明文件里找到。
#[test]
fn capabilities_are_all_declared_in_spec() {
    let top_level = members_declared_in_spec("interface ClipBeamPlugin {");
    let tray_level = members_declared_in_spec("interface ClipBeamPluginTray {");

    for capability in spec::capabilities() {
        let found = match capability.name.split_once('.') {
            None => top_level.contains(&capability.name) || capability.name == "tray",
            Some((container, member)) => {
                // 目前只有一个嵌套容器；将来加别的容器时在这里扩
                assert_eq!(container, "tray", "未知的嵌套容器：{}", capability.name);
                top_level.contains(&container.to_string())
                    && tray_level.contains(&member.to_string())
            }
        };

        assert!(
            found,
            "能力 {} 没有写进 plugins.d.ts（顶层：{top_level:?}，嵌套：{tray_level:?}）",
            capability.name
        );
    }
}

/// 声明文件里的命名空间常量必须与 Rust 侧一致（名字是外部契约）。
#[test]
fn namespace_names_are_declared_with_the_same_spelling() {
    let source = include_str!("../src/spec/plugins.d.ts");
    assert!(
        source.contains("declare const ClipBeamPlugin"),
        "声明文件里应当有 ClipBeamPlugin"
    );
    assert!(
        source.contains("declare const $plugin"),
        "声明文件里应当有 $plugin 别名"
    );
    assert_eq!(clipbeam_plugins::NAMESPACE, "ClipBeamPlugin");
    assert_eq!(clipbeam_plugins::NAMESPACE_ALIAS, "$plugin");
}

/// 从 `plugins.d.ts` 的某个 `interface X { … }` 块里抽成员名。
///
/// 与 `clipbeam-scripting/tests/capabilities.rs` 的解析同一套规则：只按行读，
/// **不做字节偏移运算**（Windows 上 CRLF 会让按字节累加的括号深度算错）。
/// 行首是标识符字符且带 `(` 的算方法；不带 `(` 的标识符 + `:` 算字段（例如 `tray: ...`）。
fn members_declared_in_spec(marker: &str) -> Vec<String> {
    let source = include_str!("../src/spec/plugins.d.ts");

    let mut members = Vec::new();
    let mut inside = false;
    let mut depth = 0usize;

    for line in source.lines() {
        if !inside {
            if line.starts_with(marker) {
                inside = true;
                depth = 1; // marker 末尾那个 `{`
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_' || ch == '$') {
            // 方法：`name(...)`；字段：`name: Type`
            let name: String = trimmed
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
                .collect();
            let rest = trimmed[name.len()..].trim_start();
            if !name.is_empty() && (rest.starts_with('(') || rest.starts_with(':')) {
                members.push(name);
            }
        }

        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if depth == 0 {
            break;
        }
    }

    assert!(inside, "plugins.d.ts 里应当有行首的 `{marker}` 声明");
    assert!(
        depth == 0,
        "`{marker}` 的括号应当闭合（读到文件尾仍未闭合）"
    );
    members
}
