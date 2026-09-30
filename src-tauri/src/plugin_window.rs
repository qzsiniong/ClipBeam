//! 插件文件访问与窗口标识：**纯函数 + 校验**（这是插件自定义界面的地基）。
//!
//! # 为什么先有校验，后有窗口
//!
//! 插件可以带自己的 HTML / 图标 / 数据文件。宿主需要把「插件目录内的相对路径」变成绝对路径，
//! 而这是**安全边界**所在：`../../etc/passwd` 这种路径必须被拒绝。
//! 因此这条规则单独成文件、单独测试 —— 窗口功能落地时直接复用它。
//!
//! 本轮（插件框架第一版）落地的能力里已经用到它：
//! `$plugin.tray.setIcon("icon.png")` 要通过 [`resolve_plugin_file`] 解析图标。
//!
//! # 自定义窗口（下一轮）
//!
//! 已定方案：动态建 Tauri 窗口 + 自定义协议 `clipbeam-plugin://` + `<iframe sandbox>`。
//! 那时用到的就是这里的 [`plugin_window_label`]（窗口标签必须可预测，才能按模式配权限）
//! 与 [`resolve_plugin_file`]（协议处理器的路径解析）。

// 本模块的窗口相关常量与函数给**下一轮**的「插件自定义窗口」用（方案已定：动态窗口 +
// 自定义协议 + iframe 沙箱）。本轮已落地的是路径解析 `resolve_plugin_file`
// 与 `content_type_of`（托盘图标与协议处理器共用），窗口那部分先连同测试一起留下。
#![allow(dead_code, reason = "窗口功能在下一轮落地，规则与测试先就位")]

use std::path::{Component, Path, PathBuf};

/// 插件窗口的标签前缀（形如 `plugin-window-<插件 id>-<序号>`）。
///
/// 前缀固定 + 只含 ASCII 字母数字与连字符：Tauri 的 capability 用 `windows` 通配
/// 匹配标签（例如 `plugin-window-*`），标签一旦不可预测，就配不出权限来。
pub const PLUGIN_WINDOW_PREFIX: &str = "plugin-window-";

/// 自定义协议名（下轮窗口功能用；图标等本地文件也走同一套解析规则）。
pub const PLUGIN_PROTOCOL: &str = "clipbeam-plugin";

/// 生成一个插件窗口标签。
pub fn plugin_window_label(plugin_id: &str, seq: u32) -> String {
    format!("{PLUGIN_WINDOW_PREFIX}{plugin_id}-{seq}")
}

/// 判断一个窗口标签是不是插件窗口。
pub fn is_plugin_window(label: &str) -> bool {
    label.starts_with(PLUGIN_WINDOW_PREFIX)
}

/// 把「插件目录内的相对路径」解析成绝对路径。
///
/// 拒绝的情况（每一条对应一种真实事故）：
///
/// * **绝对路径**：插件不该用 `setIcon("/etc/passwd")` 这种方式指到目录外；
/// * **`..` 跳出**：`../../x` 这类穿越；
/// * **空路径 / 目录本身**：`""`、`"."` 都不是文件；
/// * **符号链接跳出**：解析后不在插件目录内（`canonicalize` 之后再比一次）。
///
/// 注意最后一条需要文件**存在**才能 canonicalize；文件不存在时按「不是文件」拒绝 ——
/// 图标不存在本来也该报错，而不是让托盘去读一个不存在的路径。
pub fn resolve_plugin_file(plugin_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.trim().is_empty() {
        return Err("路径不能为空".to_string());
    }

    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(format!(
            "只接受插件目录内的相对路径，收到绝对路径：{relative:?}"
        ));
    }
    // 先做纯词法检查：即使文件不存在，也要把明显的穿越挡在拼路径之前
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "路径不能包含 ..（不允许跳出插件目录）：{relative:?}"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("只接受插件目录内的相对路径：{relative:?}"));
            }
        }
    }

    let joined = plugin_dir.join(candidate);
    if !joined.is_file() {
        return Err(format!("找不到文件 {}（在插件目录内）", joined.display()));
    }

    // 再做一次真实路径检查：符号链接可以把 `link.png` 指到目录外
    let real_dir = plugin_dir
        .canonicalize()
        .map_err(|err| format!("插件目录不可用（{}）：{err}", plugin_dir.display()))?;
    let real_file = joined
        .canonicalize()
        .map_err(|err| format!("路径不可用（{}）：{err}", joined.display()))?;
    if !real_file.starts_with(&real_dir) {
        return Err(format!(
            "文件 {} 不在插件目录内（可能是符号链接）",
            joined.display()
        ));
    }

    Ok(real_file)
}

/// 为自定义协议解析请求路径：`/p-<插件 id>/<文件>` → `(插件 id, 相对路径)`。
///
/// 协议形如 `clipbeam-plugin://localhost/p-hello-plugin/index.html`。
/// 解析失败返回 `None`，由调用方决定回 404 还是空响应。
pub fn parse_plugin_protocol_path(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_start_matches('/');
    let (plugin, rest) = trimmed.split_once('/')?;
    let plugin_id = plugin.strip_prefix("p-")?;
    if plugin_id.is_empty() || rest.is_empty() {
        return None;
    }
    // 插件 id 不允许含路径分隔符（这里已经 split 过，仍再挡一次 `.` 开头）
    if plugin_id.starts_with('.') {
        return None;
    }
    Some((plugin_id.to_string(), rest.to_string()))
}

/// 按扩展名给 Content-Type（自定义协议要用）。
pub fn content_type_of(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-plugin-files-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    #[test]
    fn resolves_a_file_inside_the_plugin_directory() {
        let dir = temp_dir("ok");
        std::fs::write(dir.join("icon.png"), b"x").unwrap();

        let path = resolve_plugin_file(&dir, "icon.png").expect("应当解析成功");
        assert!(path.is_absolute(), "返回的应当是绝对路径");
        assert!(path.ends_with("icon.png"));

        // 允许子目录
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/logo.svg"), b"x").unwrap();
        assert!(resolve_plugin_file(&dir, "assets/logo.svg").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_escaping_paths() {
        let dir = temp_dir("escape");
        std::fs::write(dir.join("icon.png"), b"x").unwrap();

        for bad in [
            "../icon.png",
            "../../etc/passwd",
            "/etc/passwd",
            "",
            "  ",
            ".",
        ] {
            let err = resolve_plugin_file(&dir, bad).unwrap_or_else(|_| PathBuf::new());
            assert!(
                err.as_os_str().is_empty(),
                "{bad:?} 应当被拒绝，却解析成了 {}",
                err.display()
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_missing_files() {
        let dir = temp_dir("missing");
        let err = resolve_plugin_file(&dir, "nope.png").expect_err("不存在的文件应当报错");
        assert!(err.contains("找不到文件"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 符号链接指向目录外时必须拒绝（词法检查挡不住这一条）。
    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_pointing_outside() {
        let dir = temp_dir("symlink");
        let outside = dir.parent().unwrap().join(format!(
            "clipbeam-outside-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();

        let err = resolve_plugin_file(&dir, "link.txt").expect_err("跳出目录的软链应当被拒绝");
        assert!(err.contains("不在插件目录内"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn window_labels_are_predictable_and_matchable() {
        let label = plugin_window_label("hello-plugin", 1);
        assert_eq!(label, "plugin-window-hello-plugin-1");
        assert!(is_plugin_window(&label));

        // 界面/权限靠这个前缀区分插件窗口与主窗口
        assert!(!is_plugin_window("main"));
        assert!(!is_plugin_window("scripting"));
        assert!(!is_plugin_window("progress"));
        assert!(!is_plugin_window("standby"));
    }

    #[test]
    fn parses_protocol_paths() {
        assert_eq!(
            parse_plugin_protocol_path("/p-hello/index.html"),
            Some(("hello".to_string(), "index.html".to_string()))
        );
        assert_eq!(
            parse_plugin_protocol_path("/p-hello/assets/app.js"),
            Some(("hello".to_string(), "assets/app.js".to_string()))
        );

        // 缺前缀 / 缺文件 / 缺插件名都不是合法请求
        assert_eq!(parse_plugin_protocol_path("/hello/index.html"), None);
        assert_eq!(parse_plugin_protocol_path("/p-hello/"), None);
        assert_eq!(parse_plugin_protocol_path("/p-/index.html"), None);
        assert_eq!(parse_plugin_protocol_path("/p-.hidden/index.html"), None);
        assert_eq!(parse_plugin_protocol_path("index.html"), None);
    }

    #[test]
    fn content_types_cover_what_plugins_use() {
        assert!(content_type_of("index.html").starts_with("text/html"));
        assert!(
            content_type_of("a.JS").starts_with("text/javascript"),
            "扩展名大小写不敏感"
        );
        assert_eq!(content_type_of("a.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_of("a.svg"), "image/svg+xml");
        assert_eq!(content_type_of("a.png"), "image/png");
        assert_eq!(content_type_of("a.unknown"), "application/octet-stream");
    }
}
