//! Tauri 2 构建脚本:
//! 1. 调用 tauri_build::build() 处理 Tauri 资源/上下文。
//! 2. 代码生成图标(深色圆角底 + 白色 K 光束 + 青色光点),免二进制资产入库:
//!    - 32x32 / 128x128 / 1024x1024 PNG(供 Tauri 打包 + 托盘 + 窗口图标)
//! 3. client-vanilla 源码变化时自动执行 pnpm 构建(deploy.rs 用 include_str!
//!    内嵌 packages/client-vanilla/dist/index.html)。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const BASE: i32 = 64;

#[derive(Clone, Copy)]
struct Rgba(u8, u8, u8, u8);

const BG: Rgba = Rgba(0x14, 0x1a, 0x2b, 255);
const FG: Rgba = Rgba(0xe8, 0xec, 0xf4, 255);
const ACCENT: Rgba = Rgba(0x5b, 0x8c, 0xff, 255);

/// 以 BASE=64 逻辑网格渲染图标,scale 为整数放大倍数(最近邻风格几何,
/// 线条粗细随 scale 加粗,保证高分下图标锐利)。
fn render(scale: i32) -> Vec<u8> {
    let w = BASE * scale;
    let h = BASE * scale;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let put = |buf: &mut [u8], x: i32, y: i32, c: Rgba| {
        if (0..w).contains(&x) && (0..h).contains(&y) {
            let i = (y * w + x) as usize * 4;
            let a = c.3 as u16;
            for (k, col) in [c.0, c.1, c.2, c.3].iter().enumerate() {
                let dst = buf[i + k] as u16;
                buf[i + k] = (((dst * (255 - a) + *col as u16 * a) / 255) & 0xff) as u8;
            }
        }
    };

    // 圆角矩形底(逻辑半径 13)
    let radius = 13 * scale;
    for y in 0..h {
        for x in 0..w {
            let cx = (x - radius).max(0).min(w - 1 - radius);
            let cy = (y - radius).max(0).min(h - 1 - radius);
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= radius * radius {
                put(&mut buf, x, y, BG);
            }
        }
    }

    // 画线(沿 Bresenham 路径盖圆盘),坐标与粗细均按 scale 放大
    let line = |buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, t: i32, c: Rgba| {
        let (x0, y0, x1, y1, t) = (x0 * scale, y0 * scale, x1 * scale, y1 * scale, t * scale);
        let mut plot = |x: i32, y: i32| {
            let r2 = t * t;
            for dy in -t..=t {
                for dx in -t..=t {
                    if dx * dx + dy * dy <= r2 {
                        put(buf, x + dx, y + dy, c);
                    }
                }
            }
        };
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            plot(x, y);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    };

    // K:竖杠 + 两条对角线
    line(&mut buf, 19, 15, 19, 49, 4, FG);
    line(&mut buf, 23, 32, 44, 15, 4, FG);
    line(&mut buf, 23, 32, 44, 49, 4, FG);
    // 青色光点(光的方向)
    line(&mut buf, 47, 15, 52, 12, 2, ACCENT);

    buf
}

/// 把 RGBA 缓冲编码为 PNG 字节。
fn encode_png(buf: &[u8], size: u32) -> Vec<u8> {
    let mut png_bytes = Vec::new();
    {
        use png::Encoder;
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        let mut enc = Encoder::new(&mut cursor, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(buf).unwrap();
    }
    png_bytes
}

/// 内容与现有文件一致时跳过写入。
/// 关键:build.rs 每次运行都会执行,若无条件 fs::write 会刷新图标 mtime,
/// tauri dev 的文件监视器检测到 icons/ 变化 → 触发重建 → build.rs 再写 →
/// 无限重建循环。内容未变时不更新 mtime 即可断开循环。
fn write_png_if_changed(path: &std::path::Path, buf: &[u8], size: u32) {
    let png_bytes = encode_png(buf, size);
    if let Ok(existing) = std::fs::read(path) {
        if existing == png_bytes {
            return;
        }
    }
    std::fs::write(path, &png_bytes).unwrap();
}

/// 递归收集目录下所有文件的最新 mtime。
fn newest_mtime(dir: &Path, latest: &mut Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            newest_mtime(&path, latest);
        } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            *latest = Some(latest.map_or(modified, |t| t.max(modified)));
        }
    }
}

/// 单文件计入最新 mtime。
fn touch_mtime(path: &Path, latest: &mut Option<SystemTime>) {
    if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
        *latest = Some(latest.map_or(modified, |t| t.max(modified)));
    }
}

/// 构建内嵌接收页(client-vanilla 的 singlefile 产物)。
/// 仅当 dist 缺失或早于任一源文件时执行;不声明对 dist 的 rerun-if-changed,
/// 避免「构建刷新 dist mtime → 触发 build.rs → 再构建」的循环。
/// 设置 CLIPBEAM_SKIP_WEB_BUILD=1 可跳过(需自行保证 dist/index.html 存在)。
fn build_web_client(manifest_dir: &Path) {
    let root = manifest_dir.join("..");
    let pkg = root.join("packages/client-vanilla");
    let dist_html = pkg.join("dist/index.html");

    let mut src_newest: Option<SystemTime> = None;
    newest_mtime(&pkg.join("src"), &mut src_newest);
    newest_mtime(&root.join("packages/shared/src"), &mut src_newest);
    touch_mtime(&pkg.join("index.html"), &mut src_newest);
    touch_mtime(&pkg.join("vite.config.ts"), &mut src_newest);

    let dist_time = std::fs::metadata(&dist_html)
        .ok()
        .and_then(|m| m.modified().ok());
    let stale = match (src_newest, dist_time) {
        (Some(src), Some(dist)) => src > dist,
        (Some(_), None) => true,
        // 源目录整体缺失时不做判断,交给后续 include_str! 报错
        (None, _) => false,
    };
    if !stale {
        return;
    }

    if matches!(std::env::var("CLIPBEAM_SKIP_WEB_BUILD"), Ok(v) if v == "1") {
        println!("cargo:warning=client-vanilla 产物已过期，但 CLIPBEAM_SKIP_WEB_BUILD=1，跳过自动构建");
        return;
    }

    println!("cargo:warning=client-vanilla 构建中(pnpm --filter @clipbeam/client-vanilla build)...");
    let status = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "pnpm", "--filter", "@clipbeam/client-vanilla", "build"])
            .current_dir(&root)
            .status()
    } else {
        Command::new("pnpm")
            .args(["--filter", "@clipbeam/client-vanilla", "build"])
            .current_dir(&root)
            .status()
    };

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!(
            "client-vanilla 构建失败(exit {s:?})。请手动执行 `pnpm --filter @clipbeam/client-vanilla build`，\
             或设置 CLIPBEAM_SKIP_WEB_BUILD=1 跳过(需自行保证 dist/index.html 存在)"
        ),
        Err(e) => panic!(
            "无法启动 pnpm 构建 client-vanilla: {e}。请确认 pnpm 已安装并在 PATH 中，\
             或手动执行 `pnpm --filter @clipbeam/client-vanilla build` 后重试"
        ),
    }
}

fn main() {
    // 1. Tauri 2 上下文/资源构建
    tauri_build::build();

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // 2. 源码变化时自动构建 client-vanilla(deploy.rs include_str! 其 dist 产物)
    println!("cargo:rerun-if-changed=../packages/client-vanilla/src");
    println!("cargo:rerun-if-changed=../packages/client-vanilla/index.html");
    println!("cargo:rerun-if-changed=../packages/client-vanilla/vite.config.ts");
    println!("cargo:rerun-if-changed=../packages/shared/src");
    println!("cargo:rerun-if-changed=build.rs");
    build_web_client(&manifest_dir);

    // 3. 生成图标 PNG 到 icons/ 目录(供 Tauri 打包 + 运行时托盘/窗口使用)
    let icons_dir = manifest_dir.join("icons");
    std::fs::create_dir_all(&icons_dir).unwrap();

    write_png_if_changed(&icons_dir.join("32x32.png"), &render(1), (BASE * 1) as u32);
    write_png_if_changed(
        &icons_dir.join("128x128.png"),
        &render(2),
        (BASE * 2) as u32,
    );
    write_png_if_changed(&icons_dir.join("icon.png"), &render(16), (BASE * 16) as u32);
}
