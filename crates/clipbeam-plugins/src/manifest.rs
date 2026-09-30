//! 插件清单 `plugin.json`：解析、默认值与校验。
//!
//! # 为什么是清单文件而不是「约定文件名」
//!
//! 清单让「插件叫什么、入口在哪、要哪些权限、托盘菜单长什么样」变成**数据**，
//! 于是宿主不需要为每个插件写代码，界面也能把权限原样展示给用户。
//!
//! # 宽容与严格的分界
//!
//! * **未知字段不报错**：后续版本的字段可以先写进来，老版本忽略它（前向兼容）；
//! * **写错的字段名要报错**：`menu` 少了 s、`permission` 少了 s 这类笔误，
//!   靠 serde 会静默忽略 → 用户只会看到「菜单没出现」却找不到原因。
//!   因此这里显式列一份顶层字段白名单，命中未知字段就报错并提示最接近的合法名。
//! * 解析失败 / id 非法 / 目录名与 id 不一致 → 由调用方（[`crate::catalog`]）转成
//!   一条「无效」记录显示在界面上，**不静默跳过**。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::id::check_id;
use crate::permission::PermissionSet;

/// 入口文件的缺省名。
pub const DEFAULT_ENTRY: &str = "index.js";

/// 清单文件名。
pub const MANIFEST_FILE: &str = "plugin.json";

/// 顶层字段白名单（顺序即文档顺序）。未知字段会按这份名单给出「是不是想写 xxx」的提示。
const KNOWN_FIELDS: [&str; 8] = [
    "id",
    "name",
    "version",
    "description",
    "author",
    "entry",
    "permissions",
    "menus",
];

/// 一条托盘动作菜单项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMenuItem {
    /// 动作 id：插件内唯一，点击时随 `action.id` 传给插件。
    pub id: String,
    /// 菜单上显示的文字。
    pub label: String,
    /// 是否可点击；缺省 `true`（只用于「先占位、后实现」的菜单项）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// 插件清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// 插件 id（必须与目录名一致，且符合 [`crate::id`] 的规则）。
    pub id: String,
    /// 显示名；缺省用 [`PluginManifest::id`]。
    #[serde(default)]
    pub name: Option<String>,
    /// 版本号；缺省 `0.0.0`。
    #[serde(default)]
    pub version: Option<String>,
    /// 一句话说明（界面上展示）。
    #[serde(default)]
    pub description: Option<String>,
    /// 作者；缺省不显示。
    #[serde(default)]
    pub author: Option<String>,
    /// 入口文件（相对插件目录）；缺省 [`DEFAULT_ENTRY`]。
    #[serde(default)]
    pub entry: Option<String>,
    /// 能力声明；缺省全不开。
    #[serde(default)]
    pub permissions: PermissionSet,
    /// 托盘动作菜单；缺省空。
    #[serde(default)]
    pub menus: Vec<PluginMenuItem>,
}

/// 清单解析失败的种类（决定界面上的提示语气与是否需要用户动手改）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// `plugin.json` 读不到或读不了。
    Unreadable(String),
    /// JSON 语法错误。
    Syntax(String),
    /// 字段类型/取值错误（含未知字段）。
    Invalid(String),
}

impl ManifestError {
    /// 面向用户的说明。
    pub fn message(&self) -> String {
        match self {
            ManifestError::Unreadable(detail) => detail.clone(),
            ManifestError::Syntax(detail) => format!("{MANIFEST_FILE} 不是合法的 JSON：{detail}"),
            ManifestError::Invalid(detail) => detail.clone(),
        }
    }
}

impl PluginManifest {
    /// 解析一段清单文本（不做目录相关校验）。
    ///
    /// 未知顶层字段会被拒绝：它几乎总是笔误，而 serde 默认会静默忽略。
    pub fn from_json(text: &str) -> Result<Self, ManifestError> {
        reject_unknown_fields(text)?;

        let manifest: PluginManifest =
            serde_json::from_str(text).map_err(|err| ManifestError::Invalid(err.to_string()))?;

        if let Err(detail) = check_id(&manifest.id) {
            return Err(ManifestError::Invalid(detail));
        }

        Ok(manifest)
    }

    /// 从 `<dir>/plugin.json` 读取并解析。
    pub fn load(dir: &Path) -> Result<Self, ManifestError> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|err| {
            ManifestError::Unreadable(format!("读取 {} 失败：{err}", path.display()))
        })?;

        let manifest = Self::from_json(&text)?;
        manifest.validate(dir).map_err(ManifestError::Invalid)?;
        Ok(manifest)
    }

    /// 显示名（缺省回落到 id）。
    pub fn display_name(&self) -> &str {
        match self.name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.id,
        }
    }

    /// 版本号（缺省 `0.0.0`）。
    pub fn version_or_default(&self) -> &str {
        match self.version.as_deref() {
            Some(version) if !version.trim().is_empty() => version,
            _ => "0.0.0",
        }
    }

    /// 入口文件名（缺省 [`DEFAULT_ENTRY`]）。
    pub fn entry_name(&self) -> &str {
        match self.entry.as_deref() {
            Some(entry) if !entry.trim().is_empty() => entry,
            _ => DEFAULT_ENTRY,
        }
    }

    /// 目录相关校验：目录名与 id 一致、入口存在、manifest 自身的内部一致性。
    ///
    /// 这些必须**在启用之前**查出来：等到跑入口才发现，用户拿到的是一个莫名其妙的
    /// JS 报错，而不是「你的入口文件不在」。
    pub fn validate(&self, dir: &Path) -> Result<(), String> {
        // 目录名必须等于 id：界面上看到的插件与磁盘目录永远能对上
        if let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) {
            if dir_name != self.id {
                return Err(format!(
                    "插件 id {:?} 与目录名 {dir_name:?} 不一致（两者必须相同）",
                    self.id
                ));
            }
        }

        // 入口必须是目录内的普通文件名：不允许子目录或跳出目录
        let entry = self.entry_name();
        if entry.contains('/') || entry.contains('\\') || entry.contains("..") {
            return Err(format!(
                "入口 {entry:?} 不合法：只能是插件目录内的文件名（不含路径分隔符）"
            ));
        }
        if !is_supported_entry(entry) {
            return Err(format!(
                "入口 {entry:?} 的扩展名不支持：需要 {} 之一",
                crate::runtime::ENTRY_EXTENSIONS.join(" / ")
            ));
        }
        if !dir.join(entry).is_file() {
            return Err(format!("找不到入口文件 {entry:?}（在 {}）", dir.display()));
        }

        // 菜单项 id 必须唯一且合法：点击事件靠它路由
        let mut seen: Vec<&str> = Vec::new();
        for item in &self.menus {
            if !crate::id::is_valid_id(&item.id) {
                return Err(format!(
                    "菜单项 id {:?} 不合法（规则与插件 id 相同）",
                    item.id
                ));
            }
            if seen.contains(&item.id.as_str()) {
                return Err(format!("菜单项 id {:?} 重复", item.id));
            }
            if item.label.trim().is_empty() {
                return Err(format!("菜单项 {:?} 的 label 不能为空", item.id));
            }
            seen.push(&item.id);
        }

        // 声明了 menus 却没开 tray 权限：这是个必然"不生效"的配置，直接拦下
        if !self.menus.is_empty() && !self.permissions.tray {
            return Err(format!(
                "声明了 {} 个托盘菜单项，但没有打开 tray 权限（在 permissions.tray 里声明）",
                self.menus.len()
            ));
        }

        Ok(())
    }
}

/// 入口扩展名是否受支持（与 `script-engine::ts` 的转译范围保持一致）。
fn is_supported_entry(entry: &str) -> bool {
    let extension = Path::new(entry)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension {
        Some(ext) => crate::runtime::ENTRY_EXTENSIONS.contains(&ext.as_str()),
        None => false,
    }
}

/// 拒绝未知顶层字段：serde 默认静默忽略，而笔误必须当场看见。
fn reject_unknown_fields(text: &str) -> Result<(), ManifestError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| ManifestError::Syntax(err.to_string()))?;

    let Some(object) = value.as_object() else {
        return Err(ManifestError::Invalid(format!(
            "{MANIFEST_FILE} 的顶层必须是一个对象"
        )));
    };

    for key in object.keys() {
        if KNOWN_FIELDS.contains(&key.as_str()) {
            continue;
        }

        let hint = closest_field(key);
        return Err(ManifestError::Invalid(match hint {
            Some(hint) => format!("{MANIFEST_FILE} 里有未知字段 {key:?}，是不是想写 {hint:?}？"),
            None => format!(
                "{MANIFEST_FILE} 里有未知字段 {key:?}；可用字段：{}",
                KNOWN_FIELDS.join(" / ")
            ),
        }));
    }

    Ok(())
}

/// 找一个最接近的合法字段名（前缀包含关系即可，够用且不会乱猜）。
fn closest_field(key: &str) -> Option<&'static str> {
    let key = key.to_ascii_lowercase();
    KNOWN_FIELDS
        .into_iter()
        .find(|known| known.starts_with(&key) || key.starts_with(*known))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        // 序号而不是固定名字：测试并行跑，同名目录会互相踩
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-manifest-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    #[test]
    fn minimal_manifest_gets_defaults() {
        let manifest = PluginManifest::from_json(r#"{ "id": "demo" }"#).expect("解析失败");
        assert_eq!(manifest.display_name(), "demo", "缺省显示名回落到 id");
        assert_eq!(manifest.version_or_default(), "0.0.0");
        assert_eq!(manifest.entry_name(), DEFAULT_ENTRY);
        assert_eq!(manifest.permissions, PermissionSet::NONE, "缺省不开权限");
        assert!(manifest.menus.is_empty());
    }

    #[test]
    fn full_manifest_roundtrips() {
        let text = r#"{
            "id": "hello-plugin",
            "name": "示例插件",
            "version": "1.2.3",
            "description": "说明",
            "author": "ClipBeam",
            "entry": "main.ts",
            "permissions": { "feedback": true, "tray": true },
            "menus": [{ "id": "hello", "label": "打个招呼" }]
        }"#;

        let manifest = PluginManifest::from_json(text).expect("解析失败");
        assert_eq!(manifest.id, "hello-plugin");
        assert_eq!(manifest.display_name(), "示例插件");
        assert_eq!(manifest.version_or_default(), "1.2.3");
        assert_eq!(manifest.entry_name(), "main.ts");
        assert!(manifest.permissions.feedback && manifest.permissions.tray);
        assert!(!manifest.permissions.window);
        assert_eq!(manifest.menus.len(), 1);
        assert_eq!(manifest.menus[0].id, "hello");
        assert_eq!(manifest.menus[0].enabled, None, "缺省不写 enabled");

        // 再序列化再解析，结果必须一致（清单也要能被宿主写回）
        let json = serde_json::to_string(&manifest).expect("序列化失败");
        let back = PluginManifest::from_json(&json).expect("回读失败");
        assert_eq!(manifest, back);
    }

    #[test]
    fn unknown_top_level_field_is_rejected_with_a_hint() {
        let err = PluginManifest::from_json(r#"{ "id": "demo", "menu": [] }"#)
            .expect_err("未知字段应当报错");
        let text = err.message();
        assert!(text.contains("menu"), "应当指出未知字段：{text}");
        assert!(text.contains("menus"), "应当给出最接近的合法字段：{text}");

        // 完全不像的字段也要报错，并列出可用字段
        let err = PluginManifest::from_json(r#"{ "id": "demo", "whatever": 1 }"#)
            .expect_err("未知字段应当报错");
        assert!(err.message().contains("permissions"), "应当列出可用字段");
    }

    #[test]
    fn invalid_ids_are_rejected() {
        for bad in ["", "..", "a/b", ".hidden", "插件"] {
            let text = format!(r#"{{ "id": {bad:?} }}"#);
            assert!(
                PluginManifest::from_json(&text).is_err(),
                "id {bad:?} 应当被拒绝"
            );
        }
    }

    #[test]
    fn broken_json_is_reported_as_syntax_error() {
        let err = PluginManifest::from_json("{ oops }").expect_err("语法错误应当报错");
        assert!(matches!(err, ManifestError::Syntax(_)));
        assert!(err.message().contains("合法的 JSON"));
    }

    #[test]
    fn non_object_manifest_is_rejected() {
        let err = PluginManifest::from_json("[]").expect_err("数组不是清单");
        assert!(err.message().contains("对象"));
    }

    #[test]
    fn directory_name_must_match_id() {
        let dir = temp_dir("mismatch");
        std::fs::write(dir.join("plugin.json"), r#"{ "id": "other" }"#).unwrap();
        std::fs::write(dir.join(DEFAULT_ENTRY), "").unwrap();

        let err = PluginManifest::load(&dir).expect_err("目录名不一致应当报错");
        assert!(err.message().contains("不一致"), "{}", err.message());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_entry_file_is_rejected() {
        let dir = temp_dir("no-entry");
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(
            dir.join("plugin.json"),
            format!(r#"{{ "id": {name:?}, "entry": "nope.js" }}"#),
        )
        .unwrap();

        let err = PluginManifest::load(&dir).expect_err("缺入口应当报错");
        assert!(err.message().contains("找不到入口"), "{}", err.message());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entry_cannot_escape_the_plugin_directory() {
        let dir = temp_dir("escape");
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        for entry in ["../evil.js", "sub/x.js"] {
            std::fs::write(
                dir.join("plugin.json"),
                format!(r#"{{ "id": {name:?}, "entry": {entry:?} }}"#),
            )
            .unwrap();
            let err = PluginManifest::load(&dir).expect_err("跳出目录的入口应当被拒绝");
            assert!(err.message().contains("入口"), "{}", err.message());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menus_require_ids_and_labels() {
        let dir = temp_dir("menus");
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(dir.join(DEFAULT_ENTRY), "").unwrap();

        // id 重复
        std::fs::write(
            dir.join("plugin.json"),
            format!(
                r#"{{ "id": {name:?}, "permissions": {{"tray": true}},
                     "menus": [{{"id": "a", "label": "A"}}, {{"id": "a", "label": "B"}}] }}"#
            ),
        )
        .unwrap();
        let err = PluginManifest::load(&dir).expect_err("重复 id 应当报错");
        assert!(err.message().contains("重复"), "{}", err.message());

        // label 为空
        std::fs::write(
            dir.join("plugin.json"),
            format!(
                r#"{{ "id": {name:?}, "permissions": {{"tray": true}},
                     "menus": [{{"id": "a", "label": "  "}}] }}"#
            ),
        )
        .unwrap();
        let err = PluginManifest::load(&dir).expect_err("空 label 应当报错");
        assert!(err.message().contains("label"), "{}", err.message());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menus_without_tray_permission_are_rejected() {
        let dir = temp_dir("menus-perm");
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(dir.join(DEFAULT_ENTRY), "").unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(r#"{{ "id": {name:?}, "menus": [{{"id": "a", "label": "A"}}] }}"#),
        )
        .unwrap();

        let err = PluginManifest::load(&dir).expect_err("未声明 tray 权限应当报错");
        assert!(err.message().contains("tray"), "{}", err.message());

        // 打开权限后就通过
        std::fs::write(
            dir.join("plugin.json"),
            format!(
                r#"{{ "id": {name:?}, "permissions": {{"tray": true}},
                     "menus": [{{"id": "a", "label": "A"}}] }}"#
            ),
        )
        .unwrap();
        assert!(PluginManifest::load(&dir).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_manifest_is_reported_clearly() {
        let dir = temp_dir("no-manifest");
        let err = PluginManifest::load(&dir).expect_err("缺清单应当报错");
        assert!(matches!(err, ManifestError::Unreadable(_)));
        assert!(err.message().contains(MANIFEST_FILE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空白 name/version/entry 视为「没写」，回落到缺省值。
    #[test]
    fn blank_optional_strings_fall_back_to_defaults() {
        let manifest = PluginManifest::from_json(
            r#"{ "id": "demo", "name": "  ", "version": "", "entry": " " }"#,
        )
        .expect("解析失败");
        assert_eq!(manifest.display_name(), "demo");
        assert_eq!(manifest.version_or_default(), "0.0.0");
        assert_eq!(manifest.entry_name(), DEFAULT_ENTRY);
    }
}
