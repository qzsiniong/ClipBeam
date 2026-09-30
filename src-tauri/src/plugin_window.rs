//! 插件自定义界面：**窗口参数**、**协议路径**与**路径安全校验**。
//!
//! # 这一层负责什么
//!
//! 插件可以带自己的 HTML / CSS / JS / 图标。宿主把这件事拆成三块，理解它们的关系就理解了
//! 插件窗口的整个数据流：
//!
//! 1. **协议**：插件目录里的文件通过自定义协议 `clipbeam-plugin://localhost/p-<插件 id>/<文件>`
//!    提供给 webview（见 [`PLUGIN_PROTOCOL`] 与 [`handle_protocol_request`]）。
//!    路径解析与越界校验都在这里 —— 这是**安全边界**，插件的 HTML 只能读到自己目录里的文件。
//! 2. **页面**：新建的窗口加载的是**应用自己的**一个路由 `index.html#/plugin-window/<win-id>`，
//!    它再把插件页面放进一个 `<iframe sandbox>` 里（见 [`plugin_page_url`]）。
//!    为什么要套一层 iframe：插件页面拿不到 `window.__TAURI_INTERNALS__`，
//!    也就无法直接调用宿主的 IPC，只能通过 `postMessage` 说话 —— 这是「插件能做什么」
//!    由能力清单与权限决定、而不是「它拿到了一个万能对象」的最后一环。
//! 3. **窗口参数**：插件只给「想要多大、要不要置顶」这类愿望，尺寸上下限由宿主夹取
//!    （见 [`WindowConfig::from_json`] 与 [`clamp_size`]）—— 否则插件能建出
//!    100000×100000 的窗口把桌面搞崩。
//!
//! # 本模块只做纯逻辑
//!
//! 不碰 Tauri 的窗口/协议 API，只产出「该建一个什么样的窗口」「这个请求该回什么字节」。
//! 于是这套规则可以在没有图形环境的机器上跑测试（见文件末尾），
//! 真正建窗与注册协议在 `plugin_manager.rs` / `lib.rs` 里。

// `parse_plugin_protocol_path` 等为协议处理器准备的能力目前只被测试与将来的扩展用到；
// 保留它们是因为协议路径格式是**外部契约**（插件页面里的相对引用写死了它）。
#![allow(dead_code, reason = "协议路径解析是外部契约，测试与调试入口都要用")]

use std::path::{Component, Path, PathBuf};

use clipbeam_plugins::{PluginError, WindowOptions, WindowRequest, WindowResponse};
use serde::{Deserialize, Serialize};

/// 插件窗口的标签前缀（形如 `plugin-window-<插件 id>-<序号>`）。
///
/// 前缀固定 + 只含 ASCII 字母数字与连字符：Tauri 的 capability 用 `windows` 通配
/// 匹配标签（`plugin-window-*`），标签一旦不可预测，就配不出权限来。
pub const PLUGIN_WINDOW_PREFIX: &str = "plugin-window-";

/// 自定义协议名。
pub const PLUGIN_PROTOCOL: &str = "clipbeam-plugin";

/// 插件页面在协议里的路径前缀（`/p-<插件 id>/...`）。
const PROTOCOL_PATH_PREFIX: &str = "p-";

/// 窗口尺寸下限：再小就不是一个能用的界面了。
const MIN_WINDOW_SIZE: u32 = 200;
/// 窗口尺寸上限：挡住「插件建一个 10 万像素的窗口」这种把桌面搞崩的输入。
const MAX_WINDOW_SIZE: u32 = 8000;
/// 缺省尺寸（插件没给时）。
const DEFAULT_WIDTH: f64 = 720.0;
const DEFAULT_HEIGHT: f64 = 520.0;

/// 一个已打开窗口的宿主侧状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWindow {
    /// 宿主给这次打开的**窗口标签**（Tauri 窗口 id 与前端路由都用它）。
    pub label: String,
    /// 插件侧的窗口 id（`open()` 返回给插件的那个）。
    pub plugin_window_id: String,
    /// 哪个插件的窗口。
    pub plugin_id: String,
    /// 插件页面（相对插件目录的路径）。
    pub page: String,
    /// 窗口标题。
    pub title: String,
    /// 打开时间（epoch 毫秒，诊断与排序用）。
    pub opened_at_ms: u64,
}

/// 窗口尺寸（已夹取到合法区间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSize {
    /// 逻辑宽度。
    pub width: u32,
    /// 逻辑高度。
    pub height: u32,
}

/// 一个窗口最终该长什么样（宿主夹取后的结果）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowConfig {
    /// 标题。
    pub title: String,
    /// 尺寸。
    pub size: WindowSize,
    /// 是否可缩放。
    pub resizable: bool,
    /// 是否置顶。
    pub always_on_top: bool,
    /// 是否显示系统标题栏与边框。
    pub decorations: bool,
    /// 是否透明（需要插件页面自带背景）。
    pub transparent: bool,
    /// 是否居中（否则交给系统摆放）。
    pub center: bool,
    /// 插件页面（相对插件目录）。
    pub page: String,
}

impl WindowConfig {
    /// 从插件给的 JSON 生成一份**已夹取**的配置。
    ///
    /// 宽容但不放任：
    ///
    /// * 不认识或类型不对的字段**忽略并回落缺省**（插件多传一个字段不该让开窗失败）；
    /// * 尺寸按 [`MIN_WINDOW_SIZE`] / [`MAX_WINDOW_SIZE`] 夹取；
    /// * `page` 必须是一个**插件目录内的普通文件名**（不含路径分隔符、不是 `.`/`..`），
    ///   真正的读文件边界在协议处理器的 [`resolve_plugin_file`] 里再查一次。
    pub fn from_json(title_fallback: &str, options: &WindowOptions) -> Result<Self, PluginError> {
        let raw = serde_json::to_value(options)
            .map_err(|err| PluginError::InvalidArgument(format!("窗口参数无法序列化：{err}")))?;

        let title = match raw.get("title").and_then(|value| value.as_str()) {
            Some(title) if !title.trim().is_empty() => title.to_string(),
            _ => title_fallback.to_string(),
        };

        let size = clamp_size(
            raw.get("width").and_then(serde_json::Value::as_f64),
            raw.get("height").and_then(serde_json::Value::as_f64),
        );

        let page = match raw.get("page").and_then(|value| value.as_str()) {
            Some(page) if !page.trim().is_empty() => page.to_string(),
            _ => "index.html".to_string(),
        };
        check_page_name(&page)?;

        let flag = |key: &str, fallback: bool| {
            raw.get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(fallback)
        };

        Ok(Self {
            title,
            size,
            // 缺省：可缩放、不置顶、有边框、不透明、不强行居中
            resizable: flag("resizable", true),
            always_on_top: flag("alwaysOnTop", false),
            decorations: flag("decorations", true),
            transparent: flag("transparent", false),
            center: flag("center", false),
            page,
        })
    }
}

/// 夹取尺寸：夹到合法区间；给了 `NaN`/负数/0 时回落缺省。
///
/// 单独抽出来是因为「插件给的数字能不能直接用」这件事必须有一条独立可测的分界线。
pub fn clamp_size(width: Option<f64>, height: Option<f64>) -> WindowSize {
    WindowSize {
        width: clamp_axis(width, DEFAULT_WIDTH),
        height: clamp_axis(height, DEFAULT_HEIGHT),
    }
}

/// 单个轴上的夹取。
fn clamp_axis(value: Option<f64>, fallback: f64) -> u32 {
    let Some(value) = value else {
        return fallback as u32;
    };
    if !value.is_finite() || value <= 0.0 {
        return fallback as u32;
    }
    let value = value.round();
    value.clamp(MIN_WINDOW_SIZE as f64, MAX_WINDOW_SIZE as f64) as u32
}

/// `page` 必须是一个普通文件名（可以带子目录，但不能跳出插件目录）。
fn check_page_name(page: &str) -> Result<(), PluginError> {
    if page.trim().is_empty() {
        return Err(PluginError::InvalidArgument("窗口页面不能为空".into()));
    }
    let candidate = Path::new(page);
    if candidate.is_absolute() {
        return Err(PluginError::InvalidArgument(format!(
            "窗口页面必须是相对插件目录的路径，收到绝对路径：{page:?}"
        )));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PluginError::InvalidArgument(format!(
                    "窗口页面不能包含 ..（不允许跳出插件目录）：{page:?}"
                )))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PluginError::InvalidArgument(format!(
                    "窗口页面必须是插件目录内的相对路径：{page:?}"
                )))
            }
        }
    }
    Ok(())
}

/// 生成一个插件窗口标签。
pub fn plugin_window_label(plugin_id: &str, seq: u32) -> String {
    format!("{PLUGIN_WINDOW_PREFIX}{plugin_id}-{seq}")
}

/// 判断一个窗口标签是不是插件窗口。
pub fn is_plugin_window(label: &str) -> bool {
    label.starts_with(PLUGIN_WINDOW_PREFIX)
}

/// 插件窗口在应用内的路由（新窗口加载的就是它）。
///
/// 用查询串而不是路径段：路由只用一条 `/plugin-window`，窗口 id 放查询里，
/// 前端读起来更直白，也不用为「窗口 id 里可能出现斜杠」担心。
pub fn plugin_window_route(label: &str) -> String {
    format!("index.html#/plugin-window?label={label}")
}

/// 插件页面在自定义协议里的 URL（交给 iframe 的 `src`）。
///
/// 形如 `clipbeam-plugin://localhost/p-hello-plugin/index.html`。
pub fn plugin_page_url(plugin_id: &str, page: &str) -> String {
    let page = page.trim_start_matches('/');
    format!("{PLUGIN_PROTOCOL}://localhost/{PROTOCOL_PATH_PREFIX}{plugin_id}/{page}")
}

/// 一个已登记窗口该加载的路由：`index.html#/plugin-window?label=…&plugin=…&page=…`。
///
/// 三个参数都放在查询串里，于是**那个页面自己就能把一切拼回来**（iframe 的 src、
/// 往宿主回报消息时带哪个标签），不需要额外的「初始化」事件 —— 少一条消息就少一处
/// 可能不同步的地方。
pub fn window_route_for(window: &OpenWindow) -> String {
    let page = url_encode(&window.page);
    let plugin = url_encode(&window.plugin_id);
    format!(
        "index.html#/plugin-window?label={}&plugin={plugin}&page={page}",
        url_encode(&window.label)
    )
}

/// 最小化的 URL 查询参数编码（`encodeURIComponent` 的子集）。
///
/// 只编码那些在查询串里真的会引起歧义、或在哈希路由里会被解析器吃掉字符。
/// 插件 id / 窗口标签都被限制成 ASCII 字母数字与 `-._`，真正需要编码的是 `page`
/// （可能含 `/`：`ui/panel.html` —— 斜杠在查询串里合法但容易被路由库当成路径段）。
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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
    let plugin_id = plugin.strip_prefix(PROTOCOL_PATH_PREFIX)?;
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

/// 处理一次自定义协议请求（在 `lib.rs` 里注册进 Tauri）。
///
/// 边界都在这一个函数里，便于审查：
///
/// * 路径必须是 `/p-<插件 id>/<文件>` 的形状，否则 404；
/// * 插件 id 必须在**当前发现到的插件**里（不是随便一个目录名）；
/// * 文件必须落在那个插件的目录内（[`resolve_plugin_file`] 管词法 + 符号链接两道）；
/// * 只读文件，不给目录列表；越界与不存在都回 404，不泄露「这个路径存不存在」。
pub fn handle_protocol_request(plugins_root: &Path, path: &str) -> Result<Vec<u8>, PluginError> {
    let Some((plugin_id, relative)) = parse_plugin_protocol_path(path) else {
        return Err(PluginError::Failed(format!("无法识别的协议路径：{path:?}")));
    };

    let plugin_dir = plugins_root.join(&plugin_id);
    if !plugin_dir.is_dir() {
        return Err(PluginError::Failed(format!("没有这个插件：{plugin_id:?}")));
    }

    let resolved = resolve_plugin_file(&plugin_dir, &relative).map_err(PluginError::Failed)?;
    std::fs::read(&resolved)
        .map_err(|err| PluginError::Failed(format!("读取 {} 失败：{err}", resolved.display())))
}

/// 自定义协议的入口（注册进 Tauri；**在协议线程上被调用**）。
///
/// 只回两类东西：插件目录里的字节，或者 404。四种失败（路径形状不对 / 没有这个插件 /
/// 文件不存在 / 越界）**统一回 404** —— 不区分「不存在」与「不许看」，
/// 免得协议变成一个探测文件系统的接口。
pub fn protocol_response(request: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let path = request.uri().path().to_string();
    let plugins_root = clipbeam_plugins::catalog::plugins_dir();

    match handle_protocol_request(&plugins_root, &path) {
        Ok(body) => {
            let content_type = content_type_of(&path);
            tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", content_type)
                // 插件页面通常不设缓存：调试时改完文件刷新就能看到
                .header("Cache-Control", "no-store")
                .body(body)
                .unwrap_or_else(|_| empty_response(500))
        }
        Err(err) => {
            log::debug!("插件协议请求被拒绝（{path}）：{err}");
            empty_response(404)
        }
    }
}

/// 一个只有状态码的空响应。
fn empty_response(status: u16) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .body(Vec::new())
        .expect("空响应一定能构造出来")
}

/// 处理一次窗口请求（在**主线程**上调用；真正建/关窗口在 `plugin_manager.rs` 里）。
///
/// 这里只做「参数校验 + 夹取 + 转换成宿主能执行的动作」，不碰 Tauri API ——
/// 于是窗口参数的规则可以单测，而建窗那段只剩「照着做」。
pub fn plan_window(
    request: &WindowRequest,
    plugin_name: &str,
) -> Result<PlannedWindowAction, PluginError> {
    match request {
        WindowRequest::Open { window_id, options } => {
            if window_id.trim().is_empty() {
                return Err(PluginError::InvalidArgument("窗口 id 不能为空".into()));
            }
            let config = WindowConfig::from_json(&format!("{plugin_name} 窗口"), options)?;
            Ok(PlannedWindowAction::Open {
                window_id: window_id.clone(),
                config,
            })
        }
        WindowRequest::Close { window_id } => Ok(PlannedWindowAction::Close {
            window_id: window_id.clone(),
        }),
    }
}

/// [`plan_window`] 的产物：宿主接下来该做的那一件事。
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedWindowAction {
    /// 建一个窗口。
    Open {
        /// 插件侧的窗口 id。
        window_id: String,
        /// 夹取后的窗口配置。
        config: WindowConfig,
    },
    /// 关一个窗口。
    Close {
        /// 插件侧的窗口 id。
        window_id: String,
    },
}

/// 收敛窗口请求的回执：把宿主的两个返回值拼成插件侧要的结果。
pub fn window_outcome(
    request_id: u64,
    outcome: Result<(String, u32), PluginError>,
) -> WindowResponse {
    match outcome {
        Ok((label, slot)) => WindowResponse {
            request_id,
            outcome: Ok(Some((label, slot))),
        },
        Err(err) => WindowResponse {
            request_id,
            outcome: Err(err),
        },
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

    // ── 窗口参数 ────────────────────────────────────────────────────────────

    fn options_from(json: serde_json::Value) -> WindowOptions {
        serde_json::from_value(json).expect("测试参数应当能解析成 WindowOptions")
    }

    #[test]
    fn window_config_falls_back_to_sane_defaults() {
        let config =
            WindowConfig::from_json("默认标题", &options_from(serde_json::json!({}))).unwrap();

        assert_eq!(config.title, "默认标题");
        assert_eq!(
            config.size,
            WindowSize {
                width: 720,
                height: 520
            }
        );
        assert_eq!(config.page, "index.html");
        assert!(config.resizable, "缺省应可缩放");
        assert!(!config.always_on_top, "缺省不置顶");
        assert!(config.decorations, "缺省有边框");
        assert!(!config.transparent);
        assert!(!config.center);
    }

    #[test]
    fn window_config_honours_explicit_values() {
        let config = WindowConfig::from_json(
            "默认",
            &options_from(serde_json::json!({
                "title": "我的面板",
                "width": 400,
                "height": 300,
                "resizable": false,
                "alwaysOnTop": true,
                "decorations": false,
                "transparent": true,
                "center": true,
                "page": "panel/index.html",
            })),
        )
        .unwrap();

        assert_eq!(config.title, "我的面板");
        assert_eq!(
            config.size,
            WindowSize {
                width: 400,
                height: 300
            }
        );
        assert!(!config.resizable);
        assert!(config.always_on_top);
        assert!(!config.decorations);
        assert!(config.transparent);
        assert!(config.center);
        assert_eq!(config.page, "panel/index.html", "允许子目录");
    }

    #[test]
    fn window_size_is_clamped() {
        // 太小 → 抬到下限
        let size = clamp_size(Some(10.0), Some(10.0));
        assert_eq!(
            size,
            WindowSize {
                width: MIN_WINDOW_SIZE,
                height: MIN_WINDOW_SIZE
            }
        );

        // 太大 → 夹到上限
        let size = clamp_size(Some(100_000.0), Some(100_000.0));
        assert_eq!(
            size,
            WindowSize {
                width: MAX_WINDOW_SIZE,
                height: MAX_WINDOW_SIZE
            }
        );

        // 缺失 / 非法 → 缺省（不能因为插件传了 NaN 就建出 0×0 的窗口）
        for bad in [
            None,
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(0.0),
            Some(-5.0),
        ] {
            let size = clamp_size(bad, bad);
            assert_eq!(
                size,
                WindowSize {
                    width: DEFAULT_WIDTH as u32,
                    height: DEFAULT_HEIGHT as u32
                },
                "{bad:?} 应当回落缺省尺寸"
            );
        }

        // 合法区间内的值原样保留（四舍五入）
        assert_eq!(
            clamp_size(Some(640.4), Some(480.6)),
            WindowSize {
                width: 640,
                height: 481
            }
        );
    }

    #[test]
    fn unknown_option_fields_are_ignored() {
        // 插件多传字段不该让开窗失败（`WindowOptions` 不做 deny_unknown_fields）
        let config = WindowConfig::from_json(
            "默认",
            &options_from(serde_json::json!({ "width": 300, "whatIsThis": true })),
        );
        assert!(config.is_ok(), "未知字段应当被忽略：{config:?}");
    }

    #[test]
    fn window_page_cannot_escape_the_plugin_directory() {
        for bad in ["../evil.html", "/etc/passwd", "a/../../b.html"] {
            let err =
                WindowConfig::from_json("默认", &options_from(serde_json::json!({ "page": bad })))
                    .expect_err("{bad} 应当被拒绝");
            assert!(
                matches!(err, PluginError::InvalidArgument(_)),
                "{bad} 应当报参数错误：{err:?}"
            );
        }
    }

    #[test]
    fn window_route_carries_everything_the_page_needs() {
        let window = OpenWindow {
            label: "plugin-window-hello-plugin-3".into(),
            plugin_window_id: "w1".into(),
            plugin_id: "hello-plugin".into(),
            page: "ui/panel.html".into(),
            title: "面板".into(),
            opened_at_ms: 0,
        };

        let route = window_route_for(&window);
        assert!(
            route.starts_with("index.html#/plugin-window?"),
            "应当是应用内路由：{route}"
        );
        assert!(
            route.contains("label=plugin-window-hello-plugin-3"),
            "{route}"
        );
        assert!(route.contains("plugin=hello-plugin"), "{route}");
        assert!(
            route.contains("page=ui%2Fpanel.html"),
            "页面路径必须被编码（斜杠不能留在查询串里）：{route}"
        );
    }

    #[test]
    fn window_labels_and_routes_are_predictable() {
        let label = plugin_window_label("hello-plugin", 1);
        assert_eq!(label, "plugin-window-hello-plugin-1");
        assert!(is_plugin_window(&label));
        for builtin in ["main", "scripting", "progress", "standby"] {
            assert!(!is_plugin_window(builtin));
        }

        assert_eq!(
            plugin_window_route(&label),
            "index.html#/plugin-window?label=plugin-window-hello-plugin-1"
        );
        assert_eq!(
            plugin_page_url("hello-plugin", "index.html"),
            "clipbeam-plugin://localhost/p-hello-plugin/index.html"
        );
    }

    // ── 协议路径 ────────────────────────────────────────────────────────────

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

    // ── 文件边界 ────────────────────────────────────────────────────────────

    #[test]
    fn resolves_a_file_inside_the_plugin_directory() {
        let dir = temp_dir("ok");
        std::fs::write(dir.join("icon.png"), b"x").unwrap();

        let path = resolve_plugin_file(&dir, "icon.png").expect("应当解析成功");
        assert!(path.is_absolute(), "返回的应当是绝对路径");
        assert!(path.ends_with("icon.png"));

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
            let resolved = resolve_plugin_file(&dir, bad).unwrap_or_else(|_| PathBuf::new());
            assert!(
                resolved.as_os_str().is_empty(),
                "{bad:?} 应当被拒绝，却解析成了 {}",
                resolved.display()
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

    // ── 协议处理 ────────────────────────────────────────────────────────────

    #[test]
    fn protocol_serves_files_of_a_known_plugin_only() {
        let root = temp_dir("protocol-root");
        let plugin = root.join("hello-plugin");
        std::fs::create_dir_all(plugin.join("assets")).unwrap();
        std::fs::write(plugin.join("index.html"), b"<h1>hi</h1>").unwrap();
        std::fs::write(plugin.join("assets/app.js"), b"console.log(1)").unwrap();

        // 正常取到
        let body = handle_protocol_request(&root, "/p-hello-plugin/index.html").unwrap();
        assert_eq!(body, b"<h1>hi</h1>");
        let body = handle_protocol_request(&root, "/p-hello-plugin/assets/app.js").unwrap();
        assert_eq!(body, b"console.log(1)");

        // 不存在的插件 / 文件 / 越界路径：都报错（调用方回 404）
        for bad in [
            "/p-unknown/index.html",
            "/p-hello-plugin/nope.html",
            "/p-hello-plugin/../secret.txt",
            "/not-a-plugin-path",
        ] {
            assert!(
                handle_protocol_request(&root, bad).is_err(),
                "{bad} 应当被拒绝"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    // ── 请求规划 ────────────────────────────────────────────────────────────

    #[test]
    fn plans_open_and_close_actions() {
        let open = plan_window(
            &WindowRequest::Open {
                window_id: "w1".into(),
                options: options_from(serde_json::json!({ "width": 300, "page": "ui.html" })),
            },
            "示例插件",
        )
        .unwrap();

        match open {
            PlannedWindowAction::Open { window_id, config } => {
                assert_eq!(window_id, "w1");
                assert_eq!(config.size.width, 300);
                assert_eq!(config.page, "ui.html");
                assert_eq!(config.title, "示例插件 窗口", "标题缺省带插件名");
            }
            other => panic!("应当是 Open：{other:?}"),
        }

        let close = plan_window(
            &WindowRequest::Close {
                window_id: "w1".into(),
            },
            "示例插件",
        )
        .unwrap();
        assert_eq!(
            close,
            PlannedWindowAction::Close {
                window_id: "w1".into()
            }
        );
    }

    #[test]
    fn empty_window_id_is_rejected() {
        let err = plan_window(
            &WindowRequest::Open {
                window_id: "  ".into(),
                options: options_from(serde_json::json!({})),
            },
            "示例插件",
        )
        .expect_err("空窗口 id 应当被拒绝");
        assert!(matches!(err, PluginError::InvalidArgument(_)), "{err:?}");
    }
}
