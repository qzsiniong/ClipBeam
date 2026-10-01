//! `$.basename` / `$.dirname` / `$.extname` / `$.stem`：路径的**纯字符串**操作。
//!
//! # 为什么放进独立的 `path` 扩展，而不是 `file`
//!
//! 这四个能力**不读盘、不需要授权、不做 `~` 展开** —— 它们只是字符串处理，因此是
//! **同步纯计算**。而 `file` 扩展是异步的、并且带「本目录本次运行是否放行」的授权状态。
//! 两者混在一起会让「为什么改个路径字符串还要弹授权框」变得难以回答。
//! 分开还有一个直接好处：纯计算不依赖宿主，`clipbeam script` 的 CLI 宿主天然可用。
//!
//! # 为什么两种分隔符都认
//!
//! 脚本里拿到的路径可能来自 `$.pick_path`（当前平台的分隔符），也可能是脚本作者
//! 硬写的字面量（另一个平台的写法）。用 `std::path::Path` 会按**宿主平台**解释分隔符：
//! 同一个脚本在 macOS 与 Windows 上会得出不同结果。这里改成**同时按 `/` 与 `\` 切**，
//! 于是任何宿主上结果一致 —— 脚本行为可预期比「贴合平台」更重要。
//!
//! # 语义
//!
//! 照 Node 的 `path.basename/dirname/extname`，只有一处刻意的偏离：
//! `dirname` **不留尾部斜杠**，且 `dirname("/a")` 返回 `""` 而不是 `"/"`。
//! 统一规则是「去掉最后一段」—— 为「分隔符恰好在首位」单开特例正是容易长错的地方。
//! 结果永不留下尾部斜杠，拼路径时也不会出现 `//`。
//!
//! 边界语义由 `CASES` 这张表钉死；能力层测试（`tests/capabilities.rs`）里有一份
//! 等价的 JS 表，两边一起跑，防止「Rust 单测过了但 JS 绑定漏了参数」这类漂移。

use rquickjs::{Ctx, Result as QjsResult};

use crate::extensions::extension;

extension! {
    /// `$.basename` / `$.dirname` / `$.extname` / `$.stem`。
    pub struct PathExtension;
    "basename" => js_basename,
        "basename(path: string) -> string",
        "路径的最后一段（同时按 / 与 \\\\ 切；忽略尾部斜杠）";
    "dirname" => js_dirname,
        "dirname(path: string) -> string",
        "去掉最后一段后的目录部分（不留尾部斜杠）；没有剩余时返回空串";
    "extname" => js_extname,
        "extname(path: string) -> string",
        "扩展名（含点）；点开头的文件名没有扩展名，无扩展名返回空串";
    "stem" => js_stem,
        "stem(path: string) -> string",
        "文件名去掉扩展名（`a.tar.gz` → `a.tar`）";
}

/// 路径分隔符：两种都认（理由见模块头）。
fn is_sep(ch: char) -> bool {
    ch == '/' || ch == '\\'
}

/// `basename`：剥掉尾部斜杠后取最后一段。
fn basename_of(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_sep);
    match trimmed.rfind(is_sep) {
        Some(at) => &trimmed[at + 1..],
        None => trimmed,
    }
}

/// `dirname`：去掉最后一段以及它前面的分隔符，然后**再剥掉结果尾部的分隔符**。
///
/// 于是 `"/a/b/"` → `"/a"`、`"//a//b//"` → `"//a"`、`"/a"` → `""`、`"a"` → `""`。
fn dirname_of(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_sep);
    match trimmed.rfind(is_sep) {
        Some(at) => trimmed[..at].trim_end_matches(is_sep),
        None => "",
    }
}

/// 扩展名（含点）。`.` 在首位 = 隐藏文件，视为没有扩展名。
fn extension_of(base: &str) -> &str {
    match base.rfind('.') {
        Some(at) if at > 0 => &base[at..],
        _ => "",
    }
}

/// `stem`：basename 去掉扩展名。
fn stem_of(path: &str) -> &str {
    let base = basename_of(path);
    &base[..base.len() - extension_of(base).len()]
}

/// `basename(path) -> string`
fn js_basename<'js>(_ctx: Ctx<'js>, path: String) -> QjsResult<String> {
    Ok(basename_of(&path).to_string())
}

/// `dirname(path) -> string`
fn js_dirname<'js>(_ctx: Ctx<'js>, path: String) -> QjsResult<String> {
    Ok(dirname_of(&path).to_string())
}

/// `extname(path) -> string`
fn js_extname<'js>(_ctx: Ctx<'js>, path: String) -> QjsResult<String> {
    Ok(extension_of(basename_of(&path)).to_string())
}

/// `stem(path) -> string`
fn js_stem<'js>(_ctx: Ctx<'js>, path: String) -> QjsResult<String> {
    Ok(stem_of(&path).to_string())
}

/// 四个能力的**行为契约**：`(输入, basename, dirname, extname, stem)`。
///
/// 改实现必须先改这张表。它只在测试里用，因此标 `cfg(test)`。
#[cfg(test)]
const CASES: &[(&str, &str, &str, &str, &str)] = &[
    ("/a/b/c.txt", "c.txt", "/a/b", ".txt", "c"),
    ("/a/b/", "b", "/a", "", "b"),
    ("/a", "a", "", "", "a"),
    ("c.txt", "c.txt", "", ".txt", "c"),
    ("/", "", "", "", ""),
    ("", "", "", "", ""),
    ("//a//b//", "b", "//a", "", "b"),
    ("C:\\x\\y.ZIP", "y.ZIP", "C:\\x", ".ZIP", "y"),
    ("a/b/.bashrc", ".bashrc", "a/b", "", ".bashrc"),
    ("a/b/c.tar.gz", "c.tar.gz", "a/b", ".gz", "c.tar"),
    ("a/b/c.", "c.", "a/b", ".", "c"),
    // 两种分隔符混用：按同一规则切
    ("a\\b/c.txt", "c.txt", "a\\b", ".txt", "c"),
    ("a/b\\c.txt", "c.txt", "a/b", ".txt", "c"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_table() {
        for &(input, base, dir, ext, stem) in CASES {
            assert_eq!(basename_of(input), base, "basename({input:?})");
            assert_eq!(dirname_of(input), dir, "dirname({input:?})");
            assert_eq!(extension_of(basename_of(input)), ext, "extname({input:?})");
            assert_eq!(stem_of(input), stem, "stem({input:?})");
        }
    }

    /// `dirname` 的结果永不留下尾部斜杠（拼路径时不会出现 `//`）。
    #[test]
    fn dirname_never_ends_with_separator() {
        for path in [
            "/a/b/", "//a//b//", "a//b", "/a", "a", "/", "", "a/b/", "a\\b\\\\",
        ] {
            let dir = dirname_of(path);
            assert!(
                !dir.ends_with('/') && !dir.ends_with('\\'),
                "dirname({path:?}) = {dir:?} 不该以分隔符结尾"
            );
        }
    }
}
