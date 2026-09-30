//! `ScriptRuntime::eval_global` 的行为测试。
//!
//! 这个方法是给「宿主攒着回调、之后反复唤醒」用的（插件托盘动作就是第一个用户），
//! 因此这里钉死四类契约：
//!
//! 1. **载荷**以 JSON 形态完整送达（含中文、换行、引号、嵌套结构）；
//! 2. **重复调用**可用（每次都是同一份上下文里再调一次）；
//! 3. 名字不存在 / 不是函数 → **报错**，绝不静默 no-op；
//! 4. 回调抛异常 → 报错里带脚本名与回调名（跟着错误栈定位）。
//!
//! 另外钉一个容易被忽略的细节：载荷用的**临时全局**（`__clipbeam_eval_<n>`）在调用后
//! 必须被删掉 —— 否则它会长期挂在脚本的全局对象上，变成一个没人认领的副作用。

use script_engine::ScriptRuntime;

/// 建一个只做纯计算的运行时（不注入宿主）。
async fn runtime() -> ScriptRuntime {
    ScriptRuntime::new().await.expect("创建运行时失败")
}

/// 载荷里的特殊字符必须原样送达：JSON 文本被塞进 JS 字符串字面量，
/// 中文 / 换行 / 引号 / 反斜杠都不能被转义规则吃掉。
#[tokio::test]
async fn payload_survives_special_characters() {
    let runtime = runtime().await;
    runtime
        .eval::<()>("globalThis.capture = (value) => { globalThis.seen = value }")
        .await
        .expect("注册回调失败");

    let payload = serde_json::json!({
        "text": "中文与 emoji 🎯",
        "quotes": "他说：\"引号\" 与 '单引号'",
        "backslash": "C:\\temp\\x",
        "newline": "第一行\n第二行",
        "nested": { "list": [1, 2.5, null, true, "结尾"] },
    });

    runtime
        .eval_global("capture", payload.clone())
        .await
        .expect("调用回调失败");

    // 在 JS 侧重新序列化再比字符串：避免依赖 Rust 侧对对象键顺序的假设
    let seen: String = runtime
        .eval("JSON.stringify(globalThis.seen)")
        .await
        .expect("取回载荷失败");

    let expected = serde_json::to_string(&payload).expect("序列化载荷失败");
    assert_eq!(seen, expected, "载荷应当逐字节等价地送达 JS 侧");
}

/// 回调可以是 `async`：返回 Promise 时宿主必须等它跑完（这里等的是它写入的副作用）。
#[tokio::test]
async fn async_callback_is_awaited() {
    let runtime = runtime().await;
    runtime
        .eval::<()>(
            r#"
            globalThis.slow = async (payload) => {
              await sleep(30);
              globalThis.result = payload.id + ":" + payload.n;
            }
            "#,
        )
        .await
        .expect("注册回调失败");

    runtime
        .eval_global("slow", serde_json::json!({ "id": "job", "n": 7 }))
        .await
        .expect("调用回调失败");

    let result: String = runtime.eval("globalThis.result").await.expect("取结果失败");
    assert_eq!(result, "job:7", "async 回调的副作用应当在返回前完成");
}

/// 同一个回调可以反复叫醒（托盘菜单点第二次、第三次…）。
#[tokio::test]
async fn callback_can_be_called_repeatedly() {
    let runtime = runtime().await;
    runtime
        .eval::<()>(
            r#"
            globalThis.count = 0;
            globalThis.onAction = (action) => { globalThis.count += 1; globalThis.last = action.id };
            "#,
        )
        .await
        .expect("注册回调失败");

    for id in ["first", "second", "third"] {
        runtime
            .eval_global("onAction", serde_json::json!({ "id": id }))
            .await
            .expect("调用回调失败");
    }

    let count: i32 = runtime.eval("globalThis.count").await.expect("取计数失败");
    let last: String = runtime.eval("globalThis.last").await.expect("取末次失败");
    assert_eq!(count, 3, "三次调用都应当生效");
    assert_eq!(last, "third", "最后一次载荷应当覆盖前一次");
}

/// 全局不存在：报错，且错误信息里带上名字（否则宿主只知道"失败"不知道"谁失败"）。
#[tokio::test]
async fn missing_global_is_an_error() {
    let runtime = runtime().await;

    let err = runtime
        .eval_global("notRegistered", serde_json::json!({}))
        .await
        .expect_err("未注册的回调必须报错");

    let text = err.to_string();
    assert!(
        text.contains("notRegistered"),
        "错误信息应当指出是哪个回调：{text}"
    );
    assert!(
        text.contains("不是函数"),
        "错误信息应当说明原因（typeof undefined）：{text}"
    );
}

/// 同名但不是函数（脚本里写了个变量顶着这个名字）：同样报错，不要假装调过了。
#[tokio::test]
async fn non_function_global_is_an_error() {
    let runtime = runtime().await;
    runtime
        .eval::<()>("globalThis.notAFunction = 42")
        .await
        .expect("赋值失败");

    let err = runtime
        .eval_global("notAFunction", serde_json::json!({}))
        .await
        .expect_err("非函数全局必须报错");

    let text = err.to_string();
    assert!(text.contains("notAFunction"), "错误信息应当带名字：{text}");
    assert!(
        text.contains("typeof number"),
        "错误信息应当带实际类型：{text}"
    );
}

/// 回调抛异常：错误要冒出来（而不是被吞掉），并且带上脚本名与回调名便于定位。
#[tokio::test]
async fn callback_error_propagates_with_location() {
    let runtime = runtime().await;
    runtime
        .eval::<()>(r#"globalThis.boom = () => { throw new Error("故意炸") }"#)
        .await
        .expect("注册回调失败");

    let err = runtime
        .eval_global("boom", serde_json::json!({}))
        .await
        .expect_err("回调抛异常必须冒到宿主");

    let text = err.to_string();
    assert!(text.contains("故意炸"), "应当保留原始错误信息：{text}");
    assert!(
        text.contains("boom"),
        "错误位置应当指向回调名字（便于排查）：{text}"
    );
}

/// 载荷用的临时全局必须在调用后消失（成功与失败两条路径都要清理）。
#[tokio::test]
async fn temporary_payload_global_is_cleaned_up() {
    let runtime = runtime().await;
    runtime
        .eval::<()>(r#"globalThis.ok = () => {}; globalThis.bad = () => { throw new Error("x") }"#)
        .await
        .expect("注册回调失败");

    runtime
        .eval_global("ok", serde_json::json!({ "a": 1 }))
        .await
        .expect("调用成功路径失败");
    let _ = runtime
        .eval_global("bad", serde_json::json!({ "a": 1 }))
        .await;

    let leftovers: Vec<String> = runtime
        .eval(
            r#"Object.getOwnPropertyNames(globalThis).filter((name) => name.startsWith("__clipbeam_eval_"))"#,
        )
        .await
        .expect("取全局名失败");

    assert!(
        leftovers.is_empty(),
        "临时载荷全局没有被清理：{leftovers:?}"
    );
}

/// 载荷里的引号/换行不能破坏生成的代码（这条路走的是 JSON 字符串字面量，必须真的安全）。
#[tokio::test]
async fn payload_cannot_break_out_of_the_generated_code() {
    let runtime = runtime().await;
    runtime
        .eval::<()>("globalThis.capture = (value) => { globalThis.seen = value }")
        .await
        .expect("注册回调失败");

    // 尝试注入的载荷：即使被拼进代码，也只能作为字符串内容存在
    let nasty = r#""); globalThis.hacked = true; (""#;
    runtime
        .eval_global("capture", serde_json::json!({ "text": nasty }))
        .await
        .expect("调用回调失败");

    let hacked: bool = runtime
        .eval("globalThis.hacked === true")
        .await
        .expect("取注入标记失败");
    let seen: String = runtime
        .eval("globalThis.seen.text")
        .await
        .expect("取回载荷失败");

    assert!(!hacked, "载荷不应该能跳出字符串字面量");
    assert_eq!(seen, nasty, "载荷应当原样送达");
}

/// `null` 载荷也是合法输入（表示"没有数据"），不能因此报错。
#[tokio::test]
async fn null_payload_is_accepted() {
    let runtime = runtime().await;
    runtime
        .eval::<()>("globalThis.check = (value) => { globalThis.got = value === null }")
        .await
        .expect("注册回调失败");

    runtime
        .eval_global("check", serde_json::Value::Null)
        .await
        .expect("null 载荷应当被接受");

    let got: bool = runtime.eval("globalThis.got").await.expect("取结果失败");
    assert!(got, "JS 侧应当收到 null");
}
