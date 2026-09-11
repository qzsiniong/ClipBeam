//! 构建脚本：
//! 1) 代码生成 64x64 RGBA 托盘/窗口图标（深色圆角底 + 白色 K 光束 + 青色光点），免二进制资产入库；
//! 2) web/keybeam.html 变化时触发重编译（deploy.rs 用 include_str! 内嵌）。

use std::path::PathBuf;

const W: usize = 64;
const H: usize = 64;

#[derive(Clone, Copy)]
struct Rgba(u8, u8, u8, u8);

const BG: Rgba = Rgba(0x14, 0x1a, 0x2b, 255);
const FG: Rgba = Rgba(0xe8, 0xec, 0xf4, 255);
const ACCENT: Rgba = Rgba(0x5b, 0x8c, 0xff, 255);

fn main() {
    println!("cargo:rerun-if-changed=web/keybeam.html");
    println!("cargo:rerun-if-changed=build.rs");

    let mut buf = vec![0u8; W * H * 4];
    let put = |buf: &mut [u8], x: i32, y: i32, c: Rgba| {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            let i = (y as usize * W + x as usize) * 4;
            // 简单线性混合（accent/fg 均为不透明）
            let a = c.3 as u16;
            for (k, col) in [c.0, c.1, c.2, c.3].iter().enumerate() {
                let dst = buf[i + k] as u16;
                buf[i + k] = (((dst * (255 - a) + *col as u16 * a) / 255) & 0xff) as u8;
            }
        }
    };

    // 圆角矩形底（半径 13）
    let radius = 13i32;
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let cx = (x - radius).max(0).min(W as i32 - 1 - radius);
            let cy = (y - radius).max(0).min(H as i32 - 1 - radius);
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= radius * radius {
                put(&mut buf, x, y, BG);
            }
        }
    }

    // 粗线（沿 Bresenham 路径盖圆盘）
    let line = |buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, t: i32, c: Rgba| {
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

    // K：竖杠 + 两条对角线
    line(&mut buf, 19, 15, 19, 49, 4, FG);
    line(&mut buf, 23, 32, 44, 15, 4, FG);
    line(&mut buf, 23, 32, 44, 49, 4, FG);
    // 青色光点（光的方向）
    line(&mut buf, 47, 15, 52, 12, 2, ACCENT);

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("keybeam_icon.rgba");
    std::fs::write(out, &buf).unwrap();
}
