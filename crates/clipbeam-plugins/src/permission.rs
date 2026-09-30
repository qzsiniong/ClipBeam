//! 权限模型：插件在 `plugin.json` 里**声明**要用哪些能力，宿主在每次调用时**拒绝未声明的**。
//!
//! # 这不是沙箱
//!
//! 插件与 ClipBeam 同进程运行，能读写文件 —— 权限声明挡不住恶意代码。
//! 它的价值在另外两处：
//!
//! 1. **可审计**：一个插件的「能力清单」在界面上、在清单文件里都是明确的；
//! 2. **误用当场暴露**：忘了声明就调用 → 立刻收到一条说明「该往清单里加什么」的异常，
//!    而不是功能静默失效（静默失效是最难排查的一类问题）。
//!
//! # 默认全不开
//!
//! 清单里不写 = 没有权限。新增能力时必须同时改这里与 [`Permission::name`]，
//! 测试会逐条校验映射，避免「加了 API 忘了权限」。

use serde::{Deserialize, Serialize};

/// 一个权限对应的能力组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// `$plugin.toast`
    Feedback,
    /// `$plugin.notify`（系统通知）
    Notification,
    /// `$plugin.alert` / `$plugin.confirm`（系统原生对话框）
    SystemDialog,
    /// `$plugin.tray.*` 与清单里的 `menus`
    Tray,
    /// `$plugin.window.*`
    ///
    /// 插件可以开自己的窗口（页面放在插件目录里，见 `plugin.md` §5.4）。
    /// 不需要在清单里额外声明什么：**页面路径由 `$plugin.window.open` 的参数给出**，
    /// 宿主按插件目录解析并做越界校验。
    Window,
}

impl Permission {
    /// 全部权限（顺序同时是清单解析与界面展示的顺序）。
    pub const ALL: [Permission; 5] = [
        Permission::Feedback,
        Permission::Notification,
        Permission::SystemDialog,
        Permission::Tray,
        Permission::Window,
    ];

    /// 清单 JSON 里的字段名，也是界面与日志里用的名字。
    pub const fn name(self) -> &'static str {
        match self {
            Permission::Feedback => "feedback",
            Permission::Notification => "notification",
            Permission::SystemDialog => "system_dialog",
            Permission::Tray => "tray",
            Permission::Window => "window",
        }
    }

    /// 面向用户的说明（界面上鼠标悬停时展示）。
    pub const fn doc(self) -> &'static str {
        match self {
            Permission::Feedback => "在应用窗口里弹提示（$plugin.toast）",
            Permission::Notification => "发系统通知（$plugin.notify）",
            Permission::SystemDialog => "弹系统原生对话框（$plugin.alert / $plugin.confirm）",
            Permission::Tray => "在托盘菜单里加动作（$plugin.tray、menus）",
            Permission::Window => "创建自己的窗口（$plugin.window）",
        }
    }

    /// 「拒绝」时的完整文案：说清**哪个插件、哪个能力、缺哪一组权限**。
    pub fn denied(self, plugin_id: &str, capability: &str) -> String {
        format!(
            "插件 {plugin_id} 未声明 {} 权限，无法调用 {capability}（在 plugin.json 的 permissions.{} 里声明）",
            self.name(),
            self.name()
        )
    }
}

/// 清单里的 `permissions` 字段。
///
/// 每个开关缺省 `false`：**不写就是没有**。这与「插件是用户主动装进来的、但能力不该默认全开」
/// 的取舍一致 —— 让作者明确写出自己需要什么。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet {
    /// [`Permission::Feedback`]
    #[serde(default)]
    pub feedback: bool,
    /// [`Permission::Notification`]
    #[serde(default)]
    pub notification: bool,
    /// [`Permission::SystemDialog`]
    #[serde(default)]
    pub system_dialog: bool,
    /// [`Permission::Tray`]
    #[serde(default)]
    pub tray: bool,
    /// [`Permission::Window`]
    #[serde(default)]
    pub window: bool,
}

impl PermissionSet {
    /// 一个都没开（默认）。
    pub const NONE: PermissionSet = PermissionSet {
        feedback: false,
        notification: false,
        system_dialog: false,
        tray: false,
        window: false,
    };

    /// 是否包含某权限。
    pub const fn has(&self, permission: Permission) -> bool {
        match permission {
            Permission::Feedback => self.feedback,
            Permission::Notification => self.notification,
            Permission::SystemDialog => self.system_dialog,
            Permission::Tray => self.tray,
            Permission::Window => self.window,
        }
    }

    /// 已开启的权限列表（界面上按这个渲染标签）。
    pub fn granted(&self) -> Vec<Permission> {
        Permission::ALL
            .into_iter()
            .filter(|permission| self.has(*permission))
            .collect()
    }

    /// 检查权限，未声明时返回带文案的错误。
    ///
    /// `capability` 是用户看到的 API 名字（例如 `$plugin.toast`），
    /// 这样错误信息能直接指出「你调的是哪个方法」。
    pub fn require(
        &self,
        permission: Permission,
        plugin_id: &str,
        capability: &str,
    ) -> Result<(), crate::PluginError> {
        if self.has(permission) {
            return Ok(());
        }
        Err(crate::PluginError::Permission(
            permission.denied(plugin_id, capability),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 权限名不能重复（清单字段名、界面标签、错误文案都用它）。
    #[test]
    fn permission_names_are_unique_and_stable() {
        let names: Vec<&str> = Permission::ALL.iter().map(|p| p.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "权限名不应重复：{names:?}");
        // 清单字段名是外部契约，改名等于破坏兼容 —— 这里钉死
        assert_eq!(
            names,
            vec![
                "feedback",
                "notification",
                "system_dialog",
                "tray",
                "window"
            ]
        );
    }

    /// 每个权限都要有自己的说明与拒绝文案（避免新增权限时漏写）。
    #[test]
    fn every_permission_has_doc_and_denial_text() {
        for permission in Permission::ALL {
            assert!(
                !permission.doc().is_empty(),
                "{} 缺少说明",
                permission.name()
            );
            let text = permission.denied("demo", "$plugin.x");
            assert!(text.contains("demo"), "拒绝文案应带插件 id：{text}");
            assert!(text.contains("$plugin.x"), "拒绝文案应带能力名：{text}");
            assert!(
                text.contains(&format!("permissions.{}", permission.name())),
                "拒绝文案应指出该改清单哪一项：{text}"
            );
        }
    }

    /// 缺省全不开；JSON 里只写一项时其余仍为 false。
    #[test]
    fn default_is_all_denied_and_set_is_sparse() {
        assert_eq!(PermissionSet::default(), PermissionSet::NONE);
        assert!(PermissionSet::default().granted().is_empty());

        let parsed: PermissionSet =
            serde_json::from_str(r#"{"feedback": true}"#).expect("解析失败");
        assert!(parsed.feedback);
        assert!(!parsed.tray, "没写的项必须是 false");
        assert_eq!(parsed.granted(), vec![Permission::Feedback]);
    }

    /// 每个权限都必须能单独打开（`has` 的分支不能漏）。
    #[test]
    fn each_permission_can_be_granted_individually() {
        for permission in Permission::ALL {
            let mut set = PermissionSet::NONE;
            match permission {
                Permission::Feedback => set.feedback = true,
                Permission::Notification => set.notification = true,
                Permission::SystemDialog => set.system_dialog = true,
                Permission::Tray => set.tray = true,
                Permission::Window => set.window = true,
            }
            assert!(set.has(permission), "{} 没被打开", permission.name());
            assert_eq!(set.granted(), vec![permission]);
        }
    }

    /// `require` 放行已声明的、拒绝未声明的。
    #[test]
    fn require_enforces_declaration() {
        let none = PermissionSet::NONE;
        let err = none
            .require(Permission::Feedback, "demo", "$plugin.toast")
            .expect_err("未声明应当被拒绝");
        assert!(matches!(err, crate::PluginError::Permission(_)));

        let set = PermissionSet {
            feedback: true,
            ..PermissionSet::NONE
        };
        assert!(set
            .require(Permission::Feedback, "demo", "$plugin.toast")
            .is_ok());
        assert!(set
            .require(Permission::Tray, "demo", "$plugin.tray.onAction")
            .is_err());
    }
}
