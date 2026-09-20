//! 扩展机制的集成测试：注册、签名声明、重名冲突、宿主可达性。
//!
//! 这里用一个**测试扩展**（`CaptureExtension`）证明「使用方能自由注入能力」这件事真的成立：
//! 扩展把每次调用转交给测试宿主，于是可以断言 JS → Rust 的完整链路。

use std::sync::{Arc, Mutex};

use rquickjs::{Ctx, Function, Object, Result as QjsResult};
use script_engine::{
    CancelSignal, CapabilitySpec, ConsoleHook, RuntimeOptions, ScriptExtension, ScriptRuntime,
};

/// 测试控制台钩子：把 `console.*` 收进内存，便于断言。
#[derive(Default)]
struct TestConsole {
    /// 收到的 `console.*` 输出（level, text）。
    lines: Mutex<Vec<(String, String)>>,
}

impl ConsoleHook for TestConsole {
    fn write(&self, level: &str, text: &str) {
        self.lines
            .lock()
            .unwrap()
            .push((level.to_string(), text.to_string()));
    }
}

/// 测试扩展：`$.capture(text)` —— 把文本交给宿主，用于验证「扩展能拿到宿主」。
struct CaptureExtension;

impl ScriptExtension for CaptureExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("capture", Function::new(ctx.clone(), js_capture)?)?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "capture",
            signature: "capture(text: string) -> void",
            doc: "测试用：把文本交给宿主",
        }]
    }
}

/// 测试扩展：`$.capture(text)` —— 把文本写进控制台钩子。
///
/// 用来证明「扩展能在 Rust 侧访问引擎提供的设施」（这里是 ConsoleHook），
/// 而不需要引擎认识任何宿主概念。
fn js_capture<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<()> {
    let Some(console) = script_engine::bindings::console_hook(&ctx) else {
        return Err(rquickjs::Exception::throw_message(&ctx, "没有控制台钩子"));
    };
    console.write("log", &text);
    Ok(())
}

/// 只做副作用、不挂任何函数的扩展。
struct MarkerExtension;

impl ScriptExtension for MarkerExtension {
    fn register<'js>(&self, _ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("marker", "M")?;
        Ok(())
    }
}

/// 想覆盖前一个扩展注册的 `capture` 的扩展（默认应当被拒绝）。
struct CaptureHijacker {
    allow: bool,
}

impl ScriptExtension for CaptureHijacker {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set(
            "capture",
            Function::new(ctx.clone(), |_text: String| -> String { "hijacked".into() })?,
        )?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "capture",
            signature: "capture(text: string) -> string",
            doc: "测试用：覆盖别人注册的 capture",
        }]
    }

    fn allow_override(&self) -> bool {
        self.allow
    }
}

/// 构建期望失败的运行时（`ScriptRuntime` 没有 `Debug`，不能用 `expect_err`）。
///
/// 返回 `{:#}` 格式的错误链 —— 启动期错误的说明挂在 source 上，
/// 只看顶层 Display 会得到一句笼统的「注册 JS 能力失败」。
async fn runtime_expect_err(options: RuntimeOptions) -> String {
    match ScriptRuntime::with_options(options).await {
        Ok(_) => panic!("运行时创建应当失败"),
        Err(err) => format!("{err:#}"),
    }
}

async fn runtime_with(
    extensions: Vec<Arc<dyn ScriptExtension>>,
    console: Arc<TestConsole>,
) -> anyhow::Result<ScriptRuntime> {
    // 名字由使用方配置（引擎自己不含业务名字）：测试里就叫 MyTool / $
    let mut options = RuntimeOptions::default()
        .console(console)
        .namespace("MyTool")
        .namespace_alias("$");
    for extension in extensions {
        options = options.extension(extension);
    }
    ScriptRuntime::with_options(options).await
}

/// 扩展注册的函数在 `$` 与 `MyTool` 上是同一个对象，并且能在 Rust 侧访问引擎设施
/// （这里用 ConsoleHook：`$.capture` 把文本写进控制台钩子）。
#[tokio::test]
async fn extension_capability_reaches_engine_facility() {
    let console = Arc::new(TestConsole::default());
    let runtime = runtime_with(vec![Arc::new(CaptureExtension)], console.clone())
        .await
        .expect("创建运行时失败");

    let same: bool = runtime
        .eval(
            r#"
            $.capture("abc");
            $ === MyTool && $.capture === MyTool.capture
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(same, "$ 与 MyTool 应当指向同一个对象");
    assert_eq!(
        console.lines.lock().unwrap().clone(),
        vec![("log".to_string(), "abc".to_string())]
    );
}

/// `spec()` 声明的名字必须真的注册到了 `$` 上（防止声明与实现漂移）。
#[tokio::test]
async fn spec_names_exist_on_namespace() {
    let host = Arc::new(TestConsole::default());
    let extensions: Vec<Arc<dyn ScriptExtension>> = vec![Arc::new(CaptureExtension)];
    let runtime = runtime_with(extensions.clone(), host)
        .await
        .expect("创建运行时失败");

    for extension in extensions {
        for spec in extension.spec() {
            let present: bool = runtime
                .eval(&format!("typeof $.{} === 'function'", spec.name))
                .await
                .expect("脚本执行失败");
            assert!(present, "spec 声明了 {} 但运行期不存在", spec.name);
        }
    }
}

/// 名字是使用方配置出来的：没配就没有 `$`，配了才有（引擎自己不含任何业务名字）。
#[tokio::test]
async fn namespace_names_come_from_configuration() {
    let unconfigured = ScriptRuntime::new().await.expect("创建运行时失败");
    let value: String = unconfigured
        .eval("typeof MyTool + '|' + typeof $")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "undefined|undefined", "默认配置不该挂任何命名空间");

    let configured = runtime_with(vec![], Arc::new(TestConsole::default()))
        .await
        .expect("创建运行时失败");
    let value: String = configured
        .eval("typeof MyTool + '|' + typeof $ + '|' + String($ === MyTool)")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "object|object|true");
}

/// 默认情况下扩展不能覆盖别人已注册的能力。
#[tokio::test]
async fn conflicting_capability_is_rejected_by_default() {
    let text = runtime_expect_err(
        RuntimeOptions::default()
            .console(Arc::new(TestConsole::default()))
            .namespace("MyTool")
            .extension(Arc::new(CaptureExtension))
            .extension(Arc::new(CaptureHijacker { allow: false })),
    )
    .await;

    assert!(text.contains("能力名冲突"), "错误信息应说明冲突：{text}");
    assert!(text.contains("capture"), "错误信息应含冲突名：{text}");
}

/// 显式 `allow_override()` 的扩展可以覆盖。
#[tokio::test]
async fn conflicting_capability_is_allowed_when_explicit() {
    let runtime = runtime_with(
        vec![
            Arc::new(CaptureExtension),
            Arc::new(CaptureHijacker { allow: true }),
        ],
        Arc::new(TestConsole::default()),
    )
    .await
    .expect("显式允许覆盖时应当成功");

    let value: String = runtime
        .eval(r#"$.capture("abc")"#)
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "hijacked");
}

/// 只做副作用的扩展（没有 spec）也能注册。
#[tokio::test]
async fn side_effect_only_extension_registers() {
    let runtime = runtime_with(
        vec![Arc::new(MarkerExtension)],
        Arc::new(TestConsole::default()),
    )
    .await
    .expect("创建运行时失败");

    let value: String = runtime.eval("$.marker").await.expect("脚本执行失败");
    assert_eq!(value, "M");
}

/// 取消令牌对扩展可见：`bindings::cancelled` 能让能力在动手之前先退出。
#[tokio::test]
async fn cancel_signal_is_visible_to_bindings() {
    let token = CancelSignal::new();
    token.cancel();

    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .console(Arc::new(TestConsole::default()))
            .namespace("MyTool")
            .namespace_alias("$")
            .extension(Arc::new(CancelAwareExtension))
            .cancel(token),
    )
    .await
    .expect("创建运行时失败");

    let value: String = runtime
        .eval(
            r#"
            try {
                $.probe();
                "没有抛错"
            } catch (err) {
                err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(value.contains("已中止"), "取消后能力应抛中止异常：{value}");
}

/// 会检查取消状态的测试扩展。
struct CancelAwareExtension;

impl ScriptExtension for CancelAwareExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("probe", Function::new(ctx.clone(), js_probe)?)?;
        Ok(())
    }
}

fn js_probe<'js>(ctx: Ctx<'js>) -> QjsResult<()> {
    if script_engine::bindings::cancelled(&ctx) {
        return Err(script_engine::bindings::throw_cancelled(&ctx));
    }
    Ok(())
}

/// 声明了能力却没真的注册 → 运行时创建失败（避免补全提示不存在的方法）。
#[tokio::test]
async fn declared_but_unregistered_capability_fails_loudly() {
    /// `spec` 声明 `ghost`，但 `register` 什么也不挂。
    struct GhostExtension;

    impl ScriptExtension for GhostExtension {
        fn register<'js>(&self, _ctx: &Ctx<'js>, _ns: &Object<'js>) -> QjsResult<()> {
            Ok(())
        }

        fn spec(&self) -> Vec<CapabilitySpec> {
            vec![CapabilitySpec {
                name: "ghost",
                signature: "ghost() -> void",
                doc: "测试用：只声明不注册",
            }]
        }
    }

    let text = runtime_expect_err(
        RuntimeOptions::default()
            .console(Arc::new(TestConsole::default()))
            .extension(Arc::new(GhostExtension)),
    )
    .await;

    assert!(
        text.contains("能力声明与实现不一致"),
        "应当报出声明与实现不一致：{text}"
    );
    assert!(text.contains("ghost"), "应当指出具体能力：{text}");
}

/// `console.*` 的输出应当交给 [`ConsoleHook`]，而不是引擎自己写 stdout。
///
/// 校验两件事：五档 level 都被正确透传；`prelude.js` 的格式化（字符串原样、
/// 其余值走 `JSON.stringify`）在换掉落点后仍然生效。
#[tokio::test]
async fn console_output_reaches_hook() {
    let console = Arc::new(TestConsole::default());
    let runtime = ScriptRuntime::with_options(RuntimeOptions::default().console(console.clone()))
        .await
        .expect("创建运行时失败");

    runtime
        .eval::<()>(
            r#"
            console.log("普通字符串");
            console.info("信息");
            console.debug("调试");
            console.warn("警告");
            console.error("错误");
            console.log("混合", { a: 1 }, [1, 2]);
            "#,
        )
        .await
        .expect("脚本执行失败");

    let received = console.lines.lock().unwrap().clone();
    let levels: Vec<String> = received.iter().map(|(level, _)| level.clone()).collect();
    assert_eq!(
        levels,
        vec!["log", "info", "debug", "warn", "error", "log"],
        "五档 level 都应当透传，且保持调用顺序"
    );

    assert_eq!(received[0].1, "普通字符串", "字符串应当原样输出");
    assert_eq!(
        received[5].1, r#"混合 {"a":1} [1,2]"#,
        "非字符串应当走 JSON.stringify 并以空格连接"
    );
}

/// 不设置钩子时走默认的 [`script_engine::StdoutConsole`]，`console.*` 不能抛异常
/// （打印失败不该打断脚本）。
#[tokio::test]
async fn console_without_hook_falls_back_to_stdout() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");

    let value: String = runtime
        .eval(
            r#"
            const cyclic = {};
            cyclic.self = cyclic;
            console.log("循环引用", cyclic);
            "ok"
            "#,
        )
        .await
        .expect("console 不应让脚本失败");

    assert_eq!(value, "ok");
}
