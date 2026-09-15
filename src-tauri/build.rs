//! Tauri 2 构建脚本:
//! 1. 调用 tauri_build::build() 处理 Tauri 资源/上下文。
//! 2. 代码生成图标(深色圆角底 + 白色 K 光束 + 青色光点),免二进制资产入库:
//!    - 32x32 / 128x128 / 1024x1024 PNG(供 Tauri 打包 + 托盘 + 窗口图标)
//! 3. ../web/clipbeam.html 变化时触发重编译(deploy.rs 用 include_str! 内嵌)。

use std::path::PathBuf;

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

/// 把 RGBA 缓冲写成 PNG 文件。
fn write_png(path: &std::path::Path, buf: &[u8], size: u32) {
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
    std::fs::write(path, &png_bytes).unwrap();
}

fn main() {
    // 1. Tauri 2 上下文/资源构建
    tauri_build::build();

    // 2. 触发 ../web/clipbeam.html 变化时重编译(deploy.rs include_str! 内嵌)
    println!("cargo:rerun-if-changed=../web/clipbeam.html");
    println!("cargo:rerun-if-changed=build.rs");

    // 3. 生成图标 PNG 到 icons/ 目录(供 Tauri 打包 + 运行时托盘/窗口使用)
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icons_dir = manifest_dir.join("icons");
    std::fs::create_dir_all(&icons_dir).unwrap();

    write_png(&icons_dir.join("32x32.png"), &render(1), (BASE * 1) as u32);
    write_png(&icons_dir.join("128x128.png"), &render(2), (BASE * 2) as u32);
    write_png(&icons_dir.join("icon.png"), &render(16), (BASE * 16) as u32);
}
