//! 能力扩展机制：使用方通过 [`ScriptExtension`] 把能力注入能力命名空间。
//!
//! # 为什么有这一层
//!
//! 引擎只负责「跑 JS/TS」这件通用的事：运行时、TS 转译、`console` / `TextDecoder` /
//! `TextEncoder` / `sleep` 等标准全局。像「MD5」「Zstd 压缩」「写文件」
//! 「把文本打进当前焦点窗口」这些
//! **业务能力**不该长在引擎里 —— 它们由使用方（宿主应用）以 [`crate::ScriptExtension`]
//! 的形式提供。
//!
//! # 命名空间的名字由使用方配置
//!
//! 引擎创建一个**匿名**命名空间对象，并把「叫什么名字」交给 [`crate::RuntimeOptions`]
//! （[`crate::RuntimeOptions::namespace`] / [`crate::RuntimeOptions::namespace_alias`]）：
//! `MyTool` / `$` / `cb` 这些名字的决定权完全在使用方。引擎自己不含任何业务名字。
//!
//! 挂上去的两个绑定是**不可写、不可配置**的，命名空间对象在所有扩展注册完会被
//! `Object.freeze` —— 能力集合到此定型，脚本无法用 `$.md5 = null` 这类写法自伤。
//!
//! # 一个能力要实现什么
//!
//! 1. [`ScriptExtension::register`]：把能力挂到传进来的命名空间对象上；
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
//! [`setup`] 按传入顺序调用使用方扩展，最后冻结命名空间。
//! 后注册者若与已注册能力重名会**直接报错**，除非该扩展显式
//! [`ScriptExtension::allow_override`] 返回 `true`。

use std::sync::Arc;

use rquickjs::prelude::Async;
use rquickjs::{Ctx, Exception, FromJs, Function, Object, Result as QjsResult};

use crate::bindings;
use crate::cancel::CancelRef;
use crate::console_hook::ConsoleRef;
use crate::runtime::RuntimeOptions;

/// 内部原语命名空间：`__primitives`，引导阶段给内置前置脚本 `prelude.js` 用。
///
/// prelude 执行完就从这个全局上删掉（见 [`crate::runtime`]），所以脚本看不到它。
/// 名字里刻意不带任何产品名：它是引擎的内部实现细节。
pub(crate) const PRIMITIVES_NAMESPACE: &str = "__primitives";

/// prelude 稍后会挂到全局的名字。
///
/// 校验命名空间名字时要连它们一起挡：setup 阶段这些全局还没注册，
/// 否则 `.namespace("console")` 会先绑上、再被 prelude 覆盖掉。
const PRELUDE_GLOBALS: [&str; 3] = ["TextDecoder", "TextEncoder", "console"];

/// 一条能力的签名声明。
///
/// 用 `&'static str` 而非 `String`：这是编译期写死的元数据，不需要运行时分配。
/// 需要跨进程/JSON 时由使用方自行转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySpec {
    /// JS 侧的属性名，例如 `md5`（挂点由扩展自己决定，通常是能力命名空间）。
    pub name: &'static str,
    /// 人类可读的签名，例如 `md5(data: ArrayBuffer) -> string`。
    pub signature: &'static str,
    /// 一句话说明。
    pub doc: &'static str,
}

/// 使用方给引擎注入能力的唯一入口。
pub trait ScriptExtension: Send + Sync + 'static {
    /// 把本扩展的能力挂到命名空间对象 `ns` 上（引擎已经建好并注册好全局名字）。
    ///
    /// 实现里只做「建 JS 函数并 `ns.set(..)`」；能力本体另行实现（见模块文档）。
    fn register<'js>(&self, ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()>;

    /// 声明本扩展提供的能力。默认空：适合只做副作用（例如初始化全局变量）的扩展。
    fn spec(&self) -> Vec<CapabilitySpec> {
        Vec::new()
    }

    /// 允许覆盖已注册的同名能力。
    ///
    /// 默认 `false`：重名即报错（两个扩展用同一个名字是最常见的失误）。
    fn allow_override(&self) -> bool {
        false
    }
}

/// 建命名空间 → 注册标准全局与内部原语 → 调用扩展 → 冻结命名空间。
///
/// 由 [`crate::ScriptRuntime`] 在创建时调用一次。顺序上有三处是刻意的：
///
/// 1. **标准全局先注册**：命名空间名字的冲突检查才能看见它们（避免 `.namespace("sleep")`）；
/// 2. **扩展按顺序注册**：重名冲突与「声明了却没注册」两项校验都在这一步；
/// 3. **最后 `Object.freeze`**：冻结只能发生在所有扩展注册完之后。
pub(crate) fn setup<'js>(
    ctx: &Ctx<'js>,
    options: &RuntimeOptions,
) -> QjsResult<Arc<bindings::timers::Timers>> {
    let globals = ctx.globals();

    // 控制台钩子只对 Rust 侧可见：prelude.js 的 console.* 经 __primitives.print 用它，
    // 不往 JS 侧暴露任何对象。
    ctx.store_userdata(ConsoleRef(options.console.clone()))
        .map_err(|_| throw_with_message(ctx, "控制台钩子存入上下文失败（userdata 正被借用）"))?;

    // 取消信号同样只给 Rust 侧用（sleep 与使用方能力都靠它提前退出）
    if let Some(signal) = options.cancel.clone() {
        ctx.store_userdata(CancelRef(signal))
            .map_err(|_| throw_with_message(ctx, "取消信号存入上下文失败（userdata 正被借用）"))?;
    }

    // 使用方自己的上下文初始化（例如存入宿主句柄）。放在扩展注册**之前**：
    // 扩展的 register 里就要能取到宿主。
    if let Some(prepare) = &options.prepare {
        prepare(ctx)?;
    }

    // 标准全局：定时器、sleep、atob/btoa、performance、structuredClone
    // （前四个是「浏览器/Node 都有」的东西，缺了脚本作者会踩空；都不依赖任何外部能力）
    let timers = bindings::timers::register(ctx)?;
    globals.set(
        "sleep",
        Function::new(ctx.clone(), Async(bindings::timer::sleep))?,
    )?;
    bindings::atob::register(ctx)?;
    bindings::std_global::register(ctx)?;

    // 匿名能力命名空间：引擎只负责建，名字（如果有）来自使用方配置
    let ns = Object::new(ctx.clone())?;
    attach_namespace(ctx, &ns, options)?;

    let mut registered = property_names(ctx, &ns)?;

    for extension in &options.extensions {
        // 各能力**在本次注册开始前**就已存在的属性名。注册动作本身会覆盖同名属性，
        // 因此「覆盖了谁」只能在注册前取快照，不能在注册后再比较。
        let before = property_names(ctx, &ns)?;
        let specs = extension.spec();

        // 前置检查：声明的名字与已有能力冲突（最常见的是两个扩展用了同一个名字）。
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
        // `added` 的定义是「注册后不在 `before` 里的名字」，因此一个覆盖了同名能力的
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

    // 能力集合到此定型：冻结命名空间，脚本不能替换/删除能力（`$.md5 = null` 这类自伤会被挡掉）。
    freeze_namespace(ctx, &ns)?;

    // 内部原语：给 prelude.js 里的 TextDecoder / TextEncoder / console 用。
    // prelude 会把它们关进自己的闭包，然后由 runtime 把这个全局删掉。
    let internal = Object::new(ctx.clone())?;
    bindings::text::register(ctx, &internal)?;
    // console 的输出目标由宿主决定（GUI 送前端面板、CLI 写终端），因此这里只挂桥
    internal.set("print", Function::new(ctx.clone(), js_print)?)?;
    globals.set(PRIMITIVES_NAMESPACE, internal)?;

    Ok(timers)
}

/// 按 [`RuntimeOptions`] 的配置把匿名命名空间挂成全局。
///
/// 校验（任一不过就带着原因启动失败，绝不静默降级）：
///
/// * 名字必须是合法 JS 标识符（否则脚本没法访问，属于配置错误）；
/// * 与已有全局冲突时报错（含内置对象与 prelude 稍后会挂的那几个），**不做覆盖**；
/// * 正式名与别名不能同名。
///
/// 挂上去的绑定是**不可写、不可配置**的：脚本不能把命名空间指向别的东西
/// （命名空间对象本身在此之前保持可扩展，扩展注册完才冻结）。
fn attach_namespace<'js>(
    ctx: &Ctx<'js>,
    ns: &Object<'js>,
    options: &RuntimeOptions,
) -> QjsResult<()> {
    // 使用方没配名字：引擎自己不需要名字（引擎侧测试、纯嵌入场景就是这样）
    let Some(namespace) = options.namespace.as_deref() else {
        return Ok(());
    };

    validate_global_name(ctx, "RuntimeOptions::namespace", namespace)?;

    // 别名可省略、可为空串（都表示「不要别名」）
    let alias = options
        .namespace_alias
        .as_deref()
        .filter(|alias| !alias.is_empty());
    if let Some(alias) = alias {
        validate_global_name(ctx, "RuntimeOptions::namespace_alias", alias)?;
        if alias == namespace {
            return Err(throw_with_message(
                ctx,
                &format!(
                    "RuntimeOptions：namespace 与 namespace_alias 不能同名（都是 {namespace:?}）"
                ),
            ));
        }
    }

    let mut names = vec![namespace];
    if let Some(alias) = alias {
        names.push(alias);
    }

    // 先全部查重、再全部挂上：避免「挂了一个才报错」留下半个状态
    let globals = ctx.globals();
    for name in &names {
        if globals.contains_key(*name)? || PRELUDE_GLOBALS.contains(name) {
            return Err(throw_with_message(
                ctx,
                &format!("全局名 {name:?} 已被占用（内置对象或引擎已提供的全局），换一个能力命名空间名字"),
            ));
        }
    }
    for name in &names {
        define_readonly_global(ctx, name, ns)?;
    }
    Ok(())
}

/// 名字必须是合法 JS 标识符，否则脚本根本写不出来 —— 属于配置错误，直接报错。
fn validate_global_name<'js>(ctx: &Ctx<'js>, what: &str, name: &str) -> QjsResult<()> {
    let mut chars = name.chars();
    let valid = match chars.next() {
        Some(first) => {
            (first.is_ascii_alphabetic() || first == '_' || first == '$')
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        }
        None => false,
    };
    if !valid {
        return Err(throw_with_message(
            ctx,
            &format!("{what}({name:?}) 不是合法的 JS 标识符"),
        ));
    }
    Ok(())
}

/// 挂一个**不可写、不可配置**的全局绑定。
///
/// `rquickjs` 0.14 没有 `Object::define_property`，所以这里用一小段 JS 助手来完成 ——
/// 助手函数本身只活在本次调用里，不挂到任何全局。
fn define_readonly_global<'js>(ctx: &Ctx<'js>, name: &str, value: &Object<'js>) -> QjsResult<()> {
    const DEFINE_READONLY: &str = "(function (name, value) {          Object.defineProperty(globalThis, name, {              value: value, writable: false, configurable: false,          });      })";
    let define: Function = ctx.eval(DEFINE_READONLY)?;
    define.call::<_, ()>((name, value.clone()))
}

/// 冻结命名空间对象：能力集合到此定型（属性不可增删改）。
fn freeze_namespace<'js>(ctx: &Ctx<'js>, ns: &Object<'js>) -> QjsResult<()> {
    let freeze: Function = ctx.eval("Object.freeze")?;
    freeze.call::<_, ()>((ns.clone(),))
}

/// `print(level, text) -> undefined`：把 `prelude.js` 里 `console.*` 的输出交给控制台钩子。
///
/// 为什么不让引擎直接写 stdout：GUI 应用的标准流用户看不到，而脚本作者最需要的就是
/// 「我打印的东西去哪了」。落点由使用方决定 —— 命令行写终端，GUI 写脚本窗口的 Console 面板。
///
/// 取不到钩子时静默返回：打印失败不该打断脚本。
fn js_print<'js>(ctx: Ctx<'js>, level: String, text: String) -> QjsResult<()> {
    if let Some(console) = bindings::console_hook(&ctx) {
        console.write(&level, &text);
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
