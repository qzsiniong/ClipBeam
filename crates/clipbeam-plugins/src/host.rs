//! 插件宿主接口：插件能力落地到「外部世界」的那一层。
//!
//! # 为什么与 `ScriptHost` 分开
//!
//! `clipbeam-scripting` 的 `ScriptHost` 是**脚本**的宿主接口：`type_str`（往当前焦点窗口
//! 逐键打字）、`request_focus`（待命窗口）、`pick_path`、文件授权。那套语义服务于
//! 「用户点一次、脚本跑一次」的场景，插件不该继承：
//!
//! * 插件是常驻的，会在后台被触发 —— 「往当前焦点窗口打字」在后台触发时是危险的；
//! * 「待命窗口」「焦点锁定」是键盘注入专属的风险控制，和插件无关；
//! * 反过来，插件需要的是「应用内提示」「创建窗口」「自己的托盘图标」，脚本不需要。
//!
//! 所以这里独立定义 [`PluginHost`]。代价是两套 trait，收益是两边都不会被对方的需求撑肿，
//! 而且 `clipbeam-plugins` 不必反向依赖 `clipbeam-scripting`。
//!
//! # 实现方在哪
//!
//! GUI 的实现（`TauriPluginHost`）在 `src-tauri`；测试用的是本 crate 里的
//! [`crate::test_support::FakePluginHost`]。前者需要 Tauri，后者不需要 ——
//! 所以能力语义（权限拒绝、参数校验、错误文案）可以在没有图形环境的机器上跑测试。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// 插件的身份与位置（宿主与插件都能读到）。
///
/// `dir` 会作为 `meta.dir` 交给插件：插件自己的配置文件、图标、页面都放在这个目录里，
/// 宿主不规定插件怎么用它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMeta {
    /// 插件 id（同时是目录名）。
    pub id: String,
    /// 显示名（缺省等于 id，见 [`crate::manifest::PluginManifest::display_name`]）。
    pub name: String,
    /// 版本号。
    pub version: String,
    /// 一句话说明。
    pub description: Option<String>,
    /// 作者。
    pub author: Option<String>,
    /// 插件目录的绝对路径。
    pub dir: PathBuf,
    /// 入口文件名（相对 `dir`）。
    pub entry: String,
}

impl PluginMeta {
    /// 插件目录的字符串形式（给 JS 侧用；拿不到 UTF-8 时返回 `None`）。
    pub fn dir_str(&self) -> Option<&str> {
        self.dir.to_str()
    }
}

/// toast 等级（决定前端配色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastLevel {
    /// 普通信息（默认）。
    Info,
    /// 成功。
    Success,
    /// 警告。
    Warning,
    /// 错误。
    Error,
}

impl ToastLevel {
    /// 从 JS 侧传入的字符串解析；`None`/空串用默认等级。
    ///
    /// 未知取值**报错**而不是静默回落：`level: "warn"` 这种想当然的写法应该当场发现
    /// （正确值是 `"warning"`）。
    pub fn parse(value: Option<&str>) -> Result<Self, PluginError> {
        match value.map(str::trim) {
            None | Some("") | Some("info") => Ok(ToastLevel::Info),
            Some("success") => Ok(ToastLevel::Success),
            Some("warning") => Ok(ToastLevel::Warning),
            Some("error") => Ok(ToastLevel::Error),
            Some(other) => Err(PluginError::InvalidArgument(format!(
                "不认识的通知等级 {other:?}：可用 info / success / warning / error"
            ))),
        }
    }
}

/// 系统原生对话框的按钮组合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DialogButtons {
    /// 只有一个「好」。
    Ok,
    /// 「取消 / 确定」，确定是主按钮。
    OkCancel,
    /// 自定义文案（结果按位置回传：primary → true，secondary → false，tertiary → null）。
    Custom {
        /// 主按钮文案。
        primary: String,
        /// 次按钮文案。
        secondary: Option<String>,
        /// 第三个按钮文案（结果按「关闭」处理，返回 null）。
        tertiary: Option<String>,
    },
}

/// 用户对对话框的回答（映射到 JS 的 `boolean | null`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogChoice {
    /// 主按钮（「好」/「确定」/ 自定义主按钮）。
    Primary,
    /// 次按钮（「取消」/ 自定义次按钮）。
    Secondary,
    /// 第三个按钮。
    Tertiary,
    /// 用户直接关掉了对话框。
    Dismissed,
    /// 超时（宿主侧限时返回，绝不永久阻塞插件线程）。
    Timeout,
}

/// 一次「反馈」请求：应用内提示、系统通知、系统对话框。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Feedback {
    /// 应用内 toast。
    Toast {
        /// 等级（决定配色）。
        level: ToastLevel,
        /// 文本。
        message: String,
        /// 自动消失时间（毫秒）；`None` 用宿主默认。
        #[serde(rename = "durationMs")]
        duration_ms: Option<u64>,
    },
    /// 系统通知。
    Notify {
        /// 标题。
        title: String,
        /// 正文；`None` 表示只要标题。
        body: Option<String>,
    },
    /// 系统原生对话框。
    Dialog {
        /// 标题（宿主可给默认标题）。
        title: Option<String>,
        /// 正文。
        message: String,
        /// 按钮组合。
        buttons: DialogButtons,
        /// 超时（毫秒）；`None` 表示不超时。
        #[serde(rename = "timeoutMs")]
        timeout_ms: Option<u64>,
    },
}

/// 反馈请求的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FeedbackOutcome {
    /// 已投递（toast / 系统通知：不等待用户）。
    Delivered,
    /// 用户做了选择。
    Chosen {
        /// 回答。
        choice: DialogChoice,
    },
}

/// 托盘的运行时控制请求。
///
/// 动作菜单（`menus`）**不走这里** —— 它由宿主在启用插件时按清单一次性建好，
/// 这里只处理「插件跑起来之后想改一改托盘」的几种情况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TrayRequest {
    /// 设置鼠标悬停提示。
    SetTooltip {
        /// 提示文字。
        text: String,
    },
    /// 设置图标（相对插件目录的路径）；`None` 恢复默认图标。
    ///
    /// 图标路径由宿主按插件目录解析并做越界校验（见 `src-tauri/src/plugin_window.rs`）。
    SetIcon {
        /// 相对插件目录的路径。
        path: Option<String>,
    },
    /// 设置徽标文字（macOS 菜单栏支持）；`None` 清除。
    SetBadge {
        /// 徽标文字。
        text: Option<String>,
    },
}

/// 托盘请求的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TrayOutcome {
    /// 已应用。
    Applied,
}

/// 插件与外部世界交互的唯一入口。
///
/// 实现必须是 `Send + Sync`：插件跑在自己的线程上，能力调用可能来自任意线程。
///
/// 默认实现一律返回 [`PluginError::Unsupported`]，实现方按需覆写 —— 这样「只想跑纯计算插件」
/// 的嵌入场景（测试、headless）不必写一堆空方法，而调用了不支持能力时拿到的是明确错误，
/// 不是 panic、也不是静默成功。
pub trait PluginHost: Send + Sync + 'static {
    /// 插件身份与位置。
    fn meta(&self) -> &PluginMeta;

    /// 投递一次反馈（toast / 系统通知 / 对话框）。
    fn feedback(&self, _request: Feedback) -> Result<FeedbackOutcome, PluginError> {
        Err(PluginError::Unsupported)
    }

    /// 托盘运行时控制。
    fn tray(&self, _request: TrayRequest) -> Result<TrayOutcome, PluginError> {
        Err(PluginError::Unsupported)
    }

    /// 插件自己的日志（宿主可以把它落到插件日志面板）。
    ///
    /// 默认空实现：`console.*` 走引擎的 `ConsoleHook`，这个方法留给宿主想额外记的场合。
    fn log(&self, _level: &str, _message: &str) {}

    /// 插件是否已被要求停止（宿主在停用流程里置位）。
    ///
    /// 能力实现应当在长耗时动作前后检查它，尽早收手。
    fn stopped(&self) -> bool {
        false
    }
}

/// 插件清单声明的**动作表**的登记处。
///
/// 由 `$plugin.tray.onAction(cb)` 在插件入口里登记：扩展把回调存进上下文（见
/// [`crate::context`]），同时在这里记下 `key → 全局函数名`。
///
/// # 两种 key
///
/// * **具体动作 id**（`menus[].id`）：精确派发；
/// * **接口级 key**（[`GENERIC_ACTION_KEY`]）：「这个接口就一个回调，用载荷里的 `id` 自己分」。
///
/// 宿主派发时先找具体 id，再回落到接口级 key —— 于是插件既可以为每个菜单项各写一个回调，
/// 也可以只写一个（后者更常见：托盘菜单项只是入口，具体做什么由 `action.id` 决定）。
///
/// 没有登记的动作意味着「插件声明了菜单项却没实现回调」—— 这种不一致必须在界面/日志里
/// 看得见（`eval_action` 会因为找不到全局函数而报错，而不是静默什么都不做）。
#[derive(Debug, Default)]
pub struct ActionRegistry {
    inner: std::sync::Mutex<Vec<(String, String)>>,
}

/// 接口级动作 key：`$plugin.tray.onAction` 登记到这个名字下。
pub const GENERIC_ACTION_KEY: &str = "*";

impl ActionRegistry {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记下一个动作（同一 key 重复登记时以后者为准，便于插件重载）。
    pub fn register(&self, key: &str, global_name: &str) {
        let mut items = self.inner.lock().unwrap();
        items.retain(|(existing, _)| existing != key);
        items.push((key.to_string(), global_name.to_string()));
    }

    /// 精确查一个 key 对应的全局函数名。
    pub fn global_name(&self, key: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, name)| name.clone())
    }

    /// 派发查表：先精确匹配 `action_id`，再回落到接口级 key。
    ///
    /// 返回值同时带上「命中的是哪一个 key」，方便宿主在日志里说清「这个动作由谁处理」。
    pub fn resolve(&self, action_id: &str) -> Option<(String, String)> {
        if let Some(name) = self.global_name(action_id) {
            return Some((action_id.to_string(), name));
        }
        self.global_name(GENERIC_ACTION_KEY)
            .map(|name| (GENERIC_ACTION_KEY.to_string(), name))
    }

    /// 全部已登记的 key（按登记顺序）。
    pub fn keys(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// 是否一个动作都没登记。
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_level_parsing_is_strict_about_unknown_values() {
        assert_eq!(ToastLevel::parse(None).unwrap(), ToastLevel::Info);
        assert_eq!(ToastLevel::parse(Some("")).unwrap(), ToastLevel::Info);
        assert_eq!(ToastLevel::parse(Some("info")).unwrap(), ToastLevel::Info);
        assert_eq!(
            ToastLevel::parse(Some("success")).unwrap(),
            ToastLevel::Success
        );
        assert_eq!(
            ToastLevel::parse(Some("warning")).unwrap(),
            ToastLevel::Warning
        );
        assert_eq!(ToastLevel::parse(Some("error")).unwrap(), ToastLevel::Error);

        // 常见想当然写法要报错，而不是静默当成 info
        let err = ToastLevel::parse(Some("warn")).expect_err("未知等级应当报错");
        assert!(err.message().contains("warn"), "{}", err.message());
        assert!(
            err.message().contains("warning"),
            "错误里应当列出可用取值：{}",
            err.message()
        );
    }

    #[test]
    fn action_registry_registers_looks_up_and_replaces() {
        let registry = ActionRegistry::new();
        assert!(registry.is_empty());

        registry.register("hello", "__action_0");
        registry.register("count", "__action_1");
        assert_eq!(registry.keys(), vec!["hello", "count"]);
        assert_eq!(registry.global_name("hello").as_deref(), Some("__action_0"));
        assert!(registry.global_name("missing").is_none());

        // 同一 key 重新登记（插件重载）应当覆盖，而不是留下两条
        registry.register("hello", "__action_2");
        assert_eq!(registry.global_name("hello").as_deref(), Some("__action_2"));
        assert_eq!(registry.keys(), vec!["count", "hello"]);
    }

    /// 派发查表：具体 id 优先，没有就回落到接口级 key（`*`）。
    #[test]
    fn action_registry_resolves_specific_id_then_falls_back_to_generic() {
        let registry = ActionRegistry::new();
        registry.register(GENERIC_ACTION_KEY, "__generic");
        registry.register("hello", "__specific");

        assert_eq!(
            registry.resolve("hello"),
            Some(("hello".to_string(), "__specific".to_string())),
            "具体 id 应当优先"
        );
        assert_eq!(
            registry.resolve("count"),
            Some((GENERIC_ACTION_KEY.to_string(), "__generic".to_string())),
            "没登记具体 id 时回落到接口级回调"
        );

        let empty = ActionRegistry::new();
        assert!(empty.resolve("hello").is_none(), "空表什么都不命中");
    }

    #[test]
    fn feedback_payloads_serialize_with_a_kind_tag() {
        // 前端与宿主之间靠这份 JSON 通信：tag 形状变了就是协议破坏，这里钉死
        let toast = Feedback::Toast {
            level: ToastLevel::Success,
            message: "你好".into(),
            duration_ms: None,
        };
        let json = serde_json::to_string(&toast).unwrap();
        assert!(json.contains(r#""kind":"toast""#), "{json}");
        assert!(json.contains(r#""level":"success""#), "{json}");

        let dialog = Feedback::Dialog {
            title: None,
            message: "继续吗".into(),
            buttons: DialogButtons::OkCancel,
            timeout_ms: Some(1000),
        };
        let json = serde_json::to_string(&dialog).unwrap();
        assert!(json.contains(r#""kind":"dialog""#), "{json}");
        assert!(json.contains(r#""kind":"okCancel""#), "{json}");
        assert!(json.contains(r#""timeoutMs":1000"#), "{json}");

        let outcome = FeedbackOutcome::Chosen {
            choice: DialogChoice::Secondary,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains(r#""choice":"secondary""#), "{json}");
    }

    /// 默认实现必须是「明确不支持」，不能是静默成功。
    #[test]
    fn unoverridden_capabilities_report_unsupported() {
        struct Bare;

        impl PluginHost for Bare {
            fn meta(&self) -> &PluginMeta {
                unreachable!("本测试不查 meta")
            }
        }

        let host = Bare;
        assert!(matches!(
            host.feedback(Feedback::Notify {
                title: "x".into(),
                body: None
            }),
            Err(PluginError::Unsupported)
        ));
        assert!(matches!(
            host.tray(TrayRequest::SetBadge { text: None }),
            Err(PluginError::Unsupported)
        ));
        assert!(!host.stopped());
    }
}
