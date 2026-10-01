//! `$.format_duration` / `$.format_bytes`：日志与提示用的紧凑格式化。
//!
//! 这两个函数在脚本里被反复手写（`04-file-transfer.ts` 与 `03-pack-and-shard.ts`
//! 各有一份同样的时长拼接），因此收进能力：
//!
//! * 少一处重复，少一类「两个脚本显示不一致」的漂移；
//! * 单位换算只有一处实现，不会一边按 1024 一边按 1000。
//!
//! 两者都是**纯计算**，与宿主无关，CLI / headless 下同样可用。

use rquickjs::{Ctx, Result as QjsResult};

use crate::extensions::extension;

extension! {
    /// `$.format_duration` / `$.format_bytes`。
    pub struct FormatExtension;
    "format_duration" => js_format_duration,
        "format_duration(seconds: number) -> string",
        "秒 → 紧凑时长（`45s` / `1m23s` / `1h2m3s`）；**单位是秒**，毫秒请先 /1000";
    "format_bytes" => js_format_bytes,
        "format_bytes(bytes: number) -> string",
        "字节 → 紧凑大小（1024 进制：`512B` / `1.5KB` / `2.00MB` / `1.50GB`）";
}

/// `format_duration(seconds) -> string`
///
/// 向下取整到秒。三个分段省略为零的高位：`45s`、`1m23s`、`1h2m3s`。
/// 负数与非有限值（NaN / Infinity）一律返回 `"0s"` —— 这个能力只用于展示，
/// 不该因为一个意外值把脚本抛死。
fn js_format_duration<'js>(_ctx: Ctx<'js>, seconds: f64) -> QjsResult<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Ok("0s".to_string());
    }
    let total = seconds.floor() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    Ok(if h == 0 && m == 0 {
        format!("{s}s")
    } else if h == 0 {
        format!("{m}m{s}s")
    } else {
        format!("{h}h{m}m{s}s")
    })
}

/// `format_bytes(bytes) -> string`
///
/// 1024 进制，与 Rust 侧既有的 `fmt_bytes` 写法保持一致（B / KB 一位小数 /
/// MB 与 GB 两位小数），这样同一个大小在脚本日志与进度窗口里显示一样。
/// 负数与非有限值返回 `"0B"`。
fn js_format_bytes<'js>(_ctx: Ctx<'js>, bytes: f64) -> QjsResult<String> {
    if !bytes.is_finite() || bytes <= 0.0 {
        return Ok("0B".to_string());
    }
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    Ok(if bytes < KB {
        format!("{}B", bytes.floor() as u64)
    } else if bytes < MB {
        format!("{:.1}KB", bytes / KB)
    } else if bytes < GB {
        format!("{:.2}MB", bytes / MB)
    } else {
        format!("{:.2}GB", bytes / GB)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::Context;

    fn with_ctx<T>(f: impl FnOnce(Ctx<'_>) -> T) -> T {
        let ctx = Context::full(&rquickjs::Runtime::new().unwrap()).unwrap();
        ctx.with(f)
    }

    #[test]
    fn duration_formats() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0s"),
            (1.0, "1s"),
            (45.0, "45s"),
            (59.9, "59s"),
            (60.0, "1m0s"),
            (83.0, "1m23s"),
            (3600.0, "1h0m0s"),
            (3661.0, "1h1m1s"),
            (-5.0, "0s"),
            (f64::NAN, "0s"),
            (f64::INFINITY, "0s"),
        ];
        with_ctx(|ctx| {
            for &(input, want) in cases {
                assert_eq!(
                    js_format_duration(ctx.clone(), input).unwrap(),
                    want,
                    "format_duration({input})"
                );
            }
        });
    }

    #[test]
    fn bytes_formats() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0B"),
            (512.0, "512B"),
            (1023.0, "1023B"),
            (1024.0, "1.0KB"),
            (1536.0, "1.5KB"),
            (1048576.0, "1.00MB"),
            (1610612736.0, "1.50GB"),
            (-1.0, "0B"),
            (f64::NAN, "0B"),
        ];
        with_ctx(|ctx| {
            for &(input, want) in cases {
                assert_eq!(
                    js_format_bytes(ctx.clone(), input).unwrap(),
                    want,
                    "format_bytes({input})"
                );
            }
        });
    }
}
