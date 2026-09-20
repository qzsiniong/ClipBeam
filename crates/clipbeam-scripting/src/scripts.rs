//! 脚本目录：`<系统配置目录>/ClipBeam/scripts/`。
//!
//! # 为什么放在系统配置目录
//!
//! 脚本是**用户数据**，不是应用产物：升级/重装应用不该碰它，也不该被 `git` 跟踪。
//! 因此目录位置与 `Config` 的 `config.json` 同级（见 `src-tauri/src/config.rs`）。
//!
//! # 内置示例
//!
//! 首次启动时写入 [`SEED_SCRIPTS`]，**已存在的文件永不覆盖** —— 用户改过的示例
//! 不会被下次启动还原。示例内容用 `include_str!` 内嵌，部署时不需要附带额外文件。

use std::io;
use std::path::{Path, PathBuf};

/// 允许的脚本扩展名（与 `script_engine::ts` 的转译范围保持一致）。
pub const SCRIPT_EXTENSIONS: [&str; 6] = ["js", "mjs", "cjs", "ts", "mts", "cts"];

/// 脚本名长度上限（字符）。
const MAX_NAME_CHARS: usize = 128;

/// 内置示例：(文件名, 内容)。
pub const SEED_SCRIPTS: [(&str, &str); 2] = [
    (
        "01-quick-start.js",
        include_str!("../seed/01-quick-start.js"),
    ),
    ("02-ts-demo.ts", include_str!("../seed/02-ts-demo.ts")),
];

/// 脚本目录：`<配置目录>/ClipBeam/scripts`。
///
/// 取不到系统配置目录时回退到临时目录（保持功能可用，同时让调用方能在日志里发现异常）。
pub fn scripts_dir() -> PathBuf {
    match dirs::config_dir() {
        Some(dir) => dir.join("ClipBeam").join("scripts"),
        None => std::env::temp_dir().join("ClipBeam").join("scripts"),
    }
}

/// 判断名字是否是合法的脚本文件名。
///
/// 规则：非空、不超过 [`MAX_NAME_CHARS`] 个字符、不含路径分隔符与 `..`、
/// 扩展名在 [`SCRIPT_EXTENSIONS`] 里。**不做**任何「清洗后改名」的容错 ——
/// 非法名一律拒绝，避免写出用户没预期的文件。
pub fn is_valid_script_name(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    if name.starts_with('.') {
        return false;
    }
    match extension_of(name) {
        Some(ext) => SCRIPT_EXTENSIONS.contains(&ext.as_str()),
        None => false,
    }
}

/// 取脚本名的扩展名（小写，不含点）。
pub fn extension_of(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

/// 语言标签：`ts` 家族返回 `"ts"`，其余返回 `"js"`（供前端编辑器选高亮模式）。
pub fn language_of(name: &str) -> &'static str {
    match extension_of(name).as_deref() {
        Some("ts" | "mts" | "cts") => "ts",
        _ => "js",
    }
}

/// 校验脚本名，非法时返回中文错误说明。
pub fn check_script_name(name: &str) -> Result<(), String> {
    if is_valid_script_name(name) {
        return Ok(());
    }
    Err(format!(
        "非法脚本文件名 {name:?}：需要非空、不超过 {MAX_NAME_CHARS} 字符、不含路径分隔符或 ..、\
         且扩展名为 {} 之一",
        SCRIPT_EXTENSIONS.join(" / ")
    ))
}

/// 脚本目录里一个脚本的元信息（返回给前端的形态）。
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ScriptMeta {
    /// 文件名（同时是唯一标识）。
    pub name: String,
    /// 绝对路径，便于脚本里用 `$.read` 或用户定位。
    pub path: String,
    /// `"js"` 或 `"ts"`。
    pub language: String,
}

/// 列出脚本目录里的脚本（按文件名排序；目录不存在时返回空列表）。
pub fn list_scripts() -> io::Result<Vec<ScriptMeta>> {
    let dir = scripts_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut scripts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_valid_script_name(name) {
            continue;
        }
        scripts.push(ScriptMeta {
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            language: language_of(name).to_string(),
        });
    }
    scripts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scripts)
}

/// 读取一个脚本的源码。
pub fn read_script(name: &str) -> Result<String, String> {
    check_script_name(name)?;
    let path = scripts_dir().join(name);
    std::fs::read_to_string(&path).map_err(|err| format!("读取脚本 {name:?} 失败：{err}"))
}

/// 写入一个脚本（覆盖同名文件，自动创建目录）。
pub fn write_script(name: &str, source: &str) -> Result<(), String> {
    check_script_name(name)?;
    let dir = scripts_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建脚本目录 {dir:?} 失败：{err}"))?;
    let path = dir.join(name);
    std::fs::write(&path, source).map_err(|err| format!("保存脚本 {name:?} 失败：{err}"))
}

/// 删除一个脚本。
pub fn delete_script(name: &str) -> Result<(), String> {
    check_script_name(name)?;
    let path = scripts_dir().join(name);
    std::fs::remove_file(&path).map_err(|err| format!("删除脚本 {name:?} 失败：{err}"))
}

/// 确保脚本目录存在并写入内置示例；返回本次**新写入**的文件数。
///
/// 已存在的文件不会被覆盖（用户改过的示例保持原样）。
pub fn ensure_seed_scripts() -> io::Result<usize> {
    ensure_seed_scripts_in(&scripts_dir())
}

/// [`ensure_seed_scripts`] 的可指定目录版本（测试用）。
pub fn ensure_seed_scripts_in(dir: &Path) -> io::Result<usize> {
    std::fs::create_dir_all(dir)?;

    let mut written = 0;
    for (name, source) in SEED_SCRIPTS {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        std::fs::write(&path, source)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_script_names() {
        for ok in ["a.js", "01-quick-start.js", "demo.TS", "x.mts", "y.cjs"] {
            assert!(is_valid_script_name(ok), "{ok} 应当合法");
        }
        for bad in [
            "",
            "a.txt",
            "noext",
            "../evil.js",
            "sub/dir.js",
            "sub\\dir.js",
            ".hidden.js",
            "a..b.js",
        ] {
            assert!(!is_valid_script_name(bad), "{bad} 应当被拒绝");
        }
    }

    #[test]
    fn maps_language_by_extension() {
        assert_eq!(language_of("a.ts"), "ts");
        assert_eq!(language_of("a.mts"), "ts");
        assert_eq!(language_of("a.js"), "js");
    }

    #[test]
    fn seeds_only_missing_files() {
        let dir = std::env::temp_dir().join(format!("clipbeam-seed-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let first = ensure_seed_scripts_in(&dir).expect("首次写 seed 失败");
        assert_eq!(first, SEED_SCRIPTS.len(), "首次应当写入全部示例");

        // 改掉其中一个示例，再次 ensure 不应覆盖
        let edited = "// 用户改过的内容\n";
        std::fs::write(dir.join(SEED_SCRIPTS[0].0), edited).unwrap();
        let second = ensure_seed_scripts_in(&dir).expect("二次写 seed 失败");
        assert_eq!(second, 0, "二次不应再写");
        assert_eq!(
            std::fs::read_to_string(dir.join(SEED_SCRIPTS[0].0)).unwrap(),
            edited,
            "用户改动不应被覆盖"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
