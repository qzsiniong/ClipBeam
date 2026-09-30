//! 插件宿主接口的错误类型。
//!
//! 与 `clipbeam-scripting` 的 `HostError` 同一个思路：**分类存在**，
//! 因为上层要按类型给不同文案（「已中止」/「当前环境不支持」/「权限不够」），
//! 而不是把一句自由文本透传给用户。

/// 插件能力失败的原因。
///
/// 需要 `Serialize` / `Deserialize`：托盘等能力要跨线程回执
/// （插件线程 → 主线程执行 → 结果回到插件线程），这条路上走的是 JSON 事件。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PluginError {
    /// 当前宿主不支持该能力（例如没有界面能力的宿主）。
    Unsupported,
    /// 插件没有在 `plugin.json` 里声明对应权限。
    ///
    /// 文案由 [`crate::permission::Permission`] 生成，带上插件 id 与能力名，
    /// 让用户一眼看出「该往清单里加什么」。
    Permission(String),
    /// 参数不合法（插件传了宿主无法理解的值）。
    InvalidArgument(String),
    /// 宿主侧动作失败，附带可直接展示的说明。
    Failed(String),
    /// 插件已被停用（宿主不再接受它的调用）。
    Stopped,
}

impl PluginError {
    /// 面向用户的中文说明。
    pub fn message(&self) -> String {
        match self {
            PluginError::Unsupported => "当前运行环境不支持该能力".to_string(),
            PluginError::Permission(detail) => detail.clone(),
            PluginError::InvalidArgument(detail) => detail.clone(),
            PluginError::Failed(detail) => detail.clone(),
            PluginError::Stopped => "插件已停用".to_string(),
        }
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for PluginError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_user_facing() {
        assert_eq!(
            PluginError::Unsupported.message(),
            "当前运行环境不支持该能力"
        );
        assert_eq!(PluginError::Stopped.message(), "插件已停用");
        assert_eq!(
            PluginError::Permission("缺 feedback 权限".into()).message(),
            "缺 feedback 权限"
        );
        assert_eq!(PluginError::Failed("炸了".into()).to_string(), "炸了");
    }
}
