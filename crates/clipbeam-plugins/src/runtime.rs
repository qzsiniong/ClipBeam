//! 插件运行时的组装与驱动。
//!
//! # 与脚本运行时的关系
//!
//! **同一个引擎，两套组装**：这里用 [`script_engine::ScriptRuntime`] 跑插件的 JS，
//! 只是换了一套能力（[`crate::extensions`]）、一个独立的命名空间
//! （[`crate::NAMESPACE`] / [`crate::NAMESPACE_ALIAS`]）和一个独立的上下文附着物
//! （[`crate::context`]）—— 脚本那套 `ClipBeam` / `$` 与待命窗口、键盘注入全部不参与，
//! 两边的生命周期（跑一次 vs 常驻）也不会互相污染。
//!
//! # 宿主怎么驱动插件
//!
//! 1. [`PluginRuntime::run_entry`]：跑一次入口脚本。插件在这里用
//!    `$plugin.tray.onAction(cb)` 之类注册回调，并往 [`crate::host::ActionRegistry`] 登记动作；
//! 2. [`PluginRuntime::eval_action`]：之后每收到一个动作就唤醒一次对应的回调。
//!
//! 唤醒靠引擎的 [`script_engine::ScriptRuntime::eval_global`]：回调被登记为一个全局函数，
//! 载荷以 JSON 形态递进去。引擎不认识「插件」「动作」这些概念，只做「按名字调 JS 函数」。

use std::path::Path;
use std::sync::Arc;

use script_engine::{ConsoleHook, ScriptRuntime, StdoutConsole};

use crate::context::{self, PluginContext, PluginShared};
use crate::error::PluginError;
use crate::extensions;
use crate::host::{PluginHost, PluginMeta};
use crate::permission::PermissionSet;

/// 支持的入口扩展名（与 `script-engine::ts` 的转译范围一致）。
pub const ENTRY_EXTENSIONS: [&str; 6] = ["js", "mjs", "cjs", "ts", "mts", "cts"];

/// 入口文件大小上限：插件入口是**代码**，不是数据。超过这个量级说明用法跑偏了，
/// 而且读进内存前就该拦住（避免有人放一个大文件进来）。
pub const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

/// 组装插件运行时所需的一切。
pub struct PluginRuntimeOptions {
    /// 插件身份（宿主侧拼好，含目录）。
    pub meta: PluginMeta,
    /// 声明的权限（未声明的调用会被拒绝）。
    pub permissions: PermissionSet,
    /// `console.*` 的落点；缺省写标准流（GUI 会换成插件日志面板）。
    pub console: Arc<dyn ConsoleHook>,
    /// 取消信号；`Some` 时 `sleep` 与能力检查能提前返回。
    pub cancel: Option<script_engine::CancelSignal>,
}

impl PluginRuntimeOptions {
    /// 用插件身份建一份配置（权限默认全不开，`console` 写标准流，不注入取消信号）。
    pub fn new(meta: PluginMeta) -> Self {
        Self {
            meta,
            permissions: PermissionSet::NONE,
            console: Arc::new(StdoutConsole),
            cancel: None,
        }
    }

    /// 设置权限集合。
    pub fn permissions(mut self, permissions: PermissionSet) -> Self {
        self.permissions = permissions;
        self
    }

    /// 设置 `console.*` 的落点。
    pub fn console(mut self, console: Arc<dyn ConsoleHook>) -> Self {
        self.console = console;
        self
    }

    /// 注入取消信号。
    pub fn cancel(mut self, cancel: script_engine::CancelSignal) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl std::fmt::Debug for PluginRuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntimeOptions")
            .field("meta", &self.meta)
            .field("permissions", &self.permissions)
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
}

/// 组装一份「带 `$plugin` 全部能力 + 指定宿主与日志落点」的引擎配置。
///
/// 宿主句柄与权限通过 `prepare` 钩子存进上下文（引擎不认识「插件」这个概念，
/// 它只提供「扩展注册前让我做点初始化」的入口）—— 与 `clipbeam-scripting` 的
/// `runtime_options` 同一个手法。
pub fn runtime_options(
    host: Arc<dyn PluginHost>,
    options: PluginRuntimeOptions,
) -> script_engine::RuntimeOptions {
    let PluginRuntimeOptions {
        meta,
        permissions,
        console,
        cancel,
    } = options;

    // 用 `Arc` 包一层：`prepare` 是 `Fn` 闭包（可能被调多次），不能把结构体移进去；
    // 每次构造时克隆一份「指向同一份共享状态」的上下文。
    let plugin_context = Arc::new(PluginContext {
        host,
        meta,
        permissions,
        shared: Arc::new(std::sync::Mutex::new(PluginShared::default())),
    });

    let mut runtime_options = script_engine::RuntimeOptions::default()
        .extensions(extensions::extensions())
        .console(console)
        .namespace(crate::NAMESPACE)
        .namespace_alias(crate::NAMESPACE_ALIAS)
        .prepare(Arc::new(move |ctx| {
            // 宿主句柄与权限必须在扩展注册**之前**进上下文：`register` 里就要能取到它们。
            let context = plugin_context.clone();
            context::store_context(
                ctx,
                PluginContext {
                    host: context.host.clone(),
                    meta: context.meta.clone(),
                    permissions: context.permissions,
                    shared: context.shared.clone(),
                },
            )
            .map(|_| ())
        }));

    if let Some(cancel) = cancel {
        runtime_options = runtime_options.cancel(cancel);
    }

    runtime_options
}

/// 建一个能跑插件的运行时。
pub async fn create_runtime(
    host: Arc<dyn PluginHost>,
    options: PluginRuntimeOptions,
) -> anyhow::Result<PluginRuntime> {
    let host_ref = host.clone();
    let runtime = ScriptRuntime::with_options(runtime_options(host, options)).await?;
    Ok(PluginRuntime {
        runtime,
        host: host_ref,
    })
}

/// 一个装好插件能力与宿主句柄的运行时。
pub struct PluginRuntime {
    runtime: ScriptRuntime,
    /// 宿主句柄的副本：驱动动作前检查「插件是否已被要求停止」。
    host: Arc<dyn PluginHost>,
}

impl PluginRuntime {
    /// 跑一次插件入口脚本（`name` 出现在错误栈里，用文件名）。
    pub async fn run_entry(&self, name: &str, code: &str) -> anyhow::Result<()> {
        self.runtime.run_named_script(name, code).await
    }

    /// 按动作 id 唤醒插件登记的回调。
    ///
    /// 动作表由 `$plugin.tray.onAction` 在入口里登记（接口级 key 见
    /// [`crate::host::GENERIC_ACTION_KEY`]，插件也可以为某个菜单项单独登记）：
    ///
    /// * 查表规则：**先精确匹配 `action_id`，再回落到接口级回调**；
    /// * 一个都没命中 → 明确报错。清单里写了 `menus[].id` 却没实现回调，
    ///   是插件作者的 bug，不该表现为「点了没反应」；
    /// * 回调抛异常 / 全局函数不存在 → 错误原样冒给宿主（宿主显示在插件日志里）。
    pub async fn eval_action(
        &self,
        action_id: &str,
        payload: serde_json::Value,
    ) -> anyhow::Result<()> {
        if self.host.stopped() {
            return Err(anyhow::Error::new(PluginError::Stopped));
        }

        // 查表必须在求值作用域里做：动作表是上下文里的 userdata，
        // `AsyncContext` 只在 `async_with` 里能读（这也是拿不到 JS 引用的原因）。
        let action_id_owned = action_id.to_string();
        let plugin_id = self.host.meta().id.clone();
        let resolved: Result<String, String> = self
            .runtime
            .context()
            .async_with(async move |ctx| {
                let Some(context) = context::context(&ctx) else {
                    return Err("插件上下文丢失（运行时不是按插件方式组装的）".to_string());
                };
                let lookup = {
                    let shared = context
                        .shared
                        .lock()
                        .map_err(|_| "插件状态锁已损坏".to_string())?;
                    shared.actions().resolve(&action_id_owned)
                };

                match lookup {
                    Some((key, global_name)) => {
                        // 命中接口级回调时，载荷里的 action.id 必须带上 —— 插件靠它区分菜单项
                        if key == crate::host::GENERIC_ACTION_KEY {
                            log::debug!(
                                "插件 {plugin_id} 的动作 {action_id_owned:?} 由接口级回调 {global_name} 处理"
                            );
                        }
                        Ok(global_name)
                    }
                    None => Err(format!(
                        "插件 {plugin_id} 没有登记动作 {action_id_owned:?}\
                         （清单里的 menus 需要在入口里用 $plugin.tray.onAction 实现）"
                    )),
                }
            })
            .await;

        let global_name = resolved.map_err(anyhow::Error::msg)?;
        self.runtime.eval_global(&global_name, payload).await
    }

    /// 已登记的动作键（诊断用：宿主在插件日志里列出插件登记了哪些动作）。
    ///
    /// 只读动作表的**名字**，不碰任何 JS 值 —— 回调本体留在上下文里，
    /// 由 `eval_action` 在求值作用域内唤醒。因此这个方法是异步的：
    /// userdata 只能在 `async_with` 作用域里读。
    pub async fn registered_action_keys(&self) -> Vec<String> {
        self.runtime
            .context()
            .async_with(async move |ctx| {
                context::context(&ctx)
                    .and_then(|context| {
                        context
                            .shared
                            .lock()
                            .ok()
                            .map(|shared| shared.actions().keys())
                    })
                    .unwrap_or_default()
            })
            .await
    }

    /// 原始引擎运行时（留给需要直接求值/注入全局的宿主）。
    pub fn runtime(&self) -> &ScriptRuntime {
        &self.runtime
    }
}

/// 读插件入口源码；必要时用 oxc 转译 TypeScript。
///
/// 语法错误在这里就带着文件名与行列返回，不会拖到运行期。
pub fn load_entry(meta: &PluginMeta) -> Result<String, String> {
    let path = meta.dir.join(&meta.entry);

    let metadata = std::fs::metadata(&path)
        .map_err(|err| format!("读取插件入口 {} 失败：{err}", path.display()))?;
    if metadata.len() > MAX_ENTRY_BYTES {
        return Err(format!(
            "插件入口 {} 过大（{} 字节，上限 {MAX_ENTRY_BYTES} 字节）：插件入口应当是代码，不是数据",
            path.display(),
            metadata.len()
        ));
    }

    let source = std::fs::read_to_string(&path)
        .map_err(|err| format!("读取插件入口 {} 失败：{err}", path.display()))?;

    script_engine::ts::transpile_if_needed(&source, Path::new(&meta.entry))
        .map(|code| code.into_owned())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakePluginHost;

    fn meta_with_dir(dir: &Path, entry: &str) -> PluginMeta {
        PluginMeta {
            id: "demo".into(),
            name: "演示".into(),
            version: "0.1.0".into(),
            description: None,
            author: None,
            dir: dir.to_path_buf(),
            entry: entry.into(),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clipbeam-plugin-runtime-{}-{seq}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    #[test]
    fn entry_extensions_cover_js_and_ts_families() {
        assert!(ENTRY_EXTENSIONS.contains(&"js"));
        assert!(ENTRY_EXTENSIONS.contains(&"ts"));
        assert!(!ENTRY_EXTENSIONS.contains(&"json"), "数据文件不是入口");
    }

    #[test]
    fn load_entry_reads_plain_javascript() {
        let dir = temp_dir("read-js");
        std::fs::write(dir.join("index.js"), "console.log('hi')").unwrap();

        let source = load_entry(&meta_with_dir(&dir, "index.js")).expect("读取失败");
        assert_eq!(source, "console.log('hi')");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_entry_transpiles_typescript() {
        let dir = temp_dir("read-ts");
        std::fs::write(
            dir.join("index.ts"),
            "const n: number = 1\nconsole.log(n)\n",
        )
        .unwrap();

        let source = load_entry(&meta_with_dir(&dir, "index.ts")).expect("转译失败");
        assert!(!source.contains(": number"), "类型注解应当被剥掉：{source}");
        assert!(source.contains("console.log(n)"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_entry_reports_syntax_errors_with_location() {
        let dir = temp_dir("read-bad-ts");
        std::fs::write(dir.join("index.ts"), "const = ;").unwrap();

        let err = load_entry(&meta_with_dir(&dir, "index.ts")).expect_err("语法错误应当报错");
        assert!(
            err.contains("index.ts"),
            "错误信息里应当带入口文件名：{err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_entry_rejects_oversized_files() {
        let dir = temp_dir("too-big");
        let big = "x".repeat((MAX_ENTRY_BYTES + 1) as usize);
        std::fs::write(dir.join("index.js"), big).unwrap();

        let err = load_entry(&meta_with_dir(&dir, "index.js")).expect_err("超大入口应当被拒绝");
        assert!(err.contains("过大"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_entry_reports_missing_file() {
        let dir = temp_dir("missing");
        let err = load_entry(&meta_with_dir(&dir, "index.js")).expect_err("缺文件应当报错");
        assert!(err.contains("读取插件入口"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 运行时能建起来、入口能跑、动作能唤醒、权限被强制执行（不需要 Tauri）。
    #[tokio::test]
    async fn runtime_runs_entry_and_dispatches_actions() {
        let host = FakePluginHost::with_id("demo");
        let runtime = create_runtime(
            Arc::new(host),
            PluginRuntimeOptions::new(PluginMeta {
                id: "demo".into(),
                name: "演示".into(),
                version: "0.1.0".into(),
                description: None,
                author: None,
                dir: std::path::PathBuf::from("/tmp/demo"),
                entry: "index.js".into(),
            })
            .permissions(PermissionSet {
                feedback: true,
                tray: true,
                ..PermissionSet::NONE
            }),
        )
        .await
        .expect("创建插件运行时失败");

        runtime
            .run_entry(
                "index.js",
                r#"
                globalThis.seen = [];
                $plugin.tray.onAction((action) => {
                  globalThis.seen.push(action.id);
                  $plugin.toast(`点了 ${action.id}`, { level: "success" });
                });
                "#,
            )
            .await
            .expect("跑入口失败");

        assert_eq!(
            runtime.registered_action_keys().await,
            vec!["*".to_string()],
            "onAction 登记的是接口级回调"
        );

        runtime
            .eval_action("hello", serde_json::json!({ "id": "hello" }))
            .await
            .expect("唤醒动作失败");

        let seen: String = runtime
            .runtime()
            .eval("globalThis.seen.join(',')")
            .await
            .expect("取回记录失败");
        assert_eq!(seen, "hello", "回调应当收到 action.id");
    }

    /// 清单里写了菜单项但没登记回调：必须报错，而不是「点了没反应」。
    #[tokio::test]
    async fn unknown_action_is_an_error() {
        let runtime = create_runtime(
            Arc::new(FakePluginHost::with_id("demo")),
            PluginRuntimeOptions::new(PluginMeta {
                id: "demo".into(),
                name: "演示".into(),
                version: "0.1.0".into(),
                description: None,
                author: None,
                dir: std::path::PathBuf::from("/tmp/demo"),
                entry: "index.js".into(),
            }),
        )
        .await
        .expect("创建插件运行时失败");

        // 连入口都没跑（动作表是空的）
        let err = runtime
            .eval_action("nope", serde_json::json!({}))
            .await
            .expect_err("未登记的动作应当报错");
        assert!(err.to_string().contains("没有登记动作"), "{err}");
        assert!(
            err.to_string().contains("demo"),
            "错误里应当带插件 id：{err}"
        );

        // 跑了入口、但没登记任何回调：同样报错
        runtime
            .run_entry("index.js", "console.log('什么都不注册')")
            .await
            .expect("跑入口失败");
        let err = runtime
            .eval_action("nope", serde_json::json!({}))
            .await
            .expect_err("未登记的动作应当报错");
        assert!(err.to_string().contains("没有登记动作"), "{err}");
    }

    /// 未声明权限时调用能力：报错（不是静默成功），且文案指出该改清单哪一项。
    #[tokio::test]
    async fn capabilities_are_gated_by_declared_permissions() {
        let runtime = create_runtime(
            Arc::new(FakePluginHost::with_id("demo")),
            PluginRuntimeOptions::new(PluginMeta {
                id: "demo".into(),
                name: "演示".into(),
                version: "0.1.0".into(),
                description: None,
                author: None,
                dir: std::path::PathBuf::from("/tmp/demo"),
                entry: "index.js".into(),
            }),
            // 权限一个都不给
        )
        .await
        .expect("创建插件运行时失败");

        let message: String = runtime
            .runtime()
            .eval(
                r#"
                try { $plugin.toast("x"); "没有抛错" }
                catch (err) { err.message }
                "#,
            )
            .await
            .expect("求值失败");

        assert!(message.contains("未声明"), "应当报权限错误：{message}");
        assert!(message.contains("feedback"), "应当指出权限名：{message}");
        assert!(
            message.contains("$plugin.toast"),
            "应当指出能力名：{message}"
        );
    }
}
