//! QuickJS 运行时封装。
//!
//! 本模块只负责四件事：
//!
//! 1. 按 [`RuntimeOptions`] 创建 `AsyncRuntime` / `AsyncContext`；
//! 2. 把宿主与使用方扩展交给 [`crate::extension::setup`] 完成能力注入；
//! 3. 执行脚本，把 JS 异常翻译成可读的 Rust 错误；
//! 4. 让脚本可以直接使用**顶层 await**。
//!
//! 具体能力实现都在 `bindings/` 与使用方的扩展里，这里不做任何业务处理，
//! 因此新增能力不需要改动运行时。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use rquickjs::{
    context::EvalOptions, prelude::CatchResultExt, AsyncContext, AsyncRuntime, CaughtError, Ctx,
    FromJs, Promise, Result as QjsResult, Value,
};

use crate::bindings::timers::Timers;
use crate::cancel::CancelSignal;
use crate::console_hook::{ConsoleHook, StdoutConsole};
use crate::extension::{self, ScriptExtension};

/// 脚本结束后 `idle()` 清尾的时间上限。
///
/// 脚本**真正 await** 的任务在 `idle()` 之前就已经等完了（上面已经 await 过脚本的完成值），
/// 所以这里只是清尾：给一小段预算跑完已就绪的微任务即可。
///
/// 取小值（80ms）是刻意的：`setInterval` 这类任务永远有新活，`idle()` 不会自己结束；
/// 而且 `idle()` 内部自旋时会占住线程，给太多预算会拖慢脚本结束、也会让后续
/// `setTimeout` 的定时器迟迟得不到推进。
const IDLE_DRAIN: Duration = Duration::from_millis(80);

/// 内置前置脚本：补齐 quickjs-ng 缺失的标准 API（`TextDecoder` / `TextEncoder` / `console`）。
///
/// 内容见 `src/prelude.js`，在注册完 `__primitives` 原语之后执行一次。
/// 用 `include_str!` 内嵌进二进制，部署时无需附带额外文件。
const PRELUDE: &str = include_str!("prelude.js");

/// 上下文准备钩子：在扩展注册**之前**对 `Ctx` 做自定义初始化。
///
/// 引擎不认识业务概念（宿主、会话、令牌），但使用方的扩展需要在上下文里放自己的
/// 句柄（典型的做法是 `Ctx::store_userdata`，见 `Ctx::userdata` 取回）。
///
/// 注意：**给能力命名空间起名字不走这里**，用 [`RuntimeOptions::namespace`] /
/// [`RuntimeOptions::namespace_alias`] —— 那两个名字由引擎负责挂上并做校验。
/// 这个钩子就是那个「放」的入口：它拿到的 `Ctx` 已经存好 [`ConsoleHook`] 与
/// [`CancelSignal`]，而扩展还没开始注册 —— 于是扩展一上场就能取到宿主。
///
/// ```no_run
/// # use std::sync::Arc;
/// # use script_engine::{RuntimeOptions, ConsoleHook};
/// # fn demo(console: Arc<dyn ConsoleHook>) -> RuntimeOptions {
/// let prepare: script_engine::ContextSetup = Arc::new(|ctx| {
///     // 例如：ctx.store_userdata(HostRef(host.clone()))?;
///     let _ = ctx;
///     Ok(())
/// });
/// RuntimeOptions::default().console(console).prepare(prepare)
/// # }
/// ```
pub type ContextSetup = Arc<dyn for<'js> Fn(&Ctx<'js>) -> QjsResult<()> + Send + Sync>;

/// 运行时配置。
///
/// 用 [`RuntimeOptions::default`] 得到保守的默认值，再用链式方法按需覆盖。
/// 其中能力命名空间的名字（[`RuntimeOptions::namespace`]）由使用方决定：引擎自己
/// 不发明业务名字，默认（`None`）连全局都不挂。
///
/// ```no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use std::sync::Arc;
/// use script_engine::{RuntimeOptions, ScriptRuntime};
///
/// let options = RuntimeOptions::default()
///     .memory_limit(128 * 1024 * 1024)
///     .script_name("my-task.js");
/// let runtime = ScriptRuntime::with_options(options).await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct RuntimeOptions {
    /// JS 堆内存上限（字节）。超限时 QuickJS 会抛 `out of memory`。
    pub memory_limit: usize,
    /// JS 调用栈上限（字节）。防止脚本递归爆掉宿主进程的栈。
    pub max_stack_size: usize,
    /// 出现在 JS 错误栈里的脚本名，例如 `task.js:12:5`。
    pub script_name: String,
    /// `console.*` 的落点；默认 [`StdoutConsole`]（写标准流）。
    pub console: Arc<dyn ConsoleHook>,
    /// 使用方扩展（按顺序注册，core 基础能力先注册）。
    pub extensions: Vec<Arc<dyn ScriptExtension>>,
    /// 协作式取消信号；`None` 表示不可取消。
    pub cancel: Option<CancelSignal>,
    /// 上下文准备钩子；`None` 表示不做额外初始化。
    pub prepare: Option<ContextSetup>,
    /// 能力命名空间的全局名字；`None`（默认）表示不挂全局 —— **名字由使用方决定**。
    ///
    /// 引擎只创建匿名命名空间对象；这个名字是唯一给它命名的入口。给出时必须是合法
    /// JS 标识符、且不能与已有全局冲突（启动即报错，不静默覆盖）。
    pub namespace: Option<String>,
    /// 能力命名空间的别名（例如 `$`）；`None` 或空串表示不要别名。
    pub namespace_alias: Option<String>,
}

impl std::fmt::Debug for RuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOptions")
            .field("memory_limit", &self.memory_limit)
            .field("max_stack_size", &self.max_stack_size)
            .field("script_name", &self.script_name)
            .field("extensions", &self.extensions.len())
            .field("cancel", &self.cancel)
            .field("prepare", &self.prepare.is_some())
            .field("namespace", &self.namespace)
            .field("namespace_alias", &self.namespace_alias)
            .finish()
    }
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            // 50 MiB / 256 KiB 足够跑常规脚本，同时给宿主留出余量
            memory_limit: 50 * 1024 * 1024,
            max_stack_size: 256 * 1024,
            script_name: "script.js".to_string(),
            console: Arc::new(StdoutConsole),
            extensions: Vec::new(),
            cancel: None,
            prepare: None,
            namespace: None,
            namespace_alias: None,
        }
    }
}

impl RuntimeOptions {
    /// 设置 JS 堆上限（字节）。
    pub fn memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// 设置 JS 调用栈上限（字节）。
    pub fn max_stack_size(mut self, bytes: usize) -> Self {
        self.max_stack_size = bytes;
        self
    }

    /// 设置出现在错误栈里的脚本名。
    pub fn script_name(mut self, name: impl Into<String>) -> Self {
        self.script_name = name.into();
        self
    }

    /// 设置 `console.*` 的落点。
    pub fn console(mut self, console: Arc<dyn ConsoleHook>) -> Self {
        self.console = console;
        self
    }

    /// 追加一个能力扩展。
    pub fn extension(mut self, extension: Arc<dyn ScriptExtension>) -> Self {
        self.extensions.push(extension);
        self
    }

    /// 追加一组能力扩展。
    pub fn extensions(
        mut self,
        extensions: impl IntoIterator<Item = Arc<dyn ScriptExtension>>,
    ) -> Self {
        self.extensions.extend(extensions);
        self
    }

    /// 注入取消信号（`sleep` 与使用方能力都会检查它）。
    pub fn cancel(mut self, signal: CancelSignal) -> Self {
        self.cancel = Some(signal);
        self
    }

    /// 注入上下文准备钩子（见 [`ContextSetup`]）。
    pub fn prepare(mut self, setup: ContextSetup) -> Self {
        self.prepare = Some(setup);
        self
    }

    /// 给能力命名空间起名字（挂到 `globalThis` 上）。
    ///
    /// 决定权完全在使用方：引擎是通用引擎，自己不含任何业务名字。
    ///
    /// ```no_run
    /// # use script_engine::RuntimeOptions;
    /// let options = RuntimeOptions::default()
    ///     .namespace("MyTool")
    ///     .namespace_alias("$");
    /// # let _ = options;
    /// ```
    pub fn namespace(mut self, name: impl Into<String>) -> Self {
        self.namespace = Some(name.into());
        self
    }

    /// 给能力命名空间加一个别名（与正式名指向同一个对象）；空串等于不要别名。
    pub fn namespace_alias(mut self, alias: impl Into<String>) -> Self {
        self.namespace_alias = Some(alias.into());
        self
    }
}

/// 一个装好宿主与扩展能力的 QuickJS 运行时。
///
/// 生命周期说明：`ScriptRuntime` 内部同时持有 `AsyncRuntime` 与 `AsyncContext`，
/// 并且上下文里保存了运行时的引用计数。因此结构体字段的 drop 顺序不会导致
/// 「上下文还没释放就销毁引擎」的问题，无需手写 `Drop`。
pub struct ScriptRuntime {
    /// 引擎本身（异步版，内部有 future-aware 的全局锁）。
    rt: AsyncRuntime,
    /// 全局环境，所有脚本都在这里执行。
    ctx: AsyncContext,
    /// 创建时的配置，`run_script` 会用到其中的脚本名等。
    options: RuntimeOptions,
    /// 定时器表：每次脚本执行结束都要清空，避免残留任务漏到下一次运行。
    timers: Arc<Timers>,
}

impl ScriptRuntime {
    /// 用默认配置创建运行时（无扩展、无宿主）。
    pub async fn new() -> anyhow::Result<Self> {
        Self::with_options(RuntimeOptions::default()).await
    }

    /// 用自定义配置创建运行时，并注册全部内置能力与使用方扩展。
    pub async fn with_options(options: RuntimeOptions) -> anyhow::Result<Self> {
        let rt = AsyncRuntime::new().context("创建 QuickJS 运行时失败")?;

        // 注意：这两个 setter 是 async 的，忘记 .await 会静默失效（只是构造了个 Future）
        rt.set_memory_limit(options.memory_limit).await;
        rt.set_max_stack_size(options.max_stack_size).await;

        let ctx = AsyncContext::full(&rt)
            .await
            .context("创建 QuickJS 上下文失败")?;

        // 注册命名空间 / 标准全局 / 内部原语 / 控制台钩子 / 取消信号 / 扩展
        let setup_options = options.clone();
        let timers = ctx
            .with(move |ctx| extension::setup(&ctx, &setup_options))
            .await
            .context("注册 JS 能力失败")?;

        // 再执行一次内置前置脚本：用 JS 补齐 TextDecoder/TextEncoder/console
        // （它们依赖上一步注册的 __primitives 原语，所以顺序不能颠倒）
        ctx.with(|ctx| ctx.eval::<(), _>(PRELUDE))
            .await
            .context("执行内置前置脚本失败")?;

        // prelude 已经把需要的原语关进了自己的闭包（`const prim = globalThis.__primitives`），
        // 所以这里把引导用的全局删掉：脚本看不到引擎内部，也没机会依赖它。
        ctx.with(|ctx| {
            ctx.globals()
                .remove(crate::extension::PRIMITIVES_NAMESPACE)
                .map_err(|_| rquickjs::Exception::throw_message(&ctx, "移除内部原语命名空间失败"))
        })
        .await
        .context("清理内部原语命名空间失败")?;

        Ok(Self {
            rt,
            ctx,
            options,
            timers,
        })
    }

    /// 拿到原始上下文，用于扩展：挂载自定义全局变量、直接 eval 等。
    ///
    /// ```no_run
    /// # async fn demo(runtime: &script_engine::ScriptRuntime) {
    /// // 给脚本注入一个宿主变量
    /// let version = runtime
    ///     .context()
    ///     .with(|ctx| ctx.globals().set("HOST", "demo"))
    ///     .await;
    /// # let _ = version;
    /// # }
    /// ```
    pub fn context(&self) -> &AsyncContext {
        &self.ctx
    }

    /// 查看当前配置。
    pub fn options(&self) -> &RuntimeOptions {
        &self.options
    }

    /// 执行一段 JS 脚本，忽略返回值。
    ///
    /// 脚本里可以直接使用**顶层 await**（内部用 `JS_EVAL_FLAG_ASYNC` 求值），
    /// 因此 `await sleep(...)` 这类写法无需再包一层 async IIFE。
    /// 出错时返回的错误信息包含脚本名与行列号。
    ///
    /// 注意：脚本是被当作 **async 函数体**编译的，所以换行处的自动分号插入（ASI）
    /// 行为和普通脚本一致 —— 例如 `const x = f()` 换行后紧跟 `[...]`，会被解析成
    /// 「对 `f()` 的结果做下标访问」。语句末尾建议显式写分号。
    pub async fn run_script(&self, code: &str) -> anyhow::Result<()> {
        self.run_named_script(&self.options.script_name, code).await
    }

    /// 同 [`ScriptRuntime::run_script`]，但自定义错误栈里显示的脚本名
    /// （例如传入真实文件名）。
    pub async fn run_named_script(&self, name: &str, code: &str) -> anyhow::Result<()> {
        // `()` 的 FromJs 实现会忽略完成值
        self.eval_inner::<()>(name, code).await
    }

    /// 执行一段 JS 表达式并把它（await 之后的）结果取回 Rust。
    ///
    /// 返回的是脚本的**完成值**：最后一条表达式语句的值，例如
    /// `"new TextEncoder().encode(\"hi\").byteLength"` 会返回 2。
    /// `T` 用 `String`、`i32`、`Vec<String>` 等实现了 `FromJs` 的具体类型
    /// （不能带 JS 生命周期，所以必须是拥有所有权的类型）。
    ///
    /// ```no_run
    /// # async fn demo(runtime: &script_engine::ScriptRuntime) -> anyhow::Result<()> {
    /// let size: i32 = runtime.eval("new TextEncoder().encode(\"hi\").byteLength").await?;
    /// # let _ = size;
    /// # Ok(()) }
    /// ```
    pub async fn eval<T>(&self, code: &str) -> anyhow::Result<T>
    where
        // `'static` 是必要的：结果要跨出 `async_with` 作用域，只有拥有所有权的
        // 类型（String / Vec / 数字 / () 等）才能带出来，带 JS 引用的类型不行。
        T: for<'js> FromJs<'js> + 'static,
    {
        self.eval_inner::<T>(&self.options.script_name, code).await
    }

    /// 有上限地推进一次引擎任务（见 [`IDLE_DRAIN`]）。
    ///
    /// `idle()` 是内部自旋的，遇到 `setInterval` 这类永不结束的任务不会返回；
    /// 用 `timeout` 封顶后直接进入定时器清理阶段。
    async fn drain_idle(&self, budget: Duration) {
        let _ = tokio::time::timeout(budget, self.rt.idle()).await;
    }

    /// 求值实现：把 `Promise` 的解析结果或 JS 异常统一收敛成 `Result<T, String>`。
    ///
    /// 之所以在闭包内部就把错误转成 `String`：`CaughtError` 借用的是 JS 异常对象，
    /// 生命周期绑在上下文上，无法带出 `async_with` 的作用域。
    async fn eval_inner<T>(&self, name: &str, code: &str) -> anyhow::Result<T>
    where
        T: for<'js> FromJs<'js> + 'static,
    {
        let code = code.to_string();
        let name = name.to_string();

        let outcome: Result<T, String> = self
            .ctx
            .async_with(async move |ctx| {
                // EvalOptions 是 #[non_exhaustive]，跨 crate 不能用结构体字面量，
                // 只能先取默认值再逐字段覆盖。
                let mut options = EvalOptions::default();
                // 允许顶层 await：QuickJS 会按 JS_EVAL_FLAG_ASYNC 求值
                options.promise = true;
                // 让错误栈显示我们的脚本名，而不是默认的 eval_script
                options.filename = Some(name);

                // 求值得到 Promise（脚本的完成值），再把 Promise 当 Rust future 等待
                let evaluated = ctx
                    .eval_with_options::<Promise<'_>, _>(code, options)
                    .catch(&ctx);

                // 先取成通用的 `Value`（它带有上下文生命周期，只能在闭包内使用），
                // 再在闭包内转换成调用方要的类型 `T`。
                let settled = match evaluated {
                    Ok(promise) => promise.into_future::<Value<'_>>().await.catch(&ctx),
                    Err(err) => Err(err),
                };

                // 拆掉引擎的完成值外壳，再转换类型
                let unwrapped = match settled {
                    Ok(value) => unwrap_async_eval_result(&ctx, value),
                    Err(err) => Err(stringify_caught_error(&ctx, err)),
                };

                match unwrapped {
                    Ok(value) => T::from_js(&ctx, value)
                        .catch(&ctx)
                        .map_err(|err| stringify_caught_error(&ctx, err)),
                    Err(message) => Err(message),
                }
            })
            .await;

        // 先把 interval 全部停掉，再收尾：interval 永远有新活，放任它在收尾阶段跑
        // 只会把本次运行的副作用继续放大（也无法判断「跑到哪算完」）。
        self.timers.cancel_repeating();

        // 脚本结束后把残留任务跑完：脚本里可能有没被 await 的 Promise（例如
        // 直接调用 sleep 而不等待），drain 一次可以避免它们泄漏到下一次调用。
        //
        // **必须有上限**：`idle()` 会一直推进任务直到「无事可做」，残留的一次性
        // 定时器/`setInterval` 都会让它迟迟不返回 —— 无限等下去会让 `eval()` 永不返回
        // （实测踩过）。脚本真正 await 的任务在 `idle()` 之前就已经等完了
        // （上面的 eval 已经 await 过脚本的完成值），所以这里只是清尾。
        self.drain_idle(IDLE_DRAIN).await;

        // 再清掉所有定时器：`setTimeout`/`setInterval` 只属于本次运行，
        // 残留下来会变成「上一次脚本的回调打到下一次运行里」。
        self.timers.clear_all();

        outcome.map_err(anyhow::Error::msg)
    }
}

/// 拆掉 quickjs-ng 「async eval」的完成值外壳。
///
/// 打开 `EvalOptions::promise` 后，引擎会把整段脚本编译成 **async 函数体**，并在结尾
/// 把脚本的完成值（最后一条表达式语句的值）包成对象 `{ value: <完成值> }` 返回
/// （见 quickjs-ng `quickjs.c` 中 `eval_ret_idx` 的收尾代码）。因此这里始终拆一层：
/// 即使完成值本身是对象，外层那个也一定是外壳，不存在歧义。
fn unwrap_async_eval_result<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
) -> std::result::Result<Value<'js>, String> {
    if !value.is_object() {
        // 理论上不会发生；真出现别的形状就按原值返回
        return Ok(value);
    }

    let object = value.into_object().expect("已确认是对象");
    object
        .get::<_, Value>("value")
        .catch(ctx)
        .map_err(|err| stringify_caught_error(ctx, err))
}

/// 把被捕获的 JS 错误渲染成一行可读信息（含 `Error: <脚本>:<行>:<列> <消息>`）。
///
/// `CaughtError::Value` 这类「抛出的不是 Error 实例」的情况，`Display` 只会给出
/// 一个笼统描述，这里额外提示一下，方便排查脚本里 `throw "字符串"` 的写法。
fn stringify_caught_error(_ctx: &Ctx<'_>, err: CaughtError<'_>) -> String {
    match err {
        // 抛出的不是 Error 实例：若它带 `message`（Rust 侧 `Exception::throw_message`
        // 造出来的就是这种对象），就把消息取出来 —— 否则调用方只看到一句
        // "Exception generated by quickjs"，定位不了「能力名冲突」这类自定义错误。
        CaughtError::Value(value) => {
            if let Some(object) = value.as_object() {
                if let Ok(message) = object.get::<_, String>("message") {
                    return message;
                }
            }
            format!("脚本抛出了非 Error 值：{value:?}")
        }
        other => other.to_string(),
    }
}
