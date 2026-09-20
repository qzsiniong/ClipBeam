//! 能力命名空间的配置与校验（`RuntimeOptions::namespace` / `namespace_alias`）。
//!
//! 引擎只建一个匿名对象，名字由使用方配置 —— 这个文件把「配置怎么生效」与
//! 「配置错了会怎样」都钉住：
//!
//! * 默认不挂任何全局（引擎自己不含业务名字）；
//! * 配了名字就是不可写、不可配置的全局绑定；
//! * 所有扩展注册完之后命名空间被 `Object.freeze`（脚本改不了能力）；
//! * 非法标识符 / 与已有全局冲突 / 正式名等于别名，都在启动时报错。

use std::sync::Arc;

use rquickjs::{Ctx, Function, Object, Result as QjsResult};
use script_engine::{
    CancelSignal, CapabilitySpec, ConsoleHook, RuntimeOptions, ScriptExtension, ScriptRuntime,
};

/// 测试用的空控制台（避免测试往标准流上写东西）。
#[derive(Default)]
struct TestConsole;

impl ConsoleHook for TestConsole {
    fn write(&self, _level: &str, _text: &str) {}
}

/// 一个最小的能力扩展：`$.ping()`。
struct PingExtension;

impl ScriptExtension for PingExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        ns.set(
            "ping",
            Function::new(ctx.clone(), || -> String { "pong".into() })?,
        )?;
        Ok(())
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "ping",
            signature: "ping() -> string",
            doc: "测试用",
        }]
    }
}

/// 建一个「配了命名空间 + 装了 ping 扩展」的运行时。
async fn runtime_with_namespace(options: RuntimeOptions) -> anyhow::Result<ScriptRuntime> {
    ScriptRuntime::with_options(
        options
            .console(Arc::new(TestConsole))
            .extension(Arc::new(PingExtension)),
    )
    .await
}

/// 启动失败时的错误链（`ScriptRuntime` 没有 `Debug`，不能用 `expect_err`）。
async fn setup_error(options: RuntimeOptions) -> String {
    match runtime_with_namespace(options).await {
        Ok(_) => panic!("运行时创建应当失败"),
        Err(err) => format!("{err:#}"),
    }
}

/// 默认配置不挂任何命名空间全局。
#[tokio::test]
async fn no_namespace_by_default() {
    let runtime = ScriptRuntime::new().await.expect("创建运行时失败");
    let value: String = runtime
        .eval("typeof MyTool + '|' + typeof $")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "undefined|undefined");
}

/// 配置了名字与别名：两者指向同一个对象，能力挂在它上面，绑定不可写且对象被冻结。
#[tokio::test]
async fn configured_namespace_is_attached_readonly_and_frozen() {
    let runtime = runtime_with_namespace(
        RuntimeOptions::default()
            .namespace("MyTool")
            .namespace_alias("$"),
    )
    .await
    .expect("创建运行时失败");

    let value: String = runtime
        .eval(
            r#"
            const alias = $ === MyTool;
            const descriptor = Object.getOwnPropertyDescriptor(globalThis, "MyTool");
            [
              typeof MyTool,
              String(alias),
              String(Object.isFrozen(MyTool)),
              MyTool.ping(),
              String(descriptor.writable),
              String(descriptor.configurable),
            ].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(
        value, "object|true|true|pong|false|false",
        "命名空间要挂上、别名同源、对象冻结、绑定不可写不可配置"
    );
}

/// 只有正式名、没有别名时也能用。
#[tokio::test]
async fn alias_is_optional() {
    let runtime = runtime_with_namespace(RuntimeOptions::default().namespace("MyTool"))
        .await
        .expect("创建运行时失败");

    let value: String = runtime
        .eval("typeof MyTool + '|' + typeof $")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "object|undefined");
}

/// 空别名等于「不要别名」（不报错）。
#[tokio::test]
async fn empty_alias_means_no_alias() {
    let runtime = runtime_with_namespace(
        RuntimeOptions::default()
            .namespace("MyTool")
            .namespace_alias(""),
    )
    .await
    .expect("创建运行时失败");

    let value: String = runtime
        .eval("typeof MyTool + '|' + typeof $")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "object|undefined");
}

/// 冻结之后脚本改不了能力、也改不了全局绑定：越权操作**报错**（脚本是严格模式），
/// 而且原样保留。
#[tokio::test]
async fn scripts_cannot_mutate_frozen_namespace() {
    let runtime = runtime_with_namespace(RuntimeOptions::default().namespace("MyTool"))
        .await
        .expect("创建运行时失败");

    let value: String = runtime
        .eval(
            r#"
            const results = [];
            const attempt = (label, action) => {
              try {
                action();
                results.push(label + ":没报错");
              } catch (err) {
                results.push(label + ":" + err.constructor.name);
              }
            };
            attempt("改能力", () => { MyTool.ping = null; });
            attempt("加能力", () => { MyTool.extra = "x"; });
            attempt("删能力", () => { delete MyTool.ping; });
            attempt("改全局", () => { MyTool = 1; });
            results.push(MyTool.ping());
            results.push(String(typeof MyTool.extra));
            results.join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(
        value, "改能力:TypeError|加能力:TypeError|删能力:TypeError|改全局:TypeError|pong|undefined",
        "冻结 + 不可写绑定要让越权操作直接报错，且原状不受影响"
    );
}

/// 名字非法 / 与已有全局冲突 / 正式名等于别名 —— 都在启动时报错，绝不静默降级。
#[tokio::test]
async fn invalid_namespace_config_is_rejected() {
    // 空名字
    let text = setup_error(RuntimeOptions::default().namespace("")).await;
    assert!(text.contains("不是合法的 JS 标识符"), "{text}");

    // 非法标识符
    let text = setup_error(RuntimeOptions::default().namespace("my-tool")).await;
    assert!(text.contains("不是合法的 JS 标识符"), "{text}");

    // 与引擎已提供的标准全局冲突（sleep 是本 crate 自己挂的）
    let text = setup_error(RuntimeOptions::default().namespace("sleep")).await;
    assert!(text.contains("已被占用"), "{text}");

    // 与内置对象冲突
    let text = setup_error(RuntimeOptions::default().namespace("Object")).await;
    assert!(text.contains("已被占用"), "{text}");

    // 与 prelude 稍后会挂的全局冲突（setup 阶段还看不到，必须单独挡）
    let text = setup_error(RuntimeOptions::default().namespace("console")).await;
    assert!(text.contains("已被占用"), "{text}");

    // 别名非法
    let text = setup_error(
        RuntimeOptions::default()
            .namespace("MyTool")
            .namespace_alias("my alias"),
    )
    .await;
    assert!(text.contains("不是合法的 JS 标识符"), "{text}");

    // 别名与正式名相同
    let text = setup_error(
        RuntimeOptions::default()
            .namespace("MyTool")
            .namespace_alias("MyTool"),
    )
    .await;
    assert!(text.contains("不能同名"), "{text}");
}

/// 挂命名空间不会影响扩展的注册与取消信号（配置只是「把对象挂出去」）。
#[tokio::test]
async fn namespace_does_not_disturb_extensions_or_cancel() {
    let cancel = CancelSignal::new();
    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .console(Arc::new(TestConsole))
            .namespace("MyTool")
            .namespace_alias("$")
            .extension(Arc::new(PingExtension))
            .cancel(cancel),
    )
    .await
    .expect("创建运行时失败");

    let value: String = runtime
        .eval("MyTool.ping() + '|' + typeof sleep")
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "pong|function");
}
