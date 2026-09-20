//! 引擎补齐的标准全局：`atob`/`btoa`、定时器、`performance`、`structuredClone`。
//!
//! 这些不依赖任何外部能力，属于「浏览器/Node 都有、缺了脚本作者会踩空」的东西。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use script_engine::{ConsoleHook, RuntimeOptions, ScriptRuntime};

/// 收集 `console.*`（定时器回调抛错会写到这里）。
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

async fn runtime() -> ScriptRuntime {
    ScriptRuntime::new().await.expect("创建运行时失败")
}

/// `btoa` 与 `atob` 是彼此的逆运算，且只处理 Latin-1。
#[tokio::test]
async fn atob_and_btoa_roundtrip() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const encoded = btoa("abc");
            [encoded, atob(encoded), atob("YQ=="), atob("YQ")].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    // 填充按规范省略也应当能解出来
    assert_eq!(value, "YWJj|abc|a|a");
}

/// `btoa` 对超过 0xFF 的码位抛 `TypeError`（与浏览器一致），并提示用 `$.base64`。
#[tokio::test]
async fn btoa_rejects_non_latin1() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                btoa("中文");
                "没有抛错"
            } catch (err) {
                [err.name, err.message.includes("base64")].join("|")
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "TypeError|true");
}

/// `atob` 对非法字符抛异常。
#[tokio::test]
async fn atob_rejects_invalid_characters() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                atob("Y!Q=");
                "没有抛错"
            } catch (err) {
                err.name
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "TypeError");
}

/// `performance.now()` 是单调递增的毫秒数。
#[tokio::test]
async fn performance_now_is_monotonic() {
    let runtime = runtime().await;

    let value: bool = runtime
        .eval(
            r#"
            const a = performance.now();
            // 忙等一点点，确保时间前进
            for (let i = 0; i < 200000; i++) { Math.sqrt(i); }
            const b = performance.now();
            typeof a === "number" && typeof b === "number" && b >= a && a >= 0
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(value, "performance.now() 应当是单调不减的数字");
}

/// `structuredClone` 做深拷贝（改副本不影响原对象）。
#[tokio::test]
async fn structured_clone_is_deep() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const original = { a: 1, nested: { list: [1, 2, 3] } };
            const copy = structuredClone(original);
            copy.nested.list.push(4);
            [original.nested.list.length, copy.nested.list.length].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "3|4");
}

/// 不支持的类型（`Date` / 循环引用）明确抛错，而不是静默给出错值。
#[tokio::test]
async fn structured_clone_rejects_unsupported_values() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const results = [];
            try { structuredClone(new Date()); results.push("date:没有抛错"); }
            catch (err) { results.push("date:" + err.message.includes("Date")); }

            const cyclic = {}; cyclic.self = cyclic;
            try { structuredClone(cyclic); results.push("cyclic:没有抛错"); }
            catch (err) { results.push("cyclic:true"); }
            results.join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "date:true|cyclic:true");
}

/// `setTimeout` 延迟执行，并在脚本结束前把回调跑完。
#[tokio::test]
async fn set_timeout_runs_callback() {
    let runtime = runtime().await;
    let started = Instant::now();

    let value: String = runtime
        .eval(
            r#"
            await new Promise((resolve) => {
                setTimeout(() => resolve("done"), 80);
            });
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "done");
    assert!(
        started.elapsed() >= Duration::from_millis(70),
        "应当真的等过：{:?}",
        started.elapsed()
    );
}

/// `clearTimeout` 取消后回调不再执行。
#[tokio::test]
async fn clear_timeout_cancels() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            let fired = false;
            const id = setTimeout(() => { fired = true; }, 50);
            clearTimeout(id);
            await new Promise((resolve) => setTimeout(resolve, 120));
            String(fired)
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "false", "被 clearTimeout 取消的回调不应执行");
}

/// `setInterval` 周期执行，`clearInterval` 之后停下。
#[tokio::test]
async fn set_interval_repeats_until_cleared() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            let count = 0;
            const id = setInterval(() => { count += 1; }, 30);
            await new Promise((resolve) => setTimeout(resolve, 130));
            clearInterval(id);
            const afterClear = count;
            await new Promise((resolve) => setTimeout(resolve, 80));
            // 至少跑了两次，且 clear 之后不再增长
            `${afterClear >= 2}|${count === afterClear}`
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "true|true");
}

/// 定时器回调抛错只写进 ConsoleHook，不打断脚本。
#[tokio::test]
async fn timer_callback_error_goes_to_console() {
    let console = Arc::new(TestConsole::default());
    let runtime = ScriptRuntime::with_options(RuntimeOptions::default().console(console.clone()))
        .await
        .expect("创建运行时失败");

    let value: String = runtime
        .eval(
            r#"
            setTimeout(() => { throw new Error("定时器炸了"); }, 20);
            await new Promise((resolve) => setTimeout(resolve, 80));
            "脚本继续跑完了"
            "#,
        )
        .await
        .expect("回调抛错不该让脚本失败");

    assert_eq!(value, "脚本继续跑完了");
    let lines = console.lines();
    assert!(
        lines
            .iter()
            .any(|(level, text)| level == "error" && text.contains("定时器炸了")),
        "回调异常应当写进 ConsoleHook：{lines:?}"
    );
}

/// 定时器只属于本次运行：上一次脚本的定时器不会打到下一次。
#[tokio::test]
async fn timers_do_not_leak_between_runs() {
    let runtime = runtime().await;

    // 第一次运行留下一个较晚触发的 interval（脚本自己不等待它）
    runtime
        .eval::<()>(
            r#"
            globalThis.leaked = 0;
            setInterval(() => { globalThis.leaked += 1; }, 10);
            "#,
        )
        .await
        .expect("第一次运行失败");

    // 第二次运行前应当已被清理：等一段远超过间隔的时间，计数仍为 0
    let value: String = runtime
        .eval(
            r#"
            await new Promise((resolve) => setTimeout(resolve, 80));
            String(globalThis.leaked ?? 0)
            "#,
        )
        .await
        .expect("第二次运行失败");

    assert_eq!(value, "0", "上一次运行的定时器不应继续触发");
}
