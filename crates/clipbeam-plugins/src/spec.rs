//! 能力清单：把 `$plugin` 的签名汇总成可序列化数据，交给前端与类型声明校验。
//!
//! # 只有一个真源
//!
//! 「插件能力有哪些、签名长什么样」的真源是各 [`script_engine::ScriptExtension::spec`]。
//! 本模块只做汇总：
//!
//! * 界面/前端可以从这里拿到清单（命名空间名字 + 全部能力）；
//! * `tests/spec_sync.rs` 用它校验手写的 `src/spec/plugins.d.ts` 不漂移；
//! * README 的能力表也从这里对照着写。
//!
//! 引擎**没有任何能力**（它只提供标准全局与扩展机制），所以这份清单就是 `$plugin` 的全部。

use serde::Serialize;

/// 一条能力（前端/文档消费的形态）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PluginCapability {
    /// 能力的完整路径，例如 `toast` 或 `tray.onAction`。
    pub name: String,
    /// 人类可读签名，例如 `toast(message: string, options?: ToastOptions) -> void`。
    pub signature: String,
    /// 一句话说明。
    pub doc: String,
    /// 需要哪一组权限（`permission.name()`）；顶层名字，便于界面分组展示。
    pub permission: String,
}

/// 一条能力清单 + 命名空间名字。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PluginCapabilityList {
    /// 能力命名空间的正式名（与 [`crate::NAMESPACE`] 一致）。
    pub namespace: String,
    /// 别名（与 [`crate::NAMESPACE_ALIAS`] 一致）。
    pub alias: String,
    /// 全部能力（按注册顺序）。
    pub capabilities: Vec<PluginCapability>,
}

/// 能力 → 权限组的映射（**唯一出处**，界面、文档、测试都读它）。
///
/// 新增能力时必须在这里补一行；`tests/spec_sync.rs` 会校验「清单里的每个能力都有权限归属」。
pub fn permission_of(name: &str) -> crate::Permission {
    match name {
        "toast" => crate::Permission::Feedback,
        "notify" => crate::Permission::Notification,
        "alert" | "confirm" => crate::Permission::SystemDialog,
        "tray.onAction" | "tray.setTooltip" | "tray.setBadge" => crate::Permission::Tray,
        other => panic!("能力 {other} 没有归到任何权限组：请更新 spec::permission_of"),
    }
}

/// 汇总本 crate 注入的全部能力（按注册顺序）。
pub fn capabilities() -> Vec<PluginCapability> {
    let mut all = Vec::new();

    for extension in crate::extensions::extensions() {
        for spec in extension.spec() {
            all.push(PluginCapability {
                name: spec.name.to_string(),
                signature: spec.signature.to_string(),
                doc: spec.doc.to_string(),
                permission: permission_of(spec.name).name().to_string(),
            });
        }
    }

    all
}

/// 前端消费的完整载荷：命名空间名字 + 全部能力。
pub fn capability_list() -> PluginCapabilityList {
    PluginCapabilityList {
        namespace: crate::NAMESPACE.to_string(),
        alias: crate::NAMESPACE_ALIAS.to_string(),
        capabilities: capabilities(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_permission_group() {
        for capability in capabilities() {
            assert!(
                !capability.permission.is_empty(),
                "{} 缺少权限归属",
                capability.name
            );
        }
    }

    #[test]
    fn list_carries_namespace_names() {
        let list = capability_list();
        assert_eq!(list.namespace, crate::NAMESPACE);
        assert_eq!(list.alias, crate::NAMESPACE_ALIAS);

        let names: Vec<&str> = list
            .capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "toast",
                "notify",
                "alert",
                "confirm",
                "tray.onAction",
                "tray.setTooltip",
                "tray.setBadge",
            ],
            "能力清单的顺序与内容变化会被前端与文档看到"
        );
    }

    #[test]
    fn list_is_serializable() {
        let json = serde_json::to_string(&capability_list()).expect("清单必须可序列化");
        assert!(json.contains("tray.onAction"), "{json}");
        assert!(json.contains("$plugin"), "{json}");
    }
}
