//! 能力扩展机制：使用方通过 [`ScriptExtension`] 把能力注入 `$` / `Clipbeam` 命名空间。
//!
//! # 为什么有这一层
//!
//! core 只负责「跑 JS/TS」这件通用的事：运行时、TS 转译、`$.file` / `$.sleep` / `console` /
//! `TextDecoder` / `TextEncoder`。像「MD5」「Zstd 压缩」「把文本打进当前焦点窗口」这些
//! **业务能力**不该长在引擎里 —— 它们由使用方（ClipBeam 是 [`crate::ScriptExtension`] 的
//! 实现方）决定并提供。
//!
//! # 一个能力要实现什么
//!
//! 1. [`ScriptExtension::register`]：把能力挂到命名空间对象上（core 已建好 `$` / `Clipbeam`）；
//! 2. [`ScriptExtension::spec`]：声明能力的名字与签名。这份声明同时是**文档**和
//!    CodeMirror 补全的数据源，因此必须与实际注册保持一致（本 crate 的测试会校验）。
//!
//! 需要复用现成函数的场景：能力本体写成把 `Ctx<'js>` 放在**参数表第一位**的普通函数
//! （`rquickjs` 会自动注入），然后 `Function::new(ctx.clone(), js_xxx)` 挂上去。
//!
//! **不要**捕获 `Ctx` 克隆再放进闭包：JS 函数对象活在上下文里，于是形成
//! 「上下文 → JS 函数 → Rust 闭包 → 上下文」的引用环，引用计数永不为零，
//! 进程退出时 `JS_FreeRuntime` 会因 `gc_obj_list` 非空而 abort。
//!
//! # 注册顺序与重名
//!
//! [`setup`] 先注册 core 基础能力（[`BasicExtension`]），再按传入顺序调用使用方扩展。
//! 后注册者若与已有能力重名会**直接报错**（避免误覆盖 `$.file` / `$.sleep`），
//! 除非该扩展显式 [`ScriptExtension::allow_override`] 返回 `true`。

use std::sync::Arc;

use rquickjs::{Ctx, Exception, FromJs, Function, Object, Result as QjsResult};

use crate::bindings::{self, basic};
use crate::host::{CancelRef, CancellationToken, HostRef, ScriptHost};

/// 命名空间在全局的名字：`Clipbeam`。
pub const NAMESPACE: &str = "Clipbeam";

/// 命名空间的便捷别名：`$`，与 [`NAMESPACE`] 是同一个对象。
pub const NAMESPACE_ALIAS: &str = "$";

/// 内部原语命名空间：`__clipbeam`，只给内置前置脚本 `prelude.js` 用。
pub const INTERNAL_NAMESPACE: &str = "__clipbeam";

/// 一条能力的签名声明。
///
/// 用 `&'static str` 而非 `String`：这是编译期写死的元数据，不需要运行时分配。
/// 需要跨进程/JSON 时由使用方自行转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySpec {
    /// JS 侧的属性名，例如 `md5`（挂点由扩展自己决定，通常是 `$` / `Clipbeam`）。
    pub name: &'static str,
    /// 人类可读的签名，例如 `md5(data: ArrayBuffer) -> string`。
    pub signature: &'static str,
    /// 一句话说明。
    pub doc: &'static str,
}

/// 使用方给引擎注入能力的唯一入口。
pub trait ScriptExtension: Send + Sync + 'static {
    /// 把本扩展的能力挂到命名空间对象 `ns` 上（core 已经建好 `$` / `Clipbeam`）。
    ///
    /// 实现里只做「建 JS 函数并 `ns.set(..)`」；能力本体另行实现（见模块文档）。
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()>;

    /// 声明本扩展提供的能力。默认空：适合只做副作用（例如初始化全局变量）的扩展。
    fn spec(&self) -> Vec<CapabilitySpec> {
        Vec::new()
    }

    /// 允许覆盖已注册的同名能力。
    ///
    /// 默认 `false`：重名即报错。core 的 `$.file` / `$.sleep` 因此不会被无意覆盖。
    fn allow_override(&self) -> bool {
        false
    }
}

/// core 的内置能力：`$.file` / `$.sleep`。
///
/// 之所以由 core 提供：它们不依赖任何业务库（不引 MD5 / zstd），是「脚本能做事」的最小集。
pub struct BasicExtension;

impl ScriptExtension for BasicExtension {
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
        basic::register(ctx, ns)
    }

    fn spec(&self) -> Vec<CapabilitySpec> {
        basic::spec()
    }
}

/// 建命名空间 → 注册内部原语、宿主与取消令牌 → 执行前置脚本 → 按顺序调用扩展。
///
/// 由 [`crate::ScriptRuntime`] 在创建时调用一次。
pub(crate) fn setup<'js>(
    ctx: &Ctx<'js>,
    extensions: &[Arc<dyn ScriptExtension>],
    host: Arc<dyn ScriptHost>,
    cancel: Option<CancellationToken>,
) -> QjsResult<()> {
    let globals = ctx.globals();

    // 宿主只对 Rust 侧可见：能力实现用 bindings::host_ctx 取回，不暴露给脚本。
    ctx.store_userdata(HostRef(host))
        .map_err(|_| throw_with_message(ctx, "宿主句柄存入上下文失败（userdata 正被借用）"))?;

    // 取消令牌同样只给 Rust 侧用（$.sleep 与使用方能力都靠它提前退出）
    if let Some(token) = cancel {
        ctx.store_userdata(CancelRef(token))
            .map_err(|_| throw_with_message(ctx, "取消令牌存入上下文失败（userdata 正被借用）"))?;
    }

    // `Clipbeam`（正式名）与 `$`（便捷别名）指向同一个对象
    let ns = Object::new(ctx.clone())?;
    globals.set(NAMESPACE, ns.clone())?;
    globals.set(NAMESPACE_ALIAS, ns.clone())?;

    // core 基础能力优先注册，使用方扩展随后
    let mut registered = property_names(ctx, &ns)?;
    BasicExtension.register(ctx, &ns)?;
    registered.extend(property_names(ctx, &ns)?);

    for extension in extensions {
        // 各能力**在本次注册开始前**就已存在的属性名。注册动作本身会覆盖同名属性，
        // 因此「覆盖了谁」只能在注册前取快照，不能在注册后再比较。
        let before = property_names(ctx, &ns)?;
        let specs = extension.spec();

        // 前置检查：声明的名字与已有能力冲突（最常见的误覆盖：忘了改名，或想顶掉 core 的能力）。
        if !extension.allow_override() {
            for spec in &specs {
                if registered.contains(spec.name) {
                    return Err(capability_conflict(ctx, spec.name));
                }
            }
        }

        extension.register(ctx, &ns)?;
        let after = property_names(ctx, &ns)?;
        let added: std::collections::BTreeSet<String> =
            after.difference(&before).cloned().collect();

        // 兜底检查：没声明 spec 却覆盖了更早注册的能力。
        // `added` 的定义是「注册后不在 `before` 里的名字」，因此一个覆盖了 core 同名能力的
        // 扩展会把那个名字带进 `added`（名字还在，但**值**已经被换掉了）。
        if !extension.allow_override() {
            if let Some(name) = added.iter().find(|name| registered.contains(*name)) {
                return Err(capability_conflict(ctx, name));
            }
        }

        // 反向检查：声明了却不注册 = 补全会提示一个运行期不存在的方法，属于 bug。
        for spec in &specs {
            if !after.contains(spec.name) {
                return Err(throw_with_message(
                    ctx,
                    &format!(
                        "能力声明与实现不一致：spec 声明了 {} 但 register 没有挂上它",
                        spec.name
                    ),
                ));
            }
        }

        registered.extend(added);
    }

    // 内部原语：给 prelude.js 里的 TextDecoder / TextEncoder / console 用
    let internal = Object::new(ctx.clone())?;
    bindings::text::register(ctx, &internal)?;
    // console 的输出目标由宿主决定（GUI 送前端面板、CLI 写终端），因此这里只挂桥
    internal.set("print", Function::new(ctx.clone(), js_print)?)?;
    globals.set(INTERNAL_NAMESPACE, internal)?;

    Ok(())
}

/// `print(level, text) -> undefined`：把 `prelude.js` 里 `console.*` 的输出交给宿主。
///
/// 为什么走宿主而不是直接写 stdout：GUI 应用的标准流用户看不到，而脚本作者最需要的就是
/// 「我打印的东西去哪了」。宿主决定落点 —— 命令行写终端，GUI 写脚本窗口的 Console 面板。
///
/// 取不到宿主时静默返回：打印失败不该打断脚本。
fn js_print<'js>(ctx: Ctx<'js>, level: String, text: String) -> QjsResult<()> {
    if let Some(host) = bindings::host_ctx(&ctx) {
        host.console(&level, &text);
    }
    Ok(())
}

/// 统一的「能力名冲突」异常，避免错误文案在多处漂移。
fn capability_conflict<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Error {
    throw_with_message(
        ctx,
        &format!("能力名冲突：{name} 已被注册（扩展需显式 allow_override 才能覆盖）"),
    )
}

/// 抛出一个带消息的 JS 错误，并把它从异常槽里**取出来**变成可读的 `Error`。
///
/// 为什么不能直接用 `Exception::throw_message`：它返回的是 `Error::Exception`
/// （异常已经挂在上下文上），用 `Display` 格式化只会得到 `Exception` 或
/// `Exception generated by QuickJS`，调用方看不到我们写的中文说明。
/// `ctx.catch()` 会把挂上去的值取回来，`Debug` 格式（`{:?}`）才会带上消息。
fn throw_with_message<'js>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Error {
    Exception::throw_message(ctx, message);
    let caught = ctx.catch();
    let text = caught
        .as_string()
        .and_then(|text| text.to_string().ok())
        .unwrap_or_else(|| format!("{caught:?}"));
    rquickjs::Error::new_from_js_message("value", "Error", text)
}

/// 取对象的自有属性名集合（`Object::keys` 即 `JS_GetOwnPropertyNames`）。
///
/// 用于重名检测与「声明 vs 实现」校验。**必须转成 Rust 的 `String`**：
/// `rquickjs::String` 是 JS 堆上的字符串句柄，把它留在集合里跨注册动作做比较，
/// 会读到已被回收/搬移的对象（实测表现为注册阶段抛出一个没有消息的异常）。
fn property_names<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
) -> QjsResult<std::collections::BTreeSet<String>> {
    let mut names = std::collections::BTreeSet::new();
    for name in object.keys::<rquickjs::String<'js>>() {
        let name = name?;
        names.insert(String::from_js(ctx, name.into_value())?);
    }
    Ok(names)
}
