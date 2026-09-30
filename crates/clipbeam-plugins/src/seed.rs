//! 内置示例插件：首次启动时释放到插件目录。
//!
//! 与脚本目录的 seed 完全同一个思路（见 `clipbeam-scripting::scripts::ensure_seed_scripts`）：
//!
//! * 插件是**用户数据**，放在系统配置目录里，升级/重装应用不该碰它；
//! * 内置示例用 `include_str!` 内嵌进二进制，部署时不需要附带额外文件；
//! * **已存在的文件永不覆盖** —— 用户改过的示例不会被下次启动还原。
//!
//! 示例默认**不启用**：发现 ≠ 启用（用户要在插件页面手动打开）。

use std::path::{Path, PathBuf};

use crate::manifest::MANIFEST_FILE;

/// 内置示例插件：(插件 id, [(相对路径, 内容)])。
///
/// 用 `include_str!` 而不是运行时读文件：示例必须跟着二进制走，
/// 否则「装完就少一个样例」是最常见的分发事故。
pub const SEED_PLUGINS: [(&str, [(&str, &str); 3]); 1] = [(
    "hello-plugin",
    [
        (
            MANIFEST_FILE,
            include_str!("../seed/hello-plugin/plugin.json"),
        ),
        ("index.ts", include_str!("../seed/hello-plugin/index.ts")),
        // 演示窗口的页面：由 `clipbeam-plugin://` 协议原样送给沙箱 iframe
        (
            "relay.html",
            include_str!("../seed/hello-plugin/relay.html"),
        ),
    ],
)];

/// 确保插件目录存在并写入内置示例；返回本次**新写入**的文件数。
pub fn ensure_seed_plugins() -> std::io::Result<usize> {
    ensure_seed_plugins_in(&crate::catalog::plugins_dir())
}

/// [`ensure_seed_plugins`] 的可指定目录版本（测试用）。
pub fn ensure_seed_plugins_in(root: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(root)?;

    let mut written = 0;
    for (id, files) in SEED_PLUGINS {
        let dir: PathBuf = root.join(id);
        std::fs::create_dir_all(&dir)?;
        for (name, content) in files {
            let path = dir.join(name);
            if path.exists() {
                continue;
            }
            std::fs::write(&path, content)?;
            written += 1;
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-seed-plugins-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 首次写入全部文件；第二次什么都不写；用户改动不被覆盖。
    #[test]
    fn seeds_only_missing_files() {
        let root = temp_dir("seed");

        let expected_files: usize = SEED_PLUGINS.iter().map(|(_, files)| files.len()).sum();

        let first = ensure_seed_plugins_in(&root).expect("首次写 seed 失败");
        assert_eq!(first, expected_files, "首次应当写入全部示例文件");

        let second = ensure_seed_plugins_in(&root).expect("二次写 seed 失败");
        assert_eq!(second, 0, "二次不应再写");

        // 改掉**真入口**（示例是 TypeScript，入口名为 index.ts），再 ensure 不应被还原
        let entry = root.join("hello-plugin").join("index.ts");
        std::fs::write(&entry, "// 用户改过的内容\n").unwrap();
        let third = ensure_seed_plugins_in(&root).expect("三次写 seed 失败");
        assert_eq!(third, 0, "用户改动之后不该再写");
        assert_eq!(
            std::fs::read_to_string(&entry).unwrap(),
            "// 用户改过的内容\n"
        );

        // 释放结果里不该混进入口的旧名字：示例只该有「清单 + 入口」两个文件。
        // 这条断言挡的是「换过入口扩展名后，旧文件仍被写进去」这类回归 ——
        // 那会让用户看到两个入口、分不清哪个在跑。
        assert!(
            !root.join("hello-plugin").join("index.js").exists(),
            "示例只应有清单与入口两个文件，不该出现 index.js"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 释放出来的示例必须是**能通过清单校验的真插件**（否则示例本身就报错，很尴尬）。
    #[test]
    fn seeded_plugin_passes_manifest_validation() {
        let root = temp_dir("valid");
        ensure_seed_plugins_in(&root).expect("写 seed 失败");

        let dir = root.join("hello-plugin");
        let manifest = crate::PluginManifest::load(&dir).expect("示例插件的清单应当合法");

        assert_eq!(manifest.id, "hello-plugin");
        assert_eq!(manifest.entry_name(), "index.ts");
        assert!(!manifest.menus.is_empty(), "示例应当演示托盘菜单");
        assert!(
            manifest.permissions.tray,
            "声明了 menus 就必须开 tray 权限（否则清单校验会失败）"
        );
        assert!(manifest.permissions.feedback && manifest.permissions.notification);
        assert!(
            manifest.permissions.system_dialog,
            "示例演示了 confirm/alert"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 释放出来的示例必须能被**目录扫描**认出来（catalog 与 seed 的口径一致）。
    #[test]
    fn seeded_plugin_is_discovered_by_the_catalog() {
        let root = temp_dir("catalog");
        ensure_seed_plugins_in(&root).expect("写 seed 失败");

        let records = crate::catalog::scan_dir(&root).expect("扫描失败");
        assert_eq!(records.len(), 1);
        assert!(records[0].is_usable(), "示例插件应当是可用的：{records:?}");
        assert_eq!(records[0].id, "hello-plugin");
        assert_eq!(records[0].display_name(), "示例插件");

        let _ = std::fs::remove_dir_all(&root);
    }
}
