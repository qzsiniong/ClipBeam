//! 系统托盘:Tauri 2 TrayIconBuilder + Menu。
//! - 菜单项用 IconMenuItemBuilder 配 32x32 矢量图标(emoji 已移除)
//! - 菜单项热键用 accelerator 参数,macOS 自动浅色右对齐显示
//! - 忙时禁用执行类条目、状态行动态更新进度
//! - 托盘图标动态绘制进度环(单色 template,自动适配明暗主题)

use crate::config::Config;
use image::{ImageBuffer, Rgba, RgbaImage};
use tauri::{
    image::Image,
    menu::{
        IconMenuItem, IconMenuItemBuilder, IsMenuItem, Menu, PredefinedMenuItem, Submenu,
        SubmenuBuilder,
    },
    tray::TrayIconBuilder,
    App, AppHandle, Manager, Wry,
};

pub const M_STATUS: &str = "status";
pub const M_SEND_RAW: &str = "send_raw";
pub const M_SEND: &str = "send";
pub const M_RECV: &str = "recv";
pub const M_DEPLOY_TYPE: &str = "deploy_type";
pub const M_DEPLOY_COPY: &str = "deploy_copy";
pub const M_SEND_FILE: &str = "send_file";
pub const M_SCRIPTS: &str = "scripts";
pub const M_SETTINGS: &str = "settings";
pub const M_QUIT: &str = "quit";

/// 托盘图标的 id（全应用唯一；重建菜单时按它找回托盘）。
pub const TRAY_ID: &str = "main";

/// 插件菜单项 id 的前缀（`plugin:<插件 id>:<动作 id>`）。
pub const PLUGIN_MENU_PREFIX: &str = "plugin:";

/// 托盘里的一个插件动作菜单项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTrayItem {
    /// 哪个插件（插件 id）。
    pub plugin_id: String,
    /// 动作 id（对应 `plugin.json` 的 `menus[].id`）。
    pub action_id: String,
    /// 菜单上显示的文字。
    pub label: String,
    /// 是否可点击。
    pub enabled: bool,
}

/// 拼一个插件菜单项 id。
pub fn plugin_menu_id(plugin_id: &str, action_id: &str) -> String {
    format!("{PLUGIN_MENU_PREFIX}{plugin_id}:{action_id}")
}

/// 解析插件菜单项 id；不是插件项时返回 `None`。
///
/// 用 `split_once` 而不是 `split(':')`：插件 id 不允许含 `:`（见 `clipbeam_plugins::id`），
/// 而动作 id 是插件作者写的，这里对多余的分隔符保持宽容（余下部分整体当动作 id）。
pub fn parse_plugin_menu_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(PLUGIN_MENU_PREFIX)?;
    let (plugin_id, action_id) = rest.split_once(':')?;
    if plugin_id.is_empty() || action_id.is_empty() {
        return None;
    }
    Some((plugin_id, action_id))
}

/// 构建托盘（首次）：建图标与菜单，并把菜单项句柄存进 State 供忙闲切换使用。
///
/// `plugin_menus` 是插件**清单声明**的动作菜单。它只决定菜单项在不在，不决定插件跑不跑：
/// 点一个没启用插件的菜单项时，[`crate::plugin_manager`] 会给出「请先启用」的明确提示
/// —— 比「菜单项时有时无」好理解。
pub fn build(app: &App, cfg: &Config, plugin_menus: &[PluginTrayItem]) -> tauri::Result<()> {
    let items = TrayItems::build(app, cfg)?;
    let (menu, plugin_submenu, plugin_handles) = assemble_menu(app, &items, plugin_menus)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| Image::new(&[], 1, 1));

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("ClipBeam 剪贴板桥接")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;

    // macOS 27+: 显式要求显示菜单项图像(系统默认全部隐藏)
    #[cfg(target_os = "macos")]
    force_menu_icons_visible_impl(&app.handle().clone());

    app.manage(items);
    app.manage(PluginMenuState {
        menu,
        inner: std::sync::Mutex::new(PluginMenus {
            submenu: plugin_submenu,
            handles: plugin_handles,
        }),
    });

    Ok(())
}

/// 同步插件子菜单（启用/停用/刷新插件后调用）。
///
/// **只替换插件子菜单**，其它菜单项一根手指都不碰：
///
/// * 其它菜单项的句柄在 [`TrayItems`] 里，[`set_busy`] / [`set_status_text`] 还在用它们 ——
///   重建整个菜单会让那些句柄变成悬空引用；
/// * 菜单项句柄必须活到「菜单不再引用它」为止，所以新句柄交给 [`PluginMenuState`] 持有，
///   旧的由调用方在替换完成之后再释放；
/// * macOS 上新加的菜单项图像默认隐藏，替换后要再跑一次 [`force_menu_icons_visible`]。
pub fn sync_plugin_menus(app: &AppHandle, plugin_menus: &[PluginTrayItem]) -> tauri::Result<()> {
    if app.try_state::<PluginMenuState>().is_none() {
        // 托盘还没建起来（启动早期）：不报错，插件菜单会在下一次同步时补上
        return Ok(());
    }

    // 先把「旧的子菜单 + 上一批句柄」取出来：句柄必须活到旧子菜单被摘掉之后
    let (menu, previous) = {
        let state = app.state::<PluginMenuState>();
        let mut inner = state.inner.lock().unwrap();
        let previous = std::mem::take(&mut *inner);
        (state.menu.clone(), previous)
    };

    // 先建新的并挂上，再摘旧的：任何时刻菜单里都有一份可用的插件菜单
    let (new_submenu, new_handles) = build_plugin_submenu(app, plugin_menus)?;
    menu.append(&new_submenu)?;
    if let Some(old) = previous.submenu.as_ref() {
        menu.remove(old)?;
    }
    // 到这里旧句柄才真正用完（菜单已经不再引用它们）
    drop(previous);

    {
        let state = app.state::<PluginMenuState>();
        let mut inner = state.inner.lock().unwrap();
        inner.submenu = Some(new_submenu);
        inner.handles = new_handles;
    }

    #[cfg(target_os = "macos")]
    force_menu_icons_visible_state(app);

    Ok(())
}

/// 一个插件菜单项的句柄（必须活到菜单被替换为止）。
///
/// 字段「没被读过」是**故意的**：这里持有它们就是为了不让句柄被回收。
#[allow(dead_code, reason = "持有句柄即用途：提前释放会留下悬空菜单项")]
pub struct PluginMenuHandle {
    /// 菜单项本体。
    pub item: IconMenuItem<Wry>,
    /// 它的 id（`plugin:<插件 id>:<动作 id>`）。
    pub id: String,
}

/// 建出「插件」子菜单及其条目。
fn build_plugin_submenu(
    app: &AppHandle,
    plugin_menus: &[PluginTrayItem],
) -> tauri::Result<(Submenu<Wry>, Vec<PluginMenuHandle>)> {
    let submenu = SubmenuBuilder::new(app, "插件").build()?;
    let mut handles = Vec::new();

    for item in plugin_menus {
        let id = plugin_menu_id(&item.plugin_id, &item.action_id);
        let entry = IconMenuItemBuilder::with_id(id.clone(), &item.label)
            .enabled(item.enabled)
            .build(app)?;
        submenu.append(&entry)?;
        handles.push(PluginMenuHandle { item: entry, id });
    }

    Ok((submenu, handles))
}

/// 组装菜单的返回值：菜单本体 + 插件子菜单（没有插件项时为 `None`）+ 它的菜单项句柄。
type AssembledMenu = (Menu<Wry>, Option<Submenu<Wry>>, Vec<PluginMenuHandle>);

/// 组装完整托盘菜单：固定项（复用句柄）+ 插件子菜单 + 退出。
fn assemble_menu(
    app: &App,
    items: &TrayItems,
    plugin_menus: &[PluginTrayItem],
) -> tauri::Result<AssembledMenu> {
    let quit = IconMenuItemBuilder::with_id(M_QUIT, "退出 ClipBeam")
        .icon(icon_quit())
        .build(app)?;
    let plugin_part = if plugin_menus.is_empty() {
        (None, Vec::new())
    } else {
        let (submenu, handles) = build_plugin_submenu(&app.handle().clone(), plugin_menus)?;
        (Some(submenu), handles)
    };

    // `Menu::with_items` 要 `&[&dyn IsMenuItem]`：数组里的元素类型必须一致，
    // 所以先建分隔符再与句柄一起装进 `Vec<&dyn IsMenuItem>`
    let separators = [
        PredefinedMenuItem::separator(app)?,
        PredefinedMenuItem::separator(app)?,
        PredefinedMenuItem::separator(app)?,
        PredefinedMenuItem::separator(app)?,
        PredefinedMenuItem::separator(app)?,
    ];
    let mut all: Vec<&dyn IsMenuItem<Wry>> = vec![
        &items.status,
        &separators[0],
        &items.send_raw,
        &separators[1],
        &items.send,
        &items.recv,
        &separators[2],
        &items.deploy_type,
        &separators[3],
        &items.deploy_copy,
        &items.send_file,
        &items.scripts,
        &items.settings,
    ];
    if let Some(submenu) = &plugin_part.0 {
        all.push(submenu);
    }
    all.push(&separators[4]);
    all.push(&quit);

    let menu = Menu::with_items(app, &all)?;
    Ok((menu, plugin_part.0, plugin_part.1))
}

/// 托盘菜单项句柄(存入 Tauri State 供 set_busy/set_status_text 使用)。
pub struct TrayItems {
    pub status: IconMenuItem<Wry>,
    pub send: IconMenuItem<Wry>,
    pub recv: IconMenuItem<Wry>,
    pub deploy_type: IconMenuItem<Wry>,
    pub send_raw: IconMenuItem<Wry>,
    pub deploy_copy: IconMenuItem<Wry>,
    pub scripts: IconMenuItem<Wry>,
    pub settings: IconMenuItem<Wry>,
    /// 「发送文件」：与其它执行类菜单项一起随忙闲启停。
    pub send_file: IconMenuItem<Wry>,
}

impl TrayItems {
    /// 建出全部固定菜单项。
    fn build(app: &App, cfg: &Config) -> tauri::Result<Self> {
        // 热键作为 accelerator 传入,macOS 自动浅色右对齐显示;图标由 IconMenuItemBuilder 承载
        let send_raw = IconMenuItemBuilder::with_id(M_SEND_RAW, "发送本机剪贴板(原样)")
            .accelerator(cfg.send_raw_hotkey.as_str())
            .icon(icon_send())
            .build(app)?;
        let send = IconMenuItemBuilder::with_id(M_SEND, "发送本机剪贴板 → 远程")
            .accelerator(cfg.send_hotkey.as_str())
            .icon(icon_send())
            .build(app)?;
        let recv = IconMenuItemBuilder::with_id(M_RECV, "截屏接收远程二维码")
            .accelerator(cfg.recv_hotkey.as_str())
            .icon(icon_recv())
            .build(app)?;
        let status = IconMenuItemBuilder::with_id(M_STATUS, "状态:空闲")
            .enabled(false)
            .build(app)?;
        let deploy_type =
            IconMenuItemBuilder::with_id(M_DEPLOY_TYPE, "部署接收页到远程(键盘输入,约 3 分钟)")
                .icon(icon_deploy_type())
                .build(app)?;
        let deploy_copy =
            IconMenuItemBuilder::with_id(M_DEPLOY_COPY, "部署接收页(复制到宿主机剪贴板)")
                .icon(icon_deploy_copy())
                .build(app)?;
        let scripts = IconMenuItemBuilder::with_id(M_SCRIPTS, "运行脚本…")
            .icon(icon_scripts())
            .build(app)?;
        // 文件传输走脚本引擎（`04-file-transfer.ts`），所以文案里点明它是脚本通道。
        // 时长取决于文件大小与分片大小，不写死分钟数（会随改动漂移）。
        let send_file = IconMenuItemBuilder::with_id(M_SEND_FILE, "发送文件到远程(键盘通道)")
            .icon(icon_send())
            .build(app)?;
        let settings = IconMenuItemBuilder::with_id(M_SETTINGS, "设置…")
            .icon(icon_settings())
            .build(app)?;

        Ok(Self {
            status,
            send,
            recv,
            deploy_type,
            send_raw,
            deploy_copy,
            scripts,
            settings,
            send_file,
        })
    }
}

/// 插件菜单的可变状态。
///
/// 为什么必须持有这些字段：菜单项与子菜单的句柄要**活到菜单不再引用它们**为止；
/// 提前释放会留下悬空引用。
pub struct PluginMenuState {
    /// 托盘菜单本体（`remove` / `append` 都要它）。
    pub menu: Menu<Wry>,
    /// 可变部分：Tauri 的 `State<T>` 只给 `&T`，所以可变性得靠自己包一层。
    pub inner: std::sync::Mutex<PluginMenus>,
}

/// 插件菜单的可变部分。
///
/// `Default` 可以直接 derive：`Submenu<Wry>` 没有 `Default`，但它装在 `Option` 里，
/// 而「没有插件子菜单」正是 `None`。
#[derive(Default)]
pub struct PluginMenus {
    /// 当前的「插件」子菜单（`None` 表示当前没有插件菜单项）。
    pub submenu: Option<Submenu<Wry>>,
    /// 当前插件菜单项的句柄。
    pub handles: Vec<PluginMenuHandle>,
}

/// 应用一个插件的托盘运行时请求（**必须在主线程调用**）。
///
/// 返回值直接回给插件线程（`$plugin.tray.*` 的调用点）。
pub fn apply_plugin_tray_request(
    app: &AppHandle,
    plugin_id: &str,
    plugin_dir: Option<&std::path::Path>,
    request: &clipbeam_plugins::TrayRequest,
) -> Result<clipbeam_plugins::TrayOutcome, clipbeam_plugins::PluginError> {
    use clipbeam_plugins::{PluginError, TrayOutcome, TrayRequest};

    log::debug!("插件 {plugin_id} 的托盘请求：{request:?}");

    match request {
        TrayRequest::SetTooltip { text } => {
            let Some(tray) = app.tray_by_id(TRAY_ID) else {
                return Err(PluginError::Failed("找不到托盘图标".into()));
            };
            tray.set_tooltip(Some(text.as_str()))
                .map_err(|err| PluginError::Failed(format!("设置托盘提示失败：{err}")))?;
            Ok(TrayOutcome::Applied)
        }
        TrayRequest::SetIcon { path } => {
            let Some(relative) = path else {
                return Err(PluginError::InvalidArgument(
                    "setIcon 需要一个相对插件目录的图标路径".into(),
                ));
            };
            let resolved = crate::plugin_window::resolve_plugin_file(
                plugin_dir.ok_or_else(|| PluginError::Failed("找不到插件目录".into()))?,
                relative,
            )
            .map_err(PluginError::InvalidArgument)?;
            let image = load_tray_icon(&resolved)
                .map_err(|err| PluginError::Failed(format!("图标加载失败：{err}")))?;

            let Some(tray) = app.tray_by_id(TRAY_ID) else {
                return Err(PluginError::Failed("找不到托盘图标".into()));
            };
            tray.set_icon(Some(image))
                .map_err(|err| PluginError::Failed(format!("设置托盘图标失败：{err}")))?;
            Ok(TrayOutcome::Applied)
        }
        TrayRequest::SetBadge { text } => {
            // 徽标文字落在托盘的**状态菜单项**上：Tauri 2 没有暴露 NSStatusItem.button.title
            // （`set_title` 只作用于窗口），所以「状态栏上的小字」只能这样近似。
            let items = app.state::<TrayItems>();
            let label = match text {
                Some(text) => format!("状态:{text}"),
                None => "状态:空闲".to_string(),
            };
            items
                .status
                .set_text(label)
                .map_err(|err| PluginError::Failed(format!("设置徽标失败：{err}")))?;
            Ok(TrayOutcome::Applied)
        }
    }
}

/// 读一个图标文件并转成托盘能用的 RGBA 图像。
///
/// 用 `image` crate 解码（它已经因为其它用途进了依赖）：支持 PNG / JPEG / GIF / WebP，
/// 统一转成 RGBA8。建议插件用正方形 PNG（托盘图标会被缩放到系统尺寸）。
fn load_tray_icon(path: &std::path::Path) -> Result<Image<'static>, String> {
    let decoded = image::open(path).map_err(|err| err.to_string())?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Image::new_owned(rgba.into_raw(), width, height))
}

/// 从管理器读插件菜单并同步到托盘（启用/停用/刷新后调用，**主线程**）。
pub fn sync_from_manager(app: &AppHandle, manager: &crate::plugin_manager::PluginManager) {
    let items = manager.plugin_tray_items();
    if let Err(err) = sync_plugin_menus(app, &items) {
        log::warn!("同步插件托盘菜单失败：{err}");
    }
}

/// 在主线程上装好托盘请求的处理者。
///
/// # 为什么必须走这条事件
///
/// 托盘 API（tooltip / 图标 / 菜单项文本）只在**主线程**上安全。插件跑在自己的线程上，
/// 因此 `$plugin.tray.*` 会发一条事件回来，由主线程执行；结果经 oneshot 送回插件线程。
/// **主线程上绝不能同步等** —— 那样就没人处理这条事件了（必然死锁）。
pub fn setup_plugin_events(app: &AppHandle) {
    use tauri::Listener;

    let handle = app.clone();
    app.listen(crate::plugin_host::TRAY_REQUEST_EVENT, move |event| {
        let payload = event.payload();
        let Ok(message) = serde_json::from_str::<crate::plugin_host::TrayRequestMessage>(payload)
        else {
            log::warn!("收到无法解析的托盘请求：{payload}");
            return;
        };

        let Some(pending) =
            handle.try_state::<std::sync::Arc<crate::plugin_host::PendingTrayRequests>>()
        else {
            return;
        };
        let result = crate::plugin_host::handle_tray_request(&handle, message);
        pending.resolve(result);
    });
}

/// 在**已有状态**下强制显示菜单项图像（macOS 用；菜单项被替换后要重跑）。
#[cfg(target_os = "macos")]
fn force_menu_icons_visible_state(app: &AppHandle) {
    force_menu_icons_visible_impl(app);
}

/// 忙闲状态切换:禁用三个执行类菜单项,更新状态文本。
pub fn set_busy(app: &AppHandle, busy: bool, status: &str) -> tauri::Result<()> {
    let items = app.state::<TrayItems>();
    items.send.set_enabled(!busy)?;
    items.recv.set_enabled(!busy)?;
    items.deploy_type.set_enabled(!busy)?;
    items.send_file.set_enabled(!busy)?;
    items.status.set_text(status)?;
    // 恢复默认图标(空闲)
    if !busy {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            if let Some(icon) = app.default_window_icon() {
                let _ = tray.set_icon(Some(icon.clone()));
            }
        }
    }
    Ok(())
}

/// 显示托盘菜单。
pub fn _show_menu(app: &AppHandle) -> tauri::Result<()> {
    let app_clone = app.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(tray) = app_clone.tray_by_id("main") {
            let _ = tray.with_inner_tray_icon(|inner| {
                inner.show_menu();
            });
        }
    });

    Ok(())
}

/// 更新状态行进度文本(供 worker send_progress 调用)。
pub fn set_status_text(app: &AppHandle, text: &str) -> tauri::Result<()> {
    let items = app.state::<TrayItems>();
    items.status.set_text(text)
}

/// 更新托盘图标为带进度环的单色图标(macOS template,自动适配明暗主题)。
pub fn set_tray_progress(app: &AppHandle, percent: u8) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let img = draw_progress_icon(percent);
    let w = img.width();
    let h = img.height();
    let rgba = img.into_raw();
    let tauri_img = Image::new_owned(rgba, w, h);
    tray.set_icon(Some(tauri_img))?;
    // 设为 template image,macOS 自动适配明暗主题
    tray.set_icon_as_template(true)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 菜单图标:32x32 RGBA,程序化绘制,彩色方案在 macOS 明暗菜单下均可见
// ---------------------------------------------------------------------------

const MENU_ICON: u32 = 32;

/// 在 32x32 透明画布上执行绘制并转成 Tauri Image。
fn menu_icon(draw: impl Fn(&mut RgbaImage)) -> Image<'static> {
    let mut img: RgbaImage = ImageBuffer::from_pixel(MENU_ICON, MENU_ICON, Rgba([0, 0, 0, 0]));
    draw(&mut img);
    Image::new_owned(img.into_raw(), MENU_ICON, MENU_ICON)
}

/// 越界安全的单像素写入。
fn ico_put(img: &mut RgbaImage, x: i32, y: i32, c: Rgba<u8>) {
    if (0..MENU_ICON as i32).contains(&x) && (0..MENU_ICON as i32).contains(&y) {
        img.put_pixel(x as u32, y as u32, c);
    }
}

/// 闭区间填充矩形。
fn ico_rect(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba<u8>) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            ico_put(img, x, y, c);
        }
    }
}

/// 闭区间实心圆角矩形(r 为角半径)。
fn ico_round(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, r: i32, c: Rgba<u8>) {
    let r2 = r * r;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let cx = x.clamp(x0 + r, x1 - r);
            let cy = y.clamp(y0 + r, y1 - r);
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 实心圆盘。
fn ico_disc(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: Rgba<u8>) {
    let r2 = r * r;
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 实心三角形(重心坐标判定)。点顺序任意。
fn ico_triangle(img: &mut RgbaImage, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), c: Rgba<u8>) {
    let minx = p0.0.min(p1.0).min(p2.0).floor() as i32;
    let maxx = p0.0.max(p1.0).max(p2.0).ceil() as i32;
    let miny = p0.1.min(p1.1).min(p2.1).floor() as i32;
    let maxy = p0.1.max(p1.1).max(p2.1).ceil() as i32;
    let sign = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (p.0 - b.0) * (a.1 - b.1) - (a.0 - b.0) * (p.1 - b.1)
    };
    for y in miny..=maxy {
        for x in minx..=maxx {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d1 = sign(p, p0, p1);
            let d2 = sign(p, p1, p2);
            let d3 = sign(p, p2, p0);
            let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            if !(has_neg && has_pos) {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 粗细为 thickness 的线段(像素中心到线段距离 <= thickness/2)。
fn ico_line(img: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, c: Rgba<u8>) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    let half = thickness / 2.0;
    let pad = half.ceil() as i32 + 1;
    let minx = x0.min(x1) as i32 - pad;
    let maxx = x0.max(x1) as i32 + pad;
    let miny = y0.min(y1) as i32 - pad;
    let maxy = y0.max(y1) as i32 + pad;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = if len2 > 0.0 {
                (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let qx = x0 + t * dx;
            let qy = y0 + t * dy;
            let ddx = px - qx;
            let ddy = py - qy;
            if ddx * ddx + ddy * ddy <= half * half {
                ico_put(img, x, y, c);
            }
        }
    }
}

/// 发送:蓝色向上箭头。
fn icon_send() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([37, 99, 235, 255]); // blue-600
        ico_triangle(img, (16.0, 4.0), (6.0, 15.0), (26.0, 15.0), c);
        ico_rect(img, 13, 15, 18, 27, c);
    })
}

/// 接收:绿色向下箭头。
fn icon_recv() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([22, 163, 74, 255]); // green-600
        ico_rect(img, 13, 5, 18, 17, c);
        ico_triangle(img, (16.0, 28.0), (6.0, 17.0), (26.0, 17.0), c);
    })
}

/// 键盘部署:紫色键盘(外框 + 两排键帽)。
fn icon_deploy_type() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([124, 58, 237, 255]); // violet-600
        let clear = Rgba([0, 0, 0, 0]);
        ico_round(img, 3, 9, 28, 24, 3, c);
        ico_round(img, 6, 12, 25, 21, 2, clear);
        for &y in &[14i32, 18] {
            for &x in &[8i32, 11, 14, 17, 20] {
                ico_rect(img, x, y, x + 1, y + 1, c);
            }
        }
    })
}

/// 复制部署:青色板夹。
fn icon_deploy_copy() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([13, 148, 136, 255]); // teal-600
        let clear = Rgba([0, 0, 0, 0]);
        // 板夹主体(环)
        ico_round(img, 7, 5, 24, 28, 3, c);
        ico_round(img, 9, 8, 22, 26, 2, clear);
        // 顶部夹子(开口朝下的 U)
        ico_round(img, 12, 2, 19, 8, 2, c);
        ico_rect(img, 14, 4, 17, 8, clear);
    })
}

/// 运行脚本:琥珀色代码文档(外框 + 左侧竖线 + 两行代码条)。
fn icon_scripts() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([217, 119, 6, 255]); // amber-600
        let clear = Rgba([0, 0, 0, 0]);
        ico_round(img, 5, 4, 27, 28, 3, c);
        ico_round(img, 7, 6, 25, 26, 2, clear);
        // 代码左侧的「行号」竖线
        ico_rect(img, 9, 9, 10, 23, c);
        // 两行代码条
        ico_rect(img, 13, 11, 23, 13, c);
        ico_rect(img, 13, 16, 21, 18, c);
        ico_rect(img, 13, 21, 23, 23, c);
    })
}

/// 设置:中性灰滑块(三横线 + 圆钮),明暗菜单下均可见。
fn icon_settings() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([107, 114, 128, 255]); // gray-500
        ico_line(img, 4.0, 9.0, 18.0, 9.0, 2.5, c);
        ico_disc(img, 22, 9, 4, c);
        ico_line(img, 14.0, 16.0, 28.0, 16.0, 2.5, c);
        ico_disc(img, 10, 16, 4, c);
        ico_line(img, 4.0, 23.0, 18.0, 23.0, 2.5, c);
        ico_disc(img, 22, 23, 4, c);
    })
}

/// 退出:红色 X。
fn icon_quit() -> Image<'static> {
    menu_icon(|img| {
        let c = Rgba([220, 38, 38, 255]); // red-600
        ico_line(img, 8.0, 8.0, 24.0, 24.0, 4.0, c);
        ico_line(img, 24.0, 8.0, 8.0, 24.0, 4.0, c);
    })
}

/// 绘制 128x128 单色进度环 PNG(透明背景 + 环)。
/// 环内已完成部分 alpha=255,未完成部分 alpha=60(半透明背景环)。
fn draw_progress_icon(percent: u8) -> RgbaImage {
    let size = 128;
    let cx = 64.0;
    let cy = 64.0;
    let outer = 56.0;
    let inner = 44.0;
    let percent = percent.min(100) as f64 / 100.0;
    // 进度弧:从顶部(-90°)顺时针扫过
    let start_angle = -std::f64::consts::FRAC_PI_2;

    let mut img: RgbaImage = ImageBuffer::from_pixel(size, size, Rgba([0, 0, 0, 0]));

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < inner || dist > outer {
                continue;
            }
            // 计算像素角度(atan2,从顶部顺时针)
            let angle = dy.atan2(dx);
            // 归一化到 [-PI, PI],判断是否在进度弧内
            let in_progress = if percent >= 1.0 {
                true
            } else {
                // 把 angle 转换到 [start_angle, start_angle + TAU) 区间
                let mut a = angle - start_angle;
                while a < 0.0 {
                    a += std::f64::consts::TAU;
                }
                while a >= std::f64::consts::TAU {
                    a -= std::f64::consts::TAU;
                }
                a <= percent * std::f64::consts::TAU
            };
            let alpha = if in_progress { 255u8 } else { 60u8 };
            img.put_pixel(x, y, Rgba([0, 0, 0, alpha]));
        }
    }
    img
}

/// 托盘图标事件回调(预留)。
pub fn on_tray_event(_app: &AppHandle, _event: tauri::tray::TrayIconEvent) {}

// ---------------------------------------------------------------------------
// macOS 27 兼容:NSMenu 默认隐藏菜单项图像,需逐项设置 preferredImageVisibility
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
#[allow(
    dead_code,
    reason = "与 force_menu_icons_visible_impl 成对提供，供 App/AppHandle 两种入口"
)]
pub fn force_menu_icons_visible(app: &App) {
    force_menu_icons_visible_impl(app.handle());
}

/// 泛型实现：`App` 与 `AppHandle` 都实现了 `Manager`，因此只写一份。
#[cfg(target_os = "macos")]
fn force_menu_icons_visible_impl<R: tauri::Runtime>(manager: &AppHandle<R>) {
    use objc2::MainThreadMarker;

    /// NSMenuItemImageVisibilityVisible(macOS 27+,旧系统忽略该消息)
    const IMAGE_VISIBILITY_VISIBLE: isize = 1;

    let Some(tray) = manager.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.with_inner_tray_icon(|inner| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(status_item) = inner.ns_status_item() else {
            return;
        };
        let Some(menu) = status_item.menu(mtm) else {
            return;
        };
        unsafe {
            let n = menu.numberOfItems();
            for i in 0..n {
                if let Some(item) = menu.itemAtIndex(i) {
                    let _: () = objc2::msg_send![
                        &*item,
                        setPreferredImageVisibility: IMAGE_VISIBILITY_VISIBLE
                    ];
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 菜单项 id 的拼装与解析必须互为逆运算 —— 这是「点菜单能不能找到插件」的唯一通路。
    #[test]
    fn plugin_menu_ids_roundtrip() {
        let id = plugin_menu_id("hello-plugin", "hello");
        assert_eq!(id, "plugin:hello-plugin:hello");
        assert_eq!(
            parse_plugin_menu_id(&id),
            Some(("hello-plugin", "hello")),
            "解析结果必须与拼装一致"
        );
    }

    /// 动作 id 里带冒号时，余下部分整体当动作 id（插件作者写什么都别炸）。
    #[test]
    fn plugin_menu_id_keeps_action_ids_with_colons() {
        assert_eq!(
            parse_plugin_menu_id("plugin:demo:a:b"),
            Some(("demo", "a:b"))
        );
    }

    /// 非插件菜单项、以及残缺的插件项都要返回 `None`，好让调用方走原有的分支。
    #[test]
    fn non_plugin_menu_ids_are_rejected() {
        for id in [
            "settings",     // 应用自己的菜单项
            "send",         // 应用自己的菜单项
            "plugin:",      // 缺插件与动作
            "plugin:demo",  // 缺动作 id
            "plugin::go",   // 缺插件 id
            "plugin:demo:", // 空动作 id
            "plugin",       // 缺分隔符
            "",             // 空
        ] {
            assert!(
                parse_plugin_menu_id(id).is_none(),
                "{id:?} 不该被当成插件菜单项"
            );
        }
    }

    /// 应用自己的菜单项 id 不能意外命中插件前缀。
    #[test]
    fn builtin_menu_ids_do_not_look_like_plugin_items() {
        for id in [M_SEND, M_RECV, M_SCRIPTS, M_SETTINGS, M_QUIT, M_STATUS] {
            assert!(!id.starts_with(PLUGIN_MENU_PREFIX), "{id} 误用了插件前缀");
            assert!(parse_plugin_menu_id(id).is_none());
        }
    }
}
