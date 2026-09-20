//! `$.read` / `$.write` / …：真实文件系统的读写能力。
//!
//! # 两条硬规则
//!
//! 1. **只接受绝对路径**。`~`、`C:/x`、`C:\x`、git-bash 的 `/d/x` 与
//!    `/cygdrive/d/x` 都算绝对路径（见 [`resolve_path`]），相对路径直接报错 ——
//!    脚本的工作目录取决于宿主怎么启动，相对路径的含义是不可预期的。
//! 2. **修改要问一次**。只读操作（`read` / `read_text` / `exists` / `stat` / `list`）
//!    不问；写、追加、建目录、删除、改名、复制这些**修改**操作每次都会问宿主
//!    （[`ScriptHost::allow_file_change`](crate::ScriptHost::allow_file_change)）。
//!    用户在弹框里可以选「本次运行内该目录都允许」，那条授权记在
//!    [`FileState`] 里 —— 它随**本次运行**的上下文一起消失，不会跨运行保留。
//!
//! # 自定义的错误文案
//!
//! 所有失败都是被拒绝的 Promise，错误里带上动作与路径（例如
//! `写入 "/x/y" 失败：No such file or directory (os error 2)`），脚本侧
//! `try/catch` 能直接拿到可读信息。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use encoding_rs::Encoding;
use rquickjs::function::Opt;
use rquickjs::prelude::Async;
use rquickjs::{
    ArrayBuffer, Ctx, Exception, Function, JsLifetime, Object, Result as QjsResult, Value,
};

use script_engine::{CapabilitySpec, ScriptExtension};

use crate::extensions::{input_bytes, output_buffer, require_host, throw_host_error};
use crate::host::FileDecision;

/// 本次运行内已放行的目录（用户选过「本次运行内该目录都允许」的那些）。
///
/// 存在上下文 userdata 里而不是扩展实例上：上下文是每次运行一份，
/// 天然满足「授权不跨运行」；扩展实例在多次运行间可能被复用。
#[derive(Default)]
struct FileState {
    allowed_dirs: Mutex<Vec<PathBuf>>,
}

// SAFETY: `FileState` 只有 `Mutex<Vec<PathBuf>>`，不含任何带 `'js` 生命周期的 JS 值。
unsafe impl<'js> JsLifetime<'js> for FileState {
    type Changed<'to> = FileState;
}

impl FileState {
    /// 该目录（或它的某个父目录）是否已在本次运行中被放行。
    fn is_allowed(&self, scope_dir: &Path) -> bool {
        self.allowed_dirs
            .lock()
            .unwrap()
            .iter()
            .any(|allowed| scope_dir.starts_with(allowed))
    }

    fn allow(&self, scope_dir: &Path) {
        let mut allowed = self.allowed_dirs.lock().unwrap();
        if !allowed.iter().any(|dir| dir == scope_dir) {
            allowed.push(scope_dir.to_path_buf());
        }
    }
}

/// 取回上下文里的文件状态。
fn file_state<'js>(ctx: &Ctx<'js>) -> QjsResult<Arc<FileState>> {
    ctx.userdata::<Arc<FileState>>()
        .map(|guard| guard.clone())
        .ok_or_else(|| Exception::throw_message(ctx, "文件状态不存在（引擎未初始化）"))
}

/// `$.read` / `$.write` / …：文件系统能力。
///
/// 手写 `register`（不用 `extension!` 宏）的原因：它要在挂函数前把
/// [`FileState`] 存进上下文，宏只处理无状态扩展。
pub struct FileExtension;

impl ScriptExtension for FileExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ctx.store_userdata(Arc::new(FileState::default()))
            .map_err(|_| {
                Exception::throw_message(ctx, "文件状态存入上下文失败（userdata 正被借用）")
            })?;

        ns.set("read", Function::new(ctx.clone(), Async(js_read))?)?;
        ns.set(
            "read_text",
            Function::new(ctx.clone(), Async(js_read_text))?,
        )?;
        ns.set("exists", Function::new(ctx.clone(), Async(js_exists))?)?;
        ns.set("stat", Function::new(ctx.clone(), Async(js_stat))?)?;
        ns.set("list", Function::new(ctx.clone(), Async(js_list))?)?;
        ns.set("write", Function::new(ctx.clone(), Async(js_write))?)?;
        ns.set(
            "write_text",
            Function::new(ctx.clone(), Async(js_write_text))?,
        )?;
        ns.set("append", Function::new(ctx.clone(), Async(js_append))?)?;
        ns.set(
            "append_text",
            Function::new(ctx.clone(), Async(js_append_text))?,
        )?;
        ns.set("mkdir", Function::new(ctx.clone(), Async(js_mkdir))?)?;
        ns.set("remove", Function::new(ctx.clone(), Async(js_remove))?)?;
        ns.set("rename", Function::new(ctx.clone(), Async(js_rename))?)?;
        ns.set("copy", Function::new(ctx.clone(), Async(js_copy))?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![
            CapabilitySpec {
                name: "read",
                signature: "read(path: string) -> Promise<ArrayBuffer>",
                doc: "读取文件全部字节（只读）；路径必须是绝对路径",
            },
            CapabilitySpec {
                name: "read_text",
                signature: "read_text(path: string, encoding?: string) -> Promise<string>",
                doc: "读取文件并按 encoding 解码（默认 utf-8）",
            },
            CapabilitySpec {
                name: "exists",
                signature: "exists(path: string) -> Promise<boolean>",
                doc: "路径是否存在（文件、目录、符号链接都算）",
            },
            CapabilitySpec {
                name: "stat",
                signature: "stat(path: string) -> Promise<{ path: string, size: number, isFile: boolean, isDir: boolean, modifiedMs: number }>",
                doc: "读取文件元信息（大小 / 类型 / 修改时间）",
            },
            CapabilitySpec {
                name: "list",
                signature: "list(path: string) -> Promise<string[]>",
                doc: "列出目录下的条目名（已排序；不含 `.` / `..`）",
            },
            CapabilitySpec {
                name: "write",
                signature: "write(path: string, data: string | ArrayBuffer | ArrayBufferView) -> Promise<void>",
                doc: "覆盖写入文件（需要确认；父目录必须已存在）",
            },
            CapabilitySpec {
                name: "write_text",
                signature: "write_text(path: string, text: string) -> Promise<void>",
                doc: "以 UTF-8 覆盖写入文本（需要确认）",
            },
            CapabilitySpec {
                name: "append",
                signature: "append(path: string, data: string | ArrayBuffer | ArrayBufferView) -> Promise<void>",
                doc: "追加写入文件（需要确认；文件不存在时创建）",
            },
            CapabilitySpec {
                name: "append_text",
                signature: "append_text(path: string, text: string) -> Promise<void>",
                doc: "以 UTF-8 追加写入文本（需要确认）",
            },
            CapabilitySpec {
                name: "mkdir",
                signature: "mkdir(path: string) -> Promise<void>",
                doc: "递归创建目录（需要确认；已存在时不报错）",
            },
            CapabilitySpec {
                name: "remove",
                signature: "remove(path: string) -> Promise<void>",
                doc: "删除文件或目录（目录递归删除；需要确认）",
            },
            CapabilitySpec {
                name: "rename",
                signature: "rename(from: string, to: string) -> Promise<void>",
                doc: "移动 / 改名（需要确认）",
            },
            CapabilitySpec {
                name: "copy",
                signature: "copy(from: string, to: string) -> Promise<void>",
                doc: "复制文件或目录（目录递归；需要确认）",
            },
        ]
    }
}

// ── 路径解析 ────────────────────────────────────────────────────────────────

/// 把脚本里的路径解析成宿主的 `PathBuf`。
///
/// 支持（都视为绝对路径）：
///
/// | 写法 | 解析成 |
/// |---|---|
/// | `~/a.txt` | 主目录下的 `a.txt` |
/// | `C:/x` / `C:\x` | 原样（Windows 盘符路径） |
/// | `/d/x` | `D:/x`（git-bash / MSYS 风格） |
/// | `/cygdrive/d/x` | `D:/x`（Cygwin 风格） |
/// | `/x/y` | 原样（Unix 绝对路径） |
///
/// 其余（相对路径）返回错误：脚本的工作目录不可预期，允许相对路径等于埋雷。
fn resolve_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".to_string());
    }

    if trimmed == "~" {
        return home_dir();
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return Ok(home_dir()?.join(rest));
    }

    if let Some(rest) = trimmed.strip_prefix("/cygdrive/") {
        return Ok(drive_path(rest));
    }

    if let Some(rest) = trimmed.strip_prefix('/') {
        // git-bash 风格 `/d/xx`：单个字母 + 分隔符才当盘符
        let mut chars = rest.chars();
        if let Some(drive) = chars.next() {
            let separator = chars.next();
            if drive.is_ascii_alphabetic() && matches!(separator, Some('/') | Some('\\') | None) {
                let tail = if separator.is_some() {
                    chars.as_str()
                } else {
                    ""
                };
                return Ok(drive_path(&format!("{drive}/{tail}")));
            }
        }
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() || is_windows_absolute(trimmed) {
        return Ok(path);
    }

    Err(format!(
        "路径必须是绝对路径（支持 ~、C:/x、C:\\x、/d/x、/cygdrive/d/x）：{raw:?}"
    ))
}

/// 用户主目录（`~` 的展开目标）。
fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "无法确定用户主目录（~）".to_string())
}

/// `/cygdrive/d/xx` 或 `/d/xx` 里 `d/xx` 部分 → `D:/xx`。
fn drive_path(rest: &str) -> PathBuf {
    let mut chars = rest.chars();
    let drive = chars.next().unwrap_or('C').to_ascii_uppercase();
    let tail = chars.as_str().trim_start_matches(['/', '\\']);
    PathBuf::from(format!("{drive}:/{tail}"))
}

/// `C:/x` / `C:\x` 形态（`Path::is_absolute` 只在 Windows 上认它）。
fn is_windows_absolute(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

/// 解析路径，失败时抛出 JS 异常。
fn path_arg<'js>(ctx: &Ctx<'js>, raw: &str) -> QjsResult<PathBuf> {
    resolve_path(raw).map_err(|err| Exception::throw_message(ctx, &err))
}

/// 「本目录都允许」记的是这个目录：目标路径的父目录。
fn scope_of(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(path)
        .to_path_buf()
}

/// 修改类动作的入口：先看本次运行是否已放行该目录，再问宿主。
fn authorize<'js>(
    ctx: &Ctx<'js>,
    state: &FileState,
    action: &str,
    path: &Path,
    scope_dir: &Path,
) -> QjsResult<()> {
    if state.is_allowed(scope_dir) {
        return Ok(());
    }

    let host = require_host(ctx, "文件操作")?;
    match host.allow_file_change(action, path, scope_dir) {
        Ok(FileDecision::Allow) => Ok(()),
        Ok(FileDecision::AllowDir) => {
            state.allow(scope_dir);
            Ok(())
        }
        Ok(FileDecision::Deny) => Err(Exception::throw_message(
            ctx,
            &format!("用户拒绝了文件操作：{action} {}", path.display()),
        )),
        Err(err) => Err(throw_host_error(ctx, err)),
    }
}

/// 在阻塞线程池里跑一段文件 IO，并把 `io::Error` 翻成带动作与路径的 JS 异常。
async fn blocking<'js, T, F>(ctx: &Ctx<'js>, what: &str, path: &Path, job: F) -> QjsResult<T>
where
    F: FnOnce(&Path) -> std::io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    // 不用 tokio 的异步文件 IO：既不阻塞引擎线程，也不给引擎引入 tokio 的 fs feature
    let owned = path.to_path_buf();
    let joined = tokio::task::spawn_blocking(move || job(&owned))
        .await
        .map_err(|err| {
            Exception::throw_message(ctx, &format!("{what} {} 的任务异常：{err}", path.display()))
        })?;
    joined.map_err(|err| {
        Exception::throw_message(ctx, &format!("{what} {} 失败：{err}", path.display()))
    })
}

// ── 只读操作 ────────────────────────────────────────────────────────────────

/// `read(path) -> Promise<ArrayBuffer>`
async fn js_read<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<ArrayBuffer<'js>> {
    let target = path_arg(&ctx, &path)?;
    let bytes = blocking(&ctx, "读取", &target, |path| std::fs::read(path)).await?;
    output_buffer(&ctx, bytes)
}

/// `read_text(path, encoding = "utf-8") -> Promise<string>`
async fn js_read_text<'js>(
    ctx: Ctx<'js>,
    path: String,
    encoding: Opt<String>,
) -> QjsResult<String> {
    let target = path_arg(&ctx, &path)?;
    let label = encoding.0.unwrap_or_else(|| "utf-8".to_string());
    let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
        return Err(Exception::throw_message(
            &ctx,
            &format!("未知的文本编码标签：{label:?}"),
        ));
    };

    let bytes = blocking(&ctx, "读取", &target, |path| std::fs::read(path)).await?;

    Ok(encoding.decode_with_bom_removal(&bytes).0.into_owned())
}

/// `exists(path) -> Promise<boolean>`
async fn js_exists<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<bool> {
    let target = path_arg(&ctx, &path)?;
    // `symlink_metadata` 而不是 `metadata`：悬空符号链接也算「存在」
    blocking(&ctx, "检查", &target, |path| {
        Ok(path.symlink_metadata().is_ok())
    })
    .await
}

/// `stat(path) -> Promise<{...}>`
async fn js_stat<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<Object<'js>> {
    let target = path_arg(&ctx, &path)?;

    struct Stat {
        size: u64,
        is_file: bool,
        is_dir: bool,
        modified_ms: u64,
    }

    let stat = blocking(&ctx, "读取元信息", &target, |path| {
        let metadata = path.symlink_metadata()?;
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        Ok(Stat {
            size: metadata.len(),
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            modified_ms,
        })
    })
    .await?;

    let object = Object::new(ctx.clone())?;
    object.set("path", target.to_string_lossy().into_owned())?;
    object.set("size", stat.size)?;
    object.set("isFile", stat.is_file)?;
    object.set("isDir", stat.is_dir)?;
    object.set("modifiedMs", stat.modified_ms)?;
    Ok(object)
}

/// `list(path) -> Promise<string[]>`
async fn js_list<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<Vec<String>> {
    let target = path_arg(&ctx, &path)?;
    blocking(&ctx, "列出目录", &target, |path| {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    })
    .await
}

// ── 修改操作（每次都要确认，除非本次运行已放行该目录）─────────────────────

/// `write(path, data) -> Promise<void>`
async fn js_write<'js>(ctx: Ctx<'js>, path: String, data: Value<'js>) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let bytes = input_bytes(&ctx, data)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "写入", &target, &scope_of(&target))?;

    blocking(&ctx, "写入", &target, move |path| {
        std::fs::write(path, &bytes)
    })
    .await
}

/// `write_text(path, text) -> Promise<void>`
async fn js_write_text<'js>(ctx: Ctx<'js>, path: String, text: String) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "写入", &target, &scope_of(&target))?;

    blocking(&ctx, "写入", &target, move |path| {
        std::fs::write(path, text.as_bytes())
    })
    .await
}

/// `append(path, data) -> Promise<void>`
async fn js_append<'js>(ctx: Ctx<'js>, path: String, data: Value<'js>) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let bytes = input_bytes(&ctx, data)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "追加写入", &target, &scope_of(&target))?;

    blocking(&ctx, "追加写入", &target, move |path| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(&bytes)
    })
    .await
}

/// `append_text(path, text) -> Promise<void>`
async fn js_append_text<'js>(ctx: Ctx<'js>, path: String, text: String) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "追加写入", &target, &scope_of(&target))?;

    blocking(&ctx, "追加写入", &target, move |path| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(text.as_bytes())
    })
    .await
}

/// `mkdir(path) -> Promise<void>`
async fn js_mkdir<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "创建目录", &target, &scope_of(&target))?;

    blocking(&ctx, "创建目录", &target, |path| {
        std::fs::create_dir_all(path)
    })
    .await
}

/// `remove(path) -> Promise<void>`
async fn js_remove<'js>(ctx: Ctx<'js>, path: String) -> QjsResult<()> {
    let target = path_arg(&ctx, &path)?;
    let state = file_state(&ctx)?;
    authorize(&ctx, &state, "删除", &target, &scope_of(&target))?;

    blocking(&ctx, "删除", &target, |path| {
        // 用 symlink_metadata：删掉的是链接本身，不会顺着链接删到别处
        let metadata = path.symlink_metadata()?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    })
    .await
}

/// `rename(from, to) -> Promise<void>`
async fn js_rename<'js>(ctx: Ctx<'js>, from: String, to: String) -> QjsResult<()> {
    let source = path_arg(&ctx, &from)?;
    let target = path_arg(&ctx, &to)?;
    let state = file_state(&ctx)?;
    // 放行范围按**目标**目录算：改名/移动的结果落在哪儿，才是用户关心的范围
    authorize(
        &ctx,
        &state,
        &format!("重命名 → {}", target.display()),
        &source,
        &scope_of(&target),
    )?;

    blocking(&ctx, "重命名", &source, move |path| {
        std::fs::rename(path, &target)
    })
    .await
}

/// `copy(from, to) -> Promise<void>`
async fn js_copy<'js>(ctx: Ctx<'js>, from: String, to: String) -> QjsResult<()> {
    let source = path_arg(&ctx, &from)?;
    let target = path_arg(&ctx, &to)?;
    let state = file_state(&ctx)?;
    authorize(
        &ctx,
        &state,
        &format!("复制 → {}", target.display()),
        &source,
        &scope_of(&target),
    )?;

    blocking(&ctx, "复制", &source, move |path| {
        copy_recursive(path, &target)
    })
    .await
}

/// 递归复制（文件 / 目录 / 符号链接）。
fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = source.symlink_metadata()?;

    if metadata.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }

    std::fs::copy(source, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 绝对路径的原样写法（Unix 与 Windows 盘符）。
    #[test]
    fn accepts_absolute_paths() {
        assert_eq!(
            resolve_path("/tmp/a.txt").unwrap(),
            PathBuf::from("/tmp/a.txt")
        );
        assert_eq!(
            resolve_path("C:/Users/me/a.txt").unwrap(),
            PathBuf::from("C:/Users/me/a.txt")
        );
        assert_eq!(
            resolve_path(r"C:\Users\me\a.txt").unwrap(),
            PathBuf::from(r"C:\Users\me\a.txt")
        );
    }

    /// 相对路径一律拒绝（脚本的工作目录不可预期）。
    #[test]
    fn rejects_relative_paths() {
        for raw in ["a.txt", "./a.txt", "../a.txt", "sub/dir/a.txt"] {
            let err = resolve_path(raw).expect_err("相对路径应当报错");
            assert!(err.contains("绝对路径"), "{raw}: {err}");
        }
        assert!(resolve_path("   ").is_err(), "空路径应当报错");
    }

    /// `~` 展开成主目录。
    #[test]
    fn expands_home() {
        let home = home_dir().expect("测试环境应当有主目录");
        assert_eq!(resolve_path("~").unwrap(), home);
        assert_eq!(resolve_path("~/a.txt").unwrap(), home.join("a.txt"));
        assert_eq!(resolve_path("~\\a.txt").unwrap(), home.join("a.txt"));
    }

    /// git-bash 与 Cygwin 风格转成盘符路径。
    #[test]
    fn rewrites_msys_paths() {
        assert_eq!(resolve_path("/d/xx").unwrap(), PathBuf::from("D:/xx"));
        assert_eq!(resolve_path("/d").unwrap(), PathBuf::from("D:/"));
        assert_eq!(
            resolve_path("/cygdrive/c/Users/me").unwrap(),
            PathBuf::from("C:/Users/me")
        );
        // 单个字母但不是盘符形式（`/dev/null`）要保持原样
        assert_eq!(
            resolve_path("/dev/null").unwrap(),
            PathBuf::from("/dev/null")
        );
    }

    /// 「本目录都允许」记的是目标的父目录。
    #[test]
    fn scope_is_parent_directory() {
        assert_eq!(scope_of(Path::new("/tmp/a/b.txt")), PathBuf::from("/tmp/a"));
        assert_eq!(scope_of(Path::new("/a.txt")), PathBuf::from("/"));
    }
}
