//! `$.scan_qr()`：屏幕扫描扩展。
//!
//! 这是「脚本读不到的那一块」——QuickJS 里没有屏幕访问，所以截屏 + 二维码解码
//! 只能由宿主实现。本能力把它的**接口压到最小**：
//!
//! * **只截一次、只解一次**，返回画面里第一条能解出来的二维码文本，没有就返回 `null`；
//! * **不认识任何协议**：返回的是原始字符串，是 JSON、是 URL、还是别的什么，由脚本自己判断；
//! * **不做轮询、不做区域选择、不做超时**：节奏与超时是脚本的事（`await sleep()` 自己排），
//!   只有脚本作者知道该等多久。
//!
//! 于是它不只为文件传输服务：任何「让远程屏幕显示二维码、本机读回来」的脚本都能用。
//!
//! 宿主不支持屏幕访问时（命令行 / headless）返回 `null` 而不是报错 —— 与
//! `$.pick_path` 在没有选择界面时返回 `null` 是同一个思路：让脚本自己决定怎么降级。

use rquickjs::{Ctx, Result as QjsResult, Value};

use crate::extensions::{extension, require_host, throw_host_error};

extension! {
    /// `$.scan_qr`。
    pub struct ScanQrExtension;
    async "scan_qr" => js_scan_qr,
        "scan_qr() -> Promise<string | null>",
        "截屏一次并解出画面中的二维码文本；没扫到或当前环境不支持屏幕访问时返回 null";
}

/// `scan_qr() -> Promise<string | null>`
///
/// 宿主自己实现截屏与解码（GUI 宿主用 xcap + rqrr，命令行宿主落到默认实现）。
/// 返回 `null` 的两种情形在脚本侧**语义相同**：「这次没读到东西，该重试或降级」。
async fn js_scan_qr<'js>(ctx: Ctx<'js>) -> QjsResult<Value<'js>> {
    let host = require_host(&ctx, "$.scan_qr")?;

    let found = host.scan_qr().map_err(|err| throw_host_error(&ctx, err))?;

    // 显式造 `null`：rquickjs 把 `Option::None` 映射成 `undefined`，而「没扫到」在
    // JS 侧的语义是 `null`（`code === null` 才是判断那个写法）
    match found {
        Some(text) => Ok(rquickjs::String::from_str(ctx.clone(), &text)?.into_value()),
        None => Ok(Value::new_null(ctx.clone())),
    }
}
