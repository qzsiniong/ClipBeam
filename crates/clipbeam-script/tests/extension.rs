//! 扩展机制的集成测试：注册、签名声明、重名冲突、宿主可达性。
//!
//! 这里用一个**测试扩展**（`CaptureExtension`）证明「使用方能自由注入能力」这件事真的成立：
//! 扩展把每次调用转交给测试宿主，于是可以断言 JS → Rust 的完整链路。

use std::sync::{Arc, Mutex};

use clipbeam_script::{
    BasicExtension, CancellationToken, CapabilitySpec, ConfirmChoice, HostError, RuntimeOptions,
    ScriptExtension, ScriptHost, ScriptRuntime,
};
use rquickjs::{Ctx, Function, Object, Result as QjsResult};

/// 测试宿主：记录输出与进度，确认答案可配置。
#[derive(Default)]
struct TestHost {
    typed: Mutex<String>,
    progress: Mutex<Vec<(usize, usize)>>,
    answer: Mutex<Option<ConfirmChoice>>,
    token: Mutex<Option<CancellationToken>>,
    /// 收到的 `console.*` 输出（level, text）。
    console: Mutex<Vec<(String, String)>>,
}

impl ScriptHost for TestHost {
    fn type_str(&self, text: &str, _delay_ms: u64) -> Result<(), HostError> {
        self.typed.lock().unwrap().push_str(text);
        // 逐字符上报进度，模拟真实宿主的粒度
        let total = text.chars().count();
        self.progress
            .lock()
            .unwrap()
            .extend((1..=total).map(|n| (n, total)));
        Ok(())
    }

    fn progress(&self, typed: usize, total: usize) {
        self.progress.lock().unwrap().push((typed, total));
    }

    fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
        Ok(self.answer.lock().unwrap().unwrap_or(ConfirmChoice::No))
    }

    fn cancelled(&self) -> bool {
        self.token
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    fn console(&self, level: &str, text: &str) {
        self.console
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

fn js_capture<'js>(ctx: Ctx<'js>, text: String) -> QjsResult<()> {
    let Some(host) = clipbeam_script::bindings::host_ctx(&ctx) else {
        return Err(rquickjs::Exception::throw_message(&ctx, "没有宿主"));
    };
    host.type_str(&text, 0)
        .map_err(|err| rquickjs::Exception::throw_message(&ctx, &err.message()))
}

/// 只做副作用、不挂任何函数的扩展。
struct MarkerExtension;

impl ScriptExtension for MarkerExtension {
    fn register<'js>(&self, _ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set("marker", "M")?;
        Ok(())
    }
}

/// 想覆盖 `$.file` 的扩展（默认应当被拒绝）。
struct FileHijacker {
    allow: bool,
}

impl ScriptExtension for FileHijacker {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        // 真能当 $.file 用：接一个 path 参数并返回固定字符串
        ns.set(
            "file",
            Function::new(ctx.clone(), |_path: String| -> String {
                "hijacked".into()
            })?,
        )?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "file",
            signature: "file(path: string) -> string",
            doc: "测试用：覆盖 core 的 $.file",
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
    host: Arc<TestHost>,
) -> anyhow::Result<ScriptRuntime> {
    let mut options = RuntimeOptions::default().host(host);
    for extension in extensions {
        options = options.extension(extension);
    }
    ScriptRuntime::with_options(options).await
}

/// 扩展注册的函数在 `$` 与 `Clipbeam` 上是同一个对象，且能调到宿主。
#[tokio::test]
async fn extension_capability_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(vec![Arc::new(CaptureExtension)], host.clone())
        .await
        .expect("创建运行时失败");

    let same: bool = runtime
        .eval(
            r#"
            $.capture("abc");
            $ === Clipbeam && $.capture === Clipbeam.capture
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(same, "$ 与 Clipbeam 应当指向同一个对象");
    assert_eq!(host.typed.lock().unwrap().as_str(), "abc");
    assert_eq!(
        host.progress.lock().unwrap().clone(),
        vec![(1, 3), (2, 3), (3, 3)],
        "应当逐字符上报进度"
    );
}

/// `spec()` 声明的名字必须真的注册到了 `$` 上（防止声明与实现漂移）。
#[tokio::test]
async fn spec_names_exist_on_namespace() {
    let host = Arc::new(TestHost::default());
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

/// core 基础能力的声明也必须成立。
#[tokio::test]
async fn core_spec_names_exist() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");

    for spec in BasicExtension.spec() {
        let present: bool = runtime
            .eval(&format!("typeof $.{} === 'function'", spec.name))
            .await
            .expect("脚本执行失败");
        assert!(present, "core spec 声明了 {} 但运行期不存在", spec.name);
    }
}

/// 默认情况下扩展不能覆盖 core 已注册的能力。
#[tokio::test]
async fn conflicting_capability_is_rejected_by_default() {
    let text = runtime_expect_err(
        RuntimeOptions::default()
            .host(Arc::new(TestHost::default()))
            .extension(Arc::new(FileHijacker { allow: false })),
    )
    .await;

    assert!(text.contains("能力名冲突"), "错误信息应说明冲突：{text}");
    assert!(text.contains("file"), "错误信息应含冲突名：{text}");
}

/// 显式 `allow_override()` 的扩展可以覆盖。
#[tokio::test]
async fn conflicting_capability_is_allowed_when_explicit() {
    let runtime = runtime_with(
        vec![Arc::new(FileHijacker { allow: true })],
        Arc::new(TestHost::default()),
    )
    .await
    .expect("显式允许覆盖时应当成功");

    let value: String = runtime
        .eval("$.file('whatever')")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "hijacked");
}

/// 只做副作用的扩展（没有 spec）也能注册。
#[tokio::test]
async fn side_effect_only_extension_registers() {
    let runtime = runtime_with(vec![Arc::new(MarkerExtension)], Arc::new(TestHost::default()))
        .await
        .expect("创建运行时失败");

    let value: String = runtime.eval("$.marker").await.expect("脚本执行失败");
    assert_eq!(value, "M");
}

/// 取消令牌对扩展可见：`bindings::cancelled` 能让能力在动手之前先退出。
#[tokio::test]
async fn cancel_token_is_visible_to_bindings() {
    let token = CancellationToken::new();
    token.cancel();

    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .host(Arc::new(TestHost::default()))
            .extension(Arc::new(CancelAwareExtension))
            .cancel_token(token),
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
    if clipbeam_script::bindings::cancelled(&ctx) {
        return Err(clipbeam_script::bindings::throw_cancelled(&ctx));
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
            .host(Arc::new(TestHost::default()))
            .extension(Arc::new(GhostExtension)),
    )
    .await;

    assert!(
        text.contains("能力声明与实现不一致"),
        "应当报出声明与实现不一致：{text}"
    );
    assert!(text.contains("ghost"), "应当指出具体能力：{text}");
}

/// `console.*` 的输出应当经宿主，而不是直接写进程的 stdout/stderr。
///
/// 校验两件事：五档 level 都被正确透传；`prelude.js` 的格式化（字符串原样、
/// 其余值走 `JSON.stringify`）在换掉输出目标后仍然生效。
#[tokio::test]
async fn console_output_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime_with_host = ScriptRuntime::with_options(RuntimeOptions::default().host(host.clone()))
        .await
        .expect("创建运行时失败");

    runtime_with_host
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

    let received = host.console.lock().unwrap().clone();
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

/// 没有宿主时 `console.*` 不能抛异常（打印失败不该打断脚本）。
#[tokio::test]
async fn console_without_host_does_not_throw() {
    // 用不注册任何扩展、也不注入宿主的运行时……但 RuntimeOptions 默认就是 NoopHost，
    // 这里验证的是「宿主存在但会走默认实现」的路径不会炸，以及桥函数本身不抛。
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
