//! 引擎自带设施的集成测试：`sleep` / `console` / `TextDecoder` / `TextEncoder` / 标准全局。
//!
//! 这里刻意只依赖引擎自带的公开 API，不配置命名空间、不引入任何使用方扩展 ——
//! 用来证明「引擎的最小集」是自洽的（连 `$` 都不存在）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use script_engine::{CancelSignal, ConsoleHook, RuntimeOptions, ScriptRuntime};

/// 测试控制台钩子：把 `console.*` 收进内存，便于断言。
#[derive(Default)]
struct TestConsole {
    lines: Mutex<Vec<(String, String)>>,
}

impl TestConsole {
    fn lines(&self) -> Vec<(String, String)> {
        self.lines.lock().unwrap().clone()
    }
}

impl ConsoleHook for TestConsole {
    fn write(&self, level: &str, text: &str) {
        self.lines
            .lock()
            .unwrap()
            .push((level.to_string(), text.to_string()));
    }
}

/// 建一个装了默认配置的运行时。
async fn runtime() -> ScriptRuntime {
    ScriptRuntime::new().await.expect("创建运行时失败")
}

/// 顶层 await 可用，`sleep` 真的等待（时间下界断言）。
#[tokio::test]
async fn top_level_await_and_sleep_waits() {
    let runtime = runtime().await;

    let started = Instant::now();
    runtime
        .eval::<()>(
            r#"
            await sleep(120);
            await sleep(60);
            "#,
        )
        .await
        .expect("脚本执行失败");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(180),
        "两次 sleep 应该至少等 180ms，实际 {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "sleep 不该明显超时：{elapsed:?}"
    );
}

/// 取消令牌能打断长睡眠：不应等到 3 秒。
#[tokio::test]
async fn sleep_is_interruptible_by_cancel() {
    let token = CancelSignal::new();
    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .script_name("interruptible.js")
            .cancel(token.clone()),
    )
    .await
    .expect("创建运行时失败");

    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        token.cancel();
    });

    let started = Instant::now();
    runtime
        .eval::<()>("await sleep(3000); 'done'")
        .await
        .expect("取消不该让脚本报错");
    let elapsed = started.elapsed();
    canceller.await.unwrap();

    assert!(
        elapsed < Duration::from_millis(1500),
        "取消后应在 1.5s 内返回，实际 {elapsed:?}"
    );
}

/// `Promise.all` 里的多个 `sleep` 应当并行，而不是串行等待。
#[tokio::test]
async fn sleeps_in_promise_all_are_concurrent() {
    let runtime = runtime().await;

    let started = Instant::now();
    runtime
        .eval::<()>(
            r#"
            await Promise.all([sleep(150), sleep(150), sleep(150)]);
            "#,
        )
        .await
        .expect("脚本执行失败");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(400),
        "三个 150ms 的 sleep 并行应远小于 450ms，实际 {elapsed:?}"
    );
}

/// `console` 五个方法都存在，格式化不会抛（含循环引用）。
#[tokio::test]
async fn console_is_available_and_never_throws() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const methods = ["log", "info", "debug", "warn", "error"];
            const allFunctions = methods.every((name) => typeof console[name] === "function");
            const cyclic = {};
            cyclic.self = cyclic;
            console.log("console 测试", { a: 1 }, [1, 2], cyclic, 42);
            console.warn("警告");
            console.error("错误");
            String(allFunctions)
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "true");
}

/// UTF-8 往返：含中文与 astral 字符（emoji）。
#[tokio::test]
async fn utf8_roundtrip_with_cjk_and_astral() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const enc = new TextEncoder();
            const dec = new TextDecoder();
            const text = "中文 abc 🚀 é";
            const bytes = enc.encode(text);
            [enc.encoding, dec.encoding, bytes.length, dec.decode(bytes), dec.decode(bytes.buffer)].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    let text = "中文 abc 🚀 é";
    assert_eq!(
        value,
        format!("utf-8|utf-8|{}|{}|{}", text.len(), text, text)
    );
}

/// 非 UTF-8 编码（GBK）可用 —— 这是 core 引入 encoding_rs 的主要目的。
#[tokio::test]
async fn decodes_non_utf8_encodings() {
    // "中文" 的 GBK 编码
    let gbk_bytes: &[u8] = &[0xD6, 0xD0, 0xCE, 0xC4];
    let runtime = runtime().await;

    let value: String = runtime
        .eval(&format!(
            r#"
            const bytes = new Uint8Array([{list}]).buffer;
            const dec = new TextDecoder("GBK");
            [dec.encoding, dec.decode(bytes)].join("|")
            "#,
            list = gbk_bytes
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "gbk|中文");
}

/// 未知编码标签在**构造时**抛 `RangeError`（符合 TextDecoder 规范）。
#[tokio::test]
async fn unknown_label_throws_range_error() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                new TextDecoder("no-such-encoding");
                "没有抛错"
            } catch (err) {
                [err.name, err instanceof RangeError].join("|")
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "RangeError|true");
}

/// `fatal: true` 遇到非法 UTF-8 抛 `TypeError`；默认行为是替换成 U+FFFD。
#[tokio::test]
async fn fatal_mode_throws_on_invalid_utf8() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const bad = new Uint8Array([0x41, 0xc3, 0x28]).buffer;
            const lenient = new TextDecoder().decode(bad);
            let fatalName = "没有抛错";
            try {
                new TextDecoder("utf-8", { fatal: true }).decode(bad);
            } catch (err) {
                fatalName = [err.name, err instanceof TypeError].join(":");
            }
            [lenient, fatalName].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    let (lenient, fatal) = value.split_once('|').expect("结果格式不对");
    assert_eq!(lenient, "A\u{FFFD}(");
    assert_eq!(fatal, "TypeError:true");
}

/// `ignoreBOM` 语义：默认去掉开头的 BOM，显式传 true 时保留。
#[tokio::test]
async fn bom_handling_respects_ignore_bom() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const withBom = new Uint8Array([0xef, 0xbb, 0xbf, 0x41]).buffer;
            const stripped = new TextDecoder().decode(withBom);
            const kept = new TextDecoder("utf-8", { ignoreBOM: true }).decode(withBom);
            [stripped, kept.length, kept.charCodeAt(0) === 0xfeff].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "A|2|true");
}

/// `decode()` 接受各种 BufferSource：`ArrayBuffer` / `Uint8Array`（含偏移视图）/ `DataView`。
#[tokio::test]
async fn decode_accepts_buffer_sources() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const bytes = new Uint8Array([0x78, 0x41, 0x42, 0x43, 0x79]); // xABCy
            const view = new Uint8Array(bytes.buffer, 1, 3);              // ABC
            const dataView = new DataView(bytes.buffer, 1, 3);            // ABC
            const dec = new TextDecoder();
            [dec.decode(), dec.decode(bytes.buffer), dec.decode(view), dec.decode(dataView)].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "|xABCy|ABC|ABC");
}

/// 传错类型要抛 `TypeError`（而不是静默返回空串）。
#[tokio::test]
async fn decode_rejects_non_buffer_source() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                new TextDecoder().decode("not a buffer");
                "没有抛错";
            } catch (err) {
                [err.name, err instanceof TypeError].join("|");
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "TypeError|true");
}

/// 脚本抛异常时，错误信息应包含消息与脚本名。
#[tokio::test]
async fn script_error_is_reported_with_location() {
    let runtime = runtime().await;

    let err = runtime
        .run_named_script("boom.js", "throw new Error('炸了')")
        .await
        .expect_err("脚本抛异常时应当返回 Err");

    let text = err.to_string();
    assert!(text.contains("炸了"), "应含异常消息：{text}");
    assert!(text.contains("boom.js"), "应含脚本名：{text}");
}

/// 引擎裸跑时的全貌：只有标准全局，没有命名空间、没有内部原语残留。
///
/// 这是分层成立的证据 —— 业务能力与命名空间名字都要使用方来加。
#[tokio::test]
async fn engine_alone_exposes_only_standard_globals() {
    let runtime = runtime().await;
    let value: String = runtime
        .eval(
            r#"
            [
              typeof sleep,
              typeof MyTool,
              typeof $,
              typeof __primitives,
              typeof setTimeout,
              typeof setInterval,
              typeof atob,
              typeof btoa,
              typeof structuredClone,
              typeof performance.now,
              typeof TextDecoder,
              typeof TextEncoder,
              typeof console.log,
            ].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");
    assert_eq!(
        value,
        "function|undefined|undefined|undefined|function|function|function|function|function|function|function|function|function",
        "引擎只该提供标准全局：命名空间由使用方配置，内部原语用完即删"
    );
}

/// `console.*` 的输出交给 [`ConsoleHook`]，不写进程标准流。
#[tokio::test]
async fn console_reaches_hook_and_falls_back_to_stdout() {
    let console = Arc::new(TestConsole::default());
    let runtime = ScriptRuntime::with_options(RuntimeOptions::default().console(console.clone()))
        .await
        .expect("创建运行时失败");

    runtime
        .eval::<()>(r#"console.log("一"); console.warn("二");"#)
        .await
        .expect("脚本执行失败");

    assert_eq!(
        console.lines(),
        vec![
            ("log".to_string(), "一".to_string()),
            ("warn".to_string(), "二".to_string())
        ]
    );
}
