//! 标准全局定时器：`setTimeout` / `clearTimeout` / `setInterval` / `clearInterval`。
//!
//! quickjs-ng 没有定时器（标准 `qjs` 的那些来自 quickjs-libc，不在这里链接），而
//! 「延迟执行 / 周期执行」是脚本的常见需求：`$.sleep` 只能让**脚本自己**等，不能调度回调。
//!
//! # 为什么不用 `Async(...)` 包装
//!
//! `rquickjs` 的 `Async` 会把返回值包成 `Promise`，于是 `setTimeout` 返回的是 Promise 而不是 id ——
//! `const id = setTimeout(...); clearTimeout(id)` 直接失效（实测踩过）。
//! 定时器必须**同步返回 id**，所以这里用 `Ctx::spawn` 把等待逻辑交给引擎自己的调度器：
//! 它由 `ScriptRuntime` 的 `idle()` 驱动，与 `$.sleep` 走同一条路，且不需要捕获 `Ctx` 克隆。
//!
//! # 生命周期
//!
//! * 每个定时器登记在 [`Timers`] 表里（一个取消标志）；
//! * 等待按 25ms 切片，逐片检查取消标志，因此 `clearTimeout` 与「脚本结束时的统一清理」
//!   都能让任务很快退出；
//! * `ScriptRuntime` 在每次脚本执行结束（`idle()` 之后）调用 [`Timers::clear_all`]，
//!   避免上一次运行的回调打到下一次。
//!
//! 说明：回调参数按 `setTimeout(cb, ms?)` 支持；`setTimeout(cb, ms, a, b)` 的额外参数暂不支持。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Function, JsLifetime, Result as QjsResult};

use super::console_hook;

/// 取消检查与等待切片的粒度：最坏情况下多等这么久才退出。
const SLICE: Duration = Duration::from_millis(25);

/// 定时器表：随上下文存活，`ScriptRuntime` 另存一份用于收尾清理。
#[derive(Default)]
pub struct Timers {
    entries: Mutex<Vec<Entry>>,
    next_id: AtomicU64,
}

/// 一个已登记的定时器。
struct Entry {
    id: u64,
    cancel: Arc<AtomicBool>,
    /// 是否重复触发（`setInterval`）：脚本结束时它们必须先停，否则
    /// 「永远有新活」的 interval 会在收尾的 `idle()` 里一直跑下去。
    repeating: bool,
}

impl Timers {
    /// 登记一个定时器，返回它的 id（从 1 开始递增）。
    fn insert(&self, cancel: Arc<AtomicBool>, repeating: bool) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.entries.lock().unwrap().push(Entry {
            id,
            cancel,
            repeating,
        });
        id
    }

    /// 注销（一次性定时器跑完，或 `clear*` 之后）。
    fn remove(&self, id: u64) {
        self.entries.lock().unwrap().retain(|entry| entry.id != id);
    }

    /// 取消并移除；返回它是否还在表里（`clearTimeout` 允许重复调用）。
    fn cancel(&self, id: u64) -> bool {
        let cancel = {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.cancel.clone())
        };
        match cancel {
            Some(cancel) => {
                cancel.store(true, Ordering::SeqCst);
                self.remove(id);
                true
            }
            None => false,
        }
    }

    /// 停掉所有 `setInterval`，但保留一次性 `setTimeout`。
    ///
    /// 在收尾的 `idle()` **之前**调用：脚本返回后，interval 永远有新活会让收尾阶段
    /// 一直跑它的回调（等于把上一次运行的副作用带进收尾阶段）；一次性定时器则是有界的，
    /// 让它们在收尾阶段触发完更贴近「脚本里忘了 await 的一次性任务仍会执行」的直觉。
    pub fn cancel_repeating(&self) {
        let entries = self.entries.lock().unwrap();
        for entry in entries.iter().filter(|entry| entry.repeating) {
            entry.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// 取消并清空全部定时器（脚本执行结束时调用）。
    pub fn clear_all(&self) {
        let mut entries = self.entries.lock().unwrap();
        for entry in entries.iter() {
            entry.cancel.store(true, Ordering::SeqCst);
        }
        entries.clear();
    }
}

// SAFETY: `Timers` 只装原子量、`Mutex` 与 `Arc`，不含任何带 `'js` 生命周期的 JS 值。
unsafe impl<'js> JsLifetime<'js> for Timers {
    type Changed<'to> = Timers;
}

/// 把四个定时器函数挂到**全局**对象上，并返回共享的定时器表。
///
/// 它们是标准全局而不是 `$.xxx`：脚本作者的心智模型来自浏览器/Node。
pub fn register<'js>(ctx: &Ctx<'js>) -> QjsResult<Arc<Timers>> {
    let globals = ctx.globals();
    let timers = Arc::new(Timers::default());
    ctx.store_userdata(timers.clone()).map_err(|_| {
        Exception::throw_message(ctx, "定时器表存入上下文失败（userdata 正被借用）")
    })?;

    globals.set("setTimeout", Function::new(ctx.clone(), js_set_timeout)?)?;
    globals.set("setInterval", Function::new(ctx.clone(), js_set_interval)?)?;
    globals.set("clearTimeout", Function::new(ctx.clone(), js_clear_timer)?)?;
    globals.set("clearInterval", Function::new(ctx.clone(), js_clear_timer)?)?;
    Ok(timers)
}

/// 取回上下文里的定时器表（克隆 `Arc`，调用方拿到所有权）。
///
/// 上下文里存的是 `Arc<Timers>`，所以查询类型必须与存入类型一致 —— 存/取不匹配会
/// 静默返回 `None`（`userdata` 是按类型精确查找的）。
fn timers_of<'js>(ctx: &Ctx<'js>) -> Option<Arc<Timers>> {
    ctx.userdata::<Arc<Timers>>().map(|guard| guard.clone())
}

/// `clearTimeout(id)` / `clearInterval(id)`：共用一个 id 空间，都幂等。
fn js_clear_timer<'js>(ctx: Ctx<'js>, id: Opt<u64>) {
    if let Some(id) = id.0 {
        if let Some(timers) = timers_of(&ctx) {
            timers.cancel(id);
        }
    }
}

/// `setTimeout(cb, ms?) -> number`
fn js_set_timeout<'js>(
    ctx: Ctx<'js>,
    callback: Function<'js>,
    delay_ms: Opt<u64>,
) -> QjsResult<u64> {
    schedule(ctx, callback, delay_ms.0.unwrap_or(0), false)
}

/// `setInterval(cb, ms?) -> number`
fn js_set_interval<'js>(
    ctx: Ctx<'js>,
    callback: Function<'js>,
    delay_ms: Opt<u64>,
) -> QjsResult<u64> {
    schedule(ctx, callback, delay_ms.0.unwrap_or(0), true)
}

/// 把上下文异常槽里的错误渲染成一行可读文本。
///
/// rquickjs 的 `Error::Exception` 用 `Display` 只会给出 "Exception generated by QuickJS"，
/// 真正的消息在抛出的那个值上（可能是 `Error` 实例，也可能是普通对象/字符串）。
fn caught_message<'js>(ctx: &Ctx<'js>) -> String {
    let caught = ctx.catch();
    if let Some(object) = caught.as_object() {
        if let Ok(message) = object.get::<_, String>("message") {
            return message;
        }
    }
    if let Some(text) = caught.as_string() {
        if let Ok(text) = text.to_string() {
            return text;
        }
    }
    format!("{caught:?}")
}

/// 登记定时器，把等待交给引擎调度器，**同步**返回 id。
fn schedule<'js>(
    ctx: Ctx<'js>,
    callback: Function<'js>,
    delay_ms: u64,
    repeating: bool,
) -> QjsResult<u64> {
    let cancel = Arc::new(AtomicBool::new(false));
    let Some(timers) = timers_of(&ctx) else {
        return Err(Exception::throw_message(
            &ctx,
            "定时器表不存在（运行时未初始化）",
        ));
    };
    let id = timers.insert(cancel.clone(), repeating);

    // 让引擎驱动这段等待（与 $.sleep 同一机制：由 run_named_script 的 idle() 推进）。
    // 用克隆而不是移动 `ctx`：`spawn` 的 future 需要一个独占的 `Ctx`，
    // 而这个函数在返回 id 之后就不再使用它了。
    let timer_ctx = ctx.clone();
    ctx.spawn(async move {
        let delay = Duration::from_millis(delay_ms);
        loop {
            // 分片等待：clearTimeout 与脚本结束时的清理都能及时生效
            let mut left = delay;
            while !left.is_zero() {
                let step = SLICE.min(left);
                tokio::time::sleep(step).await;
                left -= step;
                // 每片之后都查一次：取消**立即**生效，不会在切片边界上多触发一次回调
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
            }
            if cancel.load(Ordering::SeqCst) {
                break;
            }

            // 回调抛错只记录、不打断脚本（与浏览器一致：上报错误后继续）
            if callback.call::<(), ()>(()).is_err() {
                let message = caught_message(&timer_ctx);
                if let Some(console) = console_hook(&timer_ctx) {
                    console.write("error", &format!("定时器回调抛出异常：{message}"));
                }
            }

            if !repeating {
                break;
            }
        }
        if let Some(timers) = timers_of(&timer_ctx) {
            timers.remove(id);
        }
    });

    Ok(id)
}
