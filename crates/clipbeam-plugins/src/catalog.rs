//! 插件目录的发现：扫描 `plugins/`、解析每个子目录的清单，产出**一份包含失败项**的列表。
//!
//! # 为什么失败项也要在列表里
//!
//! 「插件没出现」是最难排查的状态：用户不知道自己少了个文件、还是清单写错了、
//! 还是应用没扫到目录。所以这里的契约是：
//!
//! * 目录里**每一个子目录**都会出现在结果里；
//! * 能解析的给出 [`PluginRecord::manifest`]，不能解析的给出 [`PluginRecord::problem`]
//!   （带具体原因，例如「找不到入口文件 index.js」）；
//! * 顺便跳过明显不是插件的目录（隐藏目录、`node_modules`、`target` 这类构建产物），
//!   但**记一条 `Skipped`**，这样用户也能看到「我放的东西被忽略了，原因是这个」。
//!
//! # 与脚本目录的同一个原则
//!
//! 插件目录位于**系统配置目录**（与 `scripts/`、`config.json` 同级）：
//! 它是用户数据，不是应用产物 —— 升级/重装不该碰它，也不该被 git 跟踪。

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::manifest::PluginManifest;
use crate::permission::PermissionSet;

/// 目录名里不该被当成插件的（构建产物 / 版本控制 / 系统目录）。
const SKIPPED_DIR_NAMES: [&str; 5] = ["node_modules", "target", "dist", "__pycache__", ".git"];

/// 一个插件目录的扫描结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginRecord {
    /// 目录名（等于插件 id；清单解析失败时也用它显示，因为那时还读不到 id）。
    pub id: String,
    /// 插件目录的绝对路径。
    pub dir: String,
    /// 解析成功的清单；`None` 表示这个目录有问题（见 [`PluginRecord::problem`]）。
    pub manifest: Option<PluginManifest>,
    /// 问题说明（`manifest` 为 `None` 时一定有值）。
    pub problem: Option<String>,
    /// 为什么被跳过（非插件目录）；此时 `manifest` 与 `problem` 都可能是 `None`。
    pub skipped: Option<String>,
}

impl PluginRecord {
    /// 是否是一个**可用**的插件（清单正常、入口存在）。
    pub fn is_usable(&self) -> bool {
        self.manifest.is_some()
    }

    /// 显示名：清单里有就用清单的，否则回落到目录名（失败项也能显示得像样）。
    pub fn display_name(&self) -> &str {
        self.manifest
            .as_ref()
            .map(PluginManifest::display_name)
            .unwrap_or(&self.id)
    }

    /// 版本号（失败项显示 `-`）。
    pub fn version(&self) -> &str {
        self.manifest
            .as_ref()
            .map(PluginManifest::version_or_default)
            .unwrap_or("-")
    }

    /// 声明的权限（失败项为空）。
    pub fn permissions(&self) -> PermissionSet {
        self.manifest
            .as_ref()
            .map(|manifest| manifest.permissions)
            .unwrap_or_default()
    }
}

/// 插件根目录：`<配置目录>/ClipBeam/plugins`。
///
/// 取不到系统配置目录时回退到临时目录（保持功能可用，同时让调用方能在日志里发现异常）——
/// 与 `clipbeam-scripting::scripts::scripts_dir` 的处理一致。
pub fn plugins_dir() -> PathBuf {
    match dirs::config_dir() {
        Some(dir) => dir.join("ClipBeam").join("plugins"),
        None => std::env::temp_dir().join("ClipBeam").join("plugins"),
    }
}

/// `plugins_dir` 的可读形式（界面提示「插件放哪」用）。
pub fn plugins_dir_display() -> String {
    plugins_dir().to_string_lossy().into_owned()
}

/// 扫描插件目录。
///
/// 目录不存在时返回空列表（首次启动、用户还没放过插件，是正常状态，不是错误）。
pub fn scan() -> io::Result<Vec<PluginRecord>> {
    scan_dir(&plugins_dir())
}

/// [`scan`] 的可指定目录版本（测试用）。
pub fn scan_dir(dir: &Path) -> io::Result<Vec<PluginRecord>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // 目录不存在 = 还没放过插件，返回空列表
        return Ok(Vec::new());
    };

    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        // 隐藏目录与构建产物：跳过但记一笔，用户能看到「被忽略了」
        if name.starts_with('.') || SKIPPED_DIR_NAMES.contains(&name) {
            records.push(PluginRecord {
                id: name.to_string(),
                dir: path.to_string_lossy().into_owned(),
                manifest: None,
                problem: None,
                skipped: Some(format!("目录名 {name:?} 不是插件目录，已跳过")),
            });
            continue;
        }

        records.push(scan_one(&path, name));
    }

    // 按 id 排序：界面顺序稳定，且与磁盘无关（文件系统不保证 read_dir 的顺序）
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

/// 扫描单个插件目录。
fn scan_one(dir: &Path, name: &str) -> PluginRecord {
    match PluginManifest::load(dir) {
        Ok(manifest) => PluginRecord {
            id: manifest.id.clone(),
            dir: dir.to_string_lossy().into_owned(),
            manifest: Some(manifest),
            problem: None,
            skipped: None,
        },
        Err(err) => PluginRecord {
            id: name.to_string(),
            dir: dir.to_string_lossy().into_owned(),
            manifest: None,
            problem: Some(err.message()),
            skipped: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独占的临时目录（每个测试一个，避免并行互相踩）。
    fn temp_dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-catalog-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    /// 在 `root` 下建一个可用插件目录。
    fn write_plugin(root: &Path, id: &str, manifest_extra: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(r#"{{ "id": {id:?} {manifest_extra} }}"#),
        )
        .unwrap();
        std::fs::write(dir.join("index.js"), "console.log('hi')").unwrap();
    }

    #[test]
    fn missing_directory_is_an_empty_list_not_an_error() {
        let root = temp_dir("missing");
        let records = scan_dir(&root.join("not-created")).expect("缺目录不该报错");
        assert!(records.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_usable_plugins_sorted_by_id() {
        let root = temp_dir("usable");
        write_plugin(&root, "zeta", "");
        write_plugin(&root, "alpha", r#", "name": "阿尔法""#);

        let records = scan_dir(&root).expect("扫描失败");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "alpha", "应当按 id 排序");
        assert_eq!(records[0].display_name(), "阿尔法");
        assert_eq!(records[1].id, "zeta");
        assert_eq!(records[1].display_name(), "zeta", "没有 name 时回落到 id");
        assert!(records.iter().all(PluginRecord::is_usable));
        assert_eq!(records[0].version(), "0.0.0");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 坏插件必须**出现在列表里**并带上原因（否则用户只会看到「插件不见了」）。
    #[test]
    fn broken_plugins_are_listed_with_a_reason() {
        let root = temp_dir("broken");

        // 1) 清单语法错误
        let bad_json = root.join("bad-json");
        std::fs::create_dir_all(&bad_json).unwrap();
        std::fs::write(bad_json.join("plugin.json"), "{ oops }").unwrap();

        // 2) 目录名与 id 不一致
        let mismatch = root.join("mismatch");
        std::fs::create_dir_all(&mismatch).unwrap();
        std::fs::write(mismatch.join("plugin.json"), r#"{ "id": "other" }"#).unwrap();
        std::fs::write(mismatch.join("index.js"), "").unwrap();

        // 3) 缺清单
        let no_manifest = root.join("no-manifest");
        std::fs::create_dir_all(&no_manifest).unwrap();

        let records = scan_dir(&root).expect("扫描失败");
        assert_eq!(records.len(), 3, "三个坏目录都应当出现");

        let by_id = |id: &str| records.iter().find(|r| r.id == id).expect("找不到记录");

        assert!(!by_id("bad-json").is_usable());
        assert!(by_id("bad-json")
            .problem
            .as_deref()
            .unwrap()
            .contains("合法的 JSON"));

        assert!(!by_id("mismatch").is_usable());
        assert!(by_id("mismatch")
            .problem
            .as_deref()
            .unwrap()
            .contains("不一致"));

        assert!(!by_id("no-manifest").is_usable());
        assert!(by_id("no-manifest")
            .problem
            .as_deref()
            .unwrap()
            .contains("plugin.json"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_non_plugin_directories_but_reports_them() {
        let root = temp_dir("skips");
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        write_plugin(&root, "real", "");

        let records = scan_dir(&root).expect("扫描失败");
        assert_eq!(records.len(), 3, "跳过也要有记录");
        assert_eq!(records.iter().filter(|r| r.is_usable()).count(), 1);

        let skipped: Vec<&str> = records
            .iter()
            .filter(|r| r.skipped.is_some())
            .map(|r| r.id.as_str())
            .collect();
        assert!(skipped.contains(&"node_modules"));
        assert!(skipped.contains(&".hidden"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_at_top_level_are_ignored() {
        let root = temp_dir("files");
        std::fs::write(root.join("readme.txt"), "x").unwrap();
        write_plugin(&root, "real", "");

        let records = scan_dir(&root).expect("扫描失败");
        assert_eq!(records.len(), 1, "普通文件不该产生记录");
        assert_eq!(records[0].id, "real");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn record_exposes_declared_permissions() {
        let root = temp_dir("perms");
        write_plugin(
            &root,
            "demo",
            r#", "permissions": { "feedback": true, "tray": true }"#,
        );

        let records = scan_dir(&root).expect("扫描失败");
        let permissions = records[0].permissions();
        assert!(permissions.feedback && permissions.tray);
        assert!(!permissions.window);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugins_dir_sits_next_to_scripts() {
        let dir = plugins_dir();
        assert!(
            dir.ends_with("ClipBeam/plugins") || dir.ends_with("ClipBeam\\plugins"),
            "插件目录应当是 ClipBeam/plugins：{}",
            dir.display()
        );
    }
}
