//! 标准全局 `performance.now()` 与 `structuredClone()`。
//!
//! 两个都是"脚本作者默认应该有"的东西，实现都很薄，但缺了会让人用奇怪的替代写法
//! （例如用 `Date.now()` 度量耗时 —— 它会被系统时间调整影响）。
//!
//! ## `performance.now()`
//!
//! 单调时钟：返回**相对引擎创建时刻**的毫秒数（带小数）。用它度量两段代码之间的耗时，
//! 不会因为用户改了系统时间而出现负数或跳变。
//!
//! ## `structuredClone(value)`
//!
//! **JSON 语义**的深拷贝：能丢 `Object` / `Array` / 原始值 / `null`。
//! `Date` / `Map` / `Set` / `RegExp` / 循环引用**明确抛异常**并说明支持范围 ——
//! 静默把 `Date` 变成字符串、把循环引用丢掉，比报错危险得多。

use std::time::Instant;

use rquickjs::{Ctx, Exception, Function, Object, Result as QjsResult};

/// 记录 `performance.now()` 的零点（进程内第一次注册时）。
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// 把 `performance` 与 `structuredClone` 挂到全局对象上。
pub fn register<'js>(ctx: &Ctx<'js>) -> QjsResult<()> {
    let globals = ctx.globals();

    let performance = Object::new(ctx.clone())?;
    performance.set("now", Function::new(ctx.clone(), js_now)?)?;
    globals.set("performance", performance)?;

    globals.set(
        "structuredClone",
        Function::new(ctx.clone(), js_structured_clone)?,
    )?;
    Ok(())
}

/// `performance.now() -> number`
fn js_now() -> f64 {
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_secs_f64() * 1000.0
}

/// `structuredClone(value) -> any`
///
/// 走 JSON 往返；`JSON.stringify` 对不支持的值（函数、`undefined`、循环引用）会失败或静默丢字段，
/// 所以这里先做显式检查再拷贝。
fn js_structured_clone<'js>(
    ctx: Ctx<'js>,
    value: rquickjs::Value<'js>,
) -> QjsResult<rquickjs::Value<'js>> {
    if let Some(object) = value.as_object() {
        // `Object` 同时覆盖普通对象、数组与类实例；用构造器名字识别不支持的类型
        if let Some(name) = constructor_name(&ctx, object) {
            if !matches!(name.as_str(), "Object" | "Array") {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!(
                        "structuredClone 只支持普通对象与数组（JSON 语义），不支持 {name}；\
                         请手动拷贝需要的内容"
                    ),
                ));
            }
        }
    }

    let json = ctx.json_stringify(value)?.ok_or_else(|| {
        Exception::throw_message(
            &ctx,
            "structuredClone 只支持 JSON 可表示的值（不支持函数 / undefined / 循环引用）",
        )
    })?;
    let text = json.to_string()?;
    ctx.json_parse(text)
}

/// 取对象的构造器名（用于判断是不是普通对象/数组）。
fn constructor_name<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Option<String> {
    if object.is_array() {
        return Some("Array".to_string());
    }
    let ctor: rquickjs::Value = object.get("constructor").ok()?;
    let function = ctor.as_function()?;
    let name: rquickjs::Value = function.get("name").ok()?;
    let name = name.as_string()?;
    let _ = ctx;
    name.to_string().ok()
}
