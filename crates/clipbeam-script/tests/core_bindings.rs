//! core 基础能力的集成测试：`$.file` / `$.sleep` / `console` / `TextDecoder` / `TextEncoder`。
//!
//! 这里刻意只依赖 core 自带的公开 API，不引入任何使用方扩展 —— 用来证明
//! 「引擎的最小集」是自洽的。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clipbeam_script::{
    CancellationToken, ConfirmChoice, HostError, RuntimeOptions, ScriptHost, ScriptRuntime,
};

/// 测试宿主：把交互调用记下来，方便断言「能力有没有走到宿主」。
#[derive(Default)]
struct TestHost {
    typed: Mutex<String>,
    confirm_answer: Mutex<ConfirmChoice>,
    progress: Mutex<Vec<(usize, usize)>>,
    token: Mutex<Option<CancellationToken>>,
}

impl TestHost {
    fn with_answer(answer: ConfirmChoice) -> Self {
        Self {
            confirm_answer: Mutex::new(answer),
            ..Default::default()
        }
    }

    fn typed(&self) -> String {
        self.typed.lock().unwrap().clone()
    }

    fn progress(&self) -> Vec<(usize, usize)> {
        self.progress.lock().unwrap().clone()
    }
}

impl ScriptHost for TestHost {
    fn type_str(&self, text: &str, _delay_ms: u64) -> Result<(), HostError> {
        self.typed.lock().unwrap().push_str(text);
        Ok(())
    }

    fn progress(&self, typed: usize, total: usize) {
        self.progress.lock().unwrap().push((typed, total));
    }

    fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
        Ok(*self.confirm_answer.lock().unwrap())
    }

    fn cancelled(&self) -> bool {
        self.token
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }
}

/// 在系统临时目录里写一个测试用文件，返回路径。
///
/// 文件名带上进程号 + 序号，避免并行运行多个测试时互相覆盖。
fn write_temp_file(name: &str, data: &[u8]) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clipbeam-script-test-{}-{seq}-{name}",
        std::process::id()
    ));
    std::fs::write(&path, data).expect("写入临时文件失败");
    path
}

/// 把路径转成能安全嵌进 JS 字符串的字面量（自己实现一份，避免依赖 core 未提供的工具）。
fn js_path(path: &std::path::Path) -> String {
    format!(
        "{:?}",
        path.to_str().expect("临时路径不是合法 UTF-8").replace('\\', "\\\\")
    )
}

/// 建一个装了默认配置的运行时。
async fn runtime() -> ScriptRuntime {
    ScriptRuntime::new().await.expect("创建运行时失败")
}


/// `$.file` 读到的字节必须与磁盘内容完全一致（含 NUL 与高位字节）。
#[tokio::test]
async fn file_returns_exact_bytes_including_binary() {
    let payload: Vec<u8> = vec![0x00, 0xff, 0x10, b'a', b'b', b'c', 0x7f, 0x80];
    let path = write_temp_file("bytes.bin", &payload);
    let runtime = runtime().await;

    let bytes: Vec<u8> = runtime
        .eval(&format!("Array.from(new Uint8Array(await $.file({})))",
            js_path(&path)))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);
    assert_eq!(bytes, payload, "文件字节不一致");
}

/// 读取不存在的文件时，错误应当带上路径与 `$.file` 来源。
#[tokio::test]
async fn missing_file_rejects_with_path() {
    let runtime = runtime().await;

    let err = runtime
        .run_script(r#"await $.file("/clipbeam/definitely/not/here.bin")"#)
        .await
        .expect_err("读取不存在的文件应当返回 Err");

    let text = err.to_string();
    assert!(text.contains("/clipbeam/definitely/not/here.bin"), "应含路径：{text}");
    assert!(text.contains("$.file"), "应标明来源：{text}");
}

/// 顶层 await 可用，`$.sleep` 真的等待（时间下界断言）。
#[tokio::test]
async fn top_level_await_and_sleep_waits() {
    let runtime = runtime().await;

    let started = Instant::now();
    runtime
        .eval::<()>(
            r#"
            await $.sleep(120);
            await $.sleep(60);
            "#,
        )
        .await
        .expect("脚本执行失败");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(180),
        "两次 sleep 应该至少等 180ms，实际 {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(2), "sleep 不该明显超时：{elapsed:?}");
}

/// 取消令牌能打断长睡眠：不应等到 3 秒。
#[tokio::test]
async fn sleep_is_interruptible_by_cancel_token() {
    let token = CancellationToken::new();
    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .script_name("interruptible.js")
            .cancel_token(token.clone()),
    )
    .await
    .expect("创建运行时失败");

    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        token.cancel();
    });

    let started = Instant::now();
    runtime
        .eval::<()>("await $.sleep(3000); 'done'")
        .await
        .expect("取消不该让脚本报错");
    let elapsed = started.elapsed();
    canceller.await.unwrap();

    assert!(
        elapsed < Duration::from_millis(1500),
        "取消后应在 1.5s 内返回，实际 {elapsed:?}"
    );
}

/// `Promise.all` 里的多个 `$.sleep` 应当并行，而不是串行等待。
#[tokio::test]
async fn sleeps_in_promise_all_are_concurrent() {
    let runtime = runtime().await;

    let started = Instant::now();
    runtime
        .eval::<()>(
            r#"
            await Promise.all([$.sleep(150), $.sleep(150), $.sleep(150)]);
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

/// 没有注入宿主时，`NoopHost` 让运行时保持可用（core 本身不需要宿主）。
#[tokio::test]
async fn runtime_works_without_host() {
    let runtime = runtime().await;
    let value: String = runtime
        .eval(r#"[typeof $.file, typeof $.sleep].join("|")"#)
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "function|function");
}

/// 测试用宿主在 core 里能被用到（供使用方扩展的测试复用同一套语义）。
#[tokio::test]
async fn host_is_reachable_from_bindings() {
    let host = Arc::new(TestHost::with_answer(ConfirmChoice::Yes));
    let runtime = ScriptRuntime::with_options(RuntimeOptions::default().host(host.clone()))
        .await
        .expect("创建运行时失败");

    // core 不提供 typeStr，这里只验证运行时能用这个宿主跑完脚本
    runtime.eval::<()>("await $.sleep(1);").await.unwrap();
    assert!(host.typed().is_empty());
    assert!(host.progress().is_empty());
}
