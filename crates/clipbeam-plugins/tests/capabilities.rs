//! `$plugin` 能力的端到端测试：权限、参数校验、结果映射。
//!
//! 断言策略与 `clipbeam-scripting/tests/capabilities.rs` 一致：**不复用库内部实现**，
//! 而是用假宿主（[`clipbeam_plugins::test_support::FakePluginHost`]）把「送出去的请求」
//! 记下来，再逐项比对。这样「能力真的把参数原样送到了宿主」「权限真的被拦住了」
//! 都能在没有 Tauri、没有图形环境的机器上验证。

use std::sync::Arc;

use clipbeam_plugins::test_support::FakePluginHost;
use clipbeam_plugins::{
    DialogButtons, DialogChoice, Feedback, FeedbackOutcome, PermissionSet, PluginHost, PluginMeta,
    PluginRuntime, PluginRuntimeOptions, ToastLevel, TrayRequest,
};

// ── 脚手架 ──────────────────────────────────────────────────────────────────

/// 一个插件身份（目录不存在也没关系：能力测试不读入口文件）。
fn meta(id: &str) -> PluginMeta {
    PluginMeta {
        id: id.to_string(),
        name: format!("{id} 显示名"),
        version: "0.1.0".to_string(),
        description: Some("测试用".to_string()),
        author: None,
        dir: std::path::PathBuf::from(format!("/tmp/{id}")),
        entry: "index.js".to_string(),
    }
}

/// 建一个运行时 + 假宿主。
async fn runtime_with(permissions: PermissionSet) -> (PluginRuntime, Arc<FakePluginHost>) {
    let host = Arc::new(FakePluginHost::new(meta("demo")));
    let runtime = clipbeam_plugins::create_runtime(
        host.clone() as Arc<dyn PluginHost>,
        PluginRuntimeOptions::new(meta("demo")).permissions(permissions),
    )
    .await
    .expect("创建插件运行时失败");
    (runtime, host)
}

/// 全部权限都打开（测试某个能力自身语义时用）。
fn all_permissions() -> PermissionSet {
    PermissionSet {
        feedback: true,
        notification: true,
        system_dialog: true,
        tray: true,
        window: true,
    }
}

/// 跑一段 JS，把**抛出的错误消息**取回来（不抛错时返回 `"没有抛错"`）。
///
/// 用**顶层** try/catch（而不是包一层 async IIFE）：引擎取的是「脚本的完成值」，
/// 而一个 async 函数的返回值是 Promise 对象 —— 那样拿到的是 `[object Promise]`。
/// 顶层 await 会把完成值收定为字符串本身。
async fn eval_error(runtime: &PluginRuntime, code: &str) -> String {
    let body = format!(
        r#"
        try {{
          {code}
          "没有抛错"
        }} catch (err) {{
          String(err && err.message ? err.message : err)
        }}
        "#
    );
    runtime
        .runtime()
        .eval::<String>(&body)
        .await
        .expect("求值本身失败")
}

// ── 权限 ────────────────────────────────────────────────────────────────────

/// 默认（不声明任何权限）下，每个能力都必须拒绝，且文案指出该改清单哪一项。
#[tokio::test]
async fn every_capability_is_denied_without_declaration() {
    let (runtime, _host) = runtime_with(PermissionSet::NONE).await;

    let cases: [(&str, &str, &str); 6] = [
        (r#"$plugin.toast("hi")"#, "feedback", "$plugin.toast"),
        (
            r#"$plugin.notify("标题")"#,
            "notification",
            "$plugin.notify",
        ),
        (
            r#"await $plugin.alert("内容")"#,
            "system_dialog",
            "$plugin.alert",
        ),
        (
            r#"await $plugin.confirm("内容")"#,
            "system_dialog",
            "$plugin.confirm",
        ),
        (
            r#"$plugin.tray.onAction(() => {})"#,
            "tray",
            "$plugin.tray.onAction",
        ),
        (
            r#"$plugin.tray.setTooltip("x")"#,
            "tray",
            "$plugin.tray.setTooltip",
        ),
    ];

    for (code, permission, capability) in cases {
        let message = eval_error(&runtime, code).await;
        assert!(
            message.contains("未声明"),
            "{capability} 应当报权限错误：{message}"
        );
        assert!(
            message.contains(permission),
            "{capability} 的错误应当带权限名 {permission}：{message}"
        );
        assert!(
            message.contains(capability),
            "{capability} 的错误应当带能力名：{message}"
        );
    }
}

// ── toast ───────────────────────────────────────────────────────────────────

/// `toast` 默认等级 info、不阻塞；等级与时长原样送达宿主。
#[tokio::test]
async fn toast_reaches_host_with_level_and_duration() {
    let (runtime, host) = runtime_with(PermissionSet {
        feedback: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(
            r#"
            $plugin.toast("默认等级")
            $plugin.toast("成功", { level: "success", durationMs: 0 })
            $plugin.toast("警告", { level: "warning" })
            $plugin.toast("错误", { level: "error" })
            "#,
        )
        .await
        .expect("调用 toast 失败");

    assert_eq!(
        host.toasts(),
        vec![
            (ToastLevel::Info, "默认等级".to_string()),
            (ToastLevel::Success, "成功".to_string()),
            (ToastLevel::Warning, "警告".to_string()),
            (ToastLevel::Error, "错误".to_string()),
        ]
    );

    // 时长原样送达
    let requests = host.feedbacks();
    assert_eq!(
        requests[1],
        Feedback::Toast {
            level: ToastLevel::Success,
            message: "成功".into(),
            duration_ms: Some(0),
        }
    );
    assert_eq!(
        requests[2],
        Feedback::Toast {
            level: ToastLevel::Warning,
            message: "警告".into(),
            duration_ms: None,
        }
    );
}

/// 未知等级 / 空文案要报错（而不是静默当成 info 或弹一条空提示）。
#[tokio::test]
async fn toast_validates_its_arguments() {
    let (runtime, host) = runtime_with(PermissionSet {
        feedback: true,
        ..PermissionSet::NONE
    })
    .await;

    let message = eval_error(&runtime, r#"$plugin.toast("x", { level: "warn" })"#).await;
    assert!(message.contains("warn"), "应当指出非法取值：{message}");
    assert!(message.contains("warning"), "应当列出可用取值：{message}");

    let message = eval_error(&runtime, r#"$plugin.toast("   ")"#).await;
    assert!(message.contains("不能为空"), "{message}");

    assert!(host.feedbacks().is_empty(), "非法调用不该送到宿主");
}

// ── 系统通知 ────────────────────────────────────────────────────────────────

/// 系统通知：title 必填、body 可省略（空串按省略处理）。
#[tokio::test]
async fn notify_reaches_host() {
    let (runtime, host) = runtime_with(PermissionSet {
        notification: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(
            r#"
            $plugin.notify("只要标题")
            $plugin.notify("有正文", "正文内容")
            $plugin.notify("空正文", "")
            "#,
        )
        .await
        .expect("调用 notify 失败");

    assert_eq!(
        host.feedbacks(),
        vec![
            Feedback::Notify {
                title: "只要标题".into(),
                body: None
            },
            Feedback::Notify {
                title: "有正文".into(),
                body: Some("正文内容".into())
            },
            Feedback::Notify {
                title: "空正文".into(),
                body: None
            },
        ]
    );

    let message = eval_error(&runtime, r#"$plugin.notify(" ")"#).await;
    assert!(message.contains("不能为空"), "{message}");
}

// ── 对话框 ──────────────────────────────────────────────────────────────────

/// `confirm` 的三值语义：主按钮 true、次按钮 false、关闭/超时/第三个按钮 null。
#[tokio::test]
async fn confirm_maps_choices_to_boolean_or_null() {
    let (runtime, host) = runtime_with(PermissionSet {
        system_dialog: true,
        ..PermissionSet::NONE
    })
    .await;

    let cases = [
        (DialogChoice::Primary, "true"),
        (DialogChoice::Secondary, "false"),
        (DialogChoice::Tertiary, "null"),
        (DialogChoice::Dismissed, "null"),
        (DialogChoice::Timeout, "null"),
    ];

    for (choice, expected) in cases {
        host.set_dialog_answer(FeedbackOutcome::Chosen { choice });
        let got: String = runtime
            .runtime()
            .eval(r#"String(await $plugin.confirm("继续吗？"))"#)
            .await
            .expect("调用 confirm 失败");
        assert_eq!(got, expected, "{choice:?} 应当映射成 {expected}");
    }

    // 请求原样送达：默认按钮是 okCancel，标题可省
    assert_eq!(
        host.feedbacks().last().unwrap(),
        &Feedback::Dialog {
            title: None,
            message: "继续吗？".into(),
            buttons: DialogButtons::OkCancel,
            timeout_ms: None,
        }
    );
}

/// `alert` 只要一个「好」，且任何回答都不影响它（返回 void）。
#[tokio::test]
async fn alert_uses_a_single_button() {
    let (runtime, host) = runtime_with(PermissionSet {
        system_dialog: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(r#"await $plugin.alert("注意", { title: "标题", timeoutMs: 500 })"#)
        .await
        .expect("调用 alert 失败");

    assert_eq!(
        host.feedbacks(),
        vec![Feedback::Dialog {
            title: Some("标题".into()),
            message: "注意".into(),
            buttons: DialogButtons::Ok,
            timeout_ms: Some(500),
        }]
    );
}

/// 自定义按钮：字符串简写与对象写法都要认；写错要当场报错。
#[tokio::test]
async fn dialog_buttons_accept_shorthand_and_custom_labels() {
    let (runtime, host) = runtime_with(PermissionSet {
        system_dialog: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(
            r#"
            await $plugin.confirm("a", { buttons: "ok" })
            await $plugin.confirm("b", { buttons: "okCancel" })
            await $plugin.confirm("c", { buttons: { primary: "发送", secondary: "取消", tertiary: "稍后" } })
            "#,
        )
        .await
        .expect("调用 confirm 失败");

    let buttons: Vec<DialogButtons> = host
        .feedbacks()
        .into_iter()
        .filter_map(|request| match request {
            Feedback::Dialog { buttons, .. } => Some(buttons),
            _ => None,
        })
        .collect();

    assert_eq!(buttons[0], DialogButtons::Ok);
    assert_eq!(buttons[1], DialogButtons::OkCancel);
    assert_eq!(
        buttons[2],
        DialogButtons::Custom {
            primary: "发送".into(),
            secondary: Some("取消".into()),
            tertiary: Some("稍后".into()),
        }
    );

    let message = eval_error(
        &runtime,
        r#"await $plugin.confirm("x", { buttons: "yesNo" })"#,
    )
    .await;
    assert!(message.contains("yesNo"), "应当指出非法值：{message}");
    assert!(message.contains("okCancel"), "应当列出可用取值：{message}");

    let message = eval_error(&runtime, r#"await $plugin.confirm("x", { buttons: {} })"#).await;
    assert!(message.contains("primary"), "应当要求 primary：{message}");
}

/// 空消息要报错。
#[tokio::test]
async fn dialog_validates_message() {
    let (runtime, _host) = runtime_with(PermissionSet {
        system_dialog: true,
        ..PermissionSet::NONE
    })
    .await;

    let message = eval_error(&runtime, r#"await $plugin.confirm("")"#).await;
    assert!(message.contains("不能为空"), "{message}");
}

// ── 托盘 ────────────────────────────────────────────────────────────────────

/// `tray.onAction` 登记的回调可以被派发唤醒，载荷原样送到。
#[tokio::test]
async fn tray_action_callback_receives_payload() {
    let (runtime, _host) = runtime_with(PermissionSet {
        tray: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            globalThis.got = [];
            $plugin.tray.onAction((action) => {
              globalThis.got.push(action.id + ":" + action.n);
            });
            "#,
        )
        .await
        .expect("跑入口失败");

    runtime
        .eval_action("first", serde_json::json!({ "id": "first", "n": 1 }))
        .await
        .expect("派发失败");
    runtime
        .eval_action("second", serde_json::json!({ "id": "second", "n": 2 }))
        .await
        .expect("派发失败");

    let got: String = runtime
        .runtime()
        .eval("globalThis.got.join('|')")
        .await
        .expect("取回记录失败");
    assert_eq!(got, "first:1|second:2");
}

/// async 回调也会被等到（引擎的求值层负责 await）。
#[tokio::test]
async fn tray_action_callback_can_be_async() {
    let (runtime, _host) = runtime_with(PermissionSet {
        tray: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            globalThis.done = false;
            $plugin.tray.onAction(async (action) => {
              await sleep(20);
              globalThis.done = true;
              $plugin.tray.setTooltip("处理完 " + action.id);
            });
            "#,
        )
        .await
        .expect("跑入口失败");

    runtime
        .eval_action("x", serde_json::json!({ "id": "x" }))
        .await
        .expect("派发失败");

    let done: bool = runtime
        .runtime()
        .eval("globalThis.done")
        .await
        .expect("取回状态失败");
    assert!(done, "async 回调应当被等到");
}

/// 重复调用 `onAction`：以后者为准（插件重载/重新注册的语义）。
#[tokio::test]
async fn tray_action_reregistration_replaces_the_callback() {
    let (runtime, _host) = runtime_with(PermissionSet {
        tray: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            globalThis.got = [];
            $plugin.tray.onAction(() => { globalThis.got.push("first") });
            $plugin.tray.onAction(() => { globalThis.got.push("second") });
            "#,
        )
        .await
        .expect("跑入口失败");

    runtime
        .eval_action("x", serde_json::json!({ "id": "x" }))
        .await
        .expect("派发失败");

    let got: String = runtime
        .runtime()
        .eval("globalThis.got.join(',')")
        .await
        .expect("取回记录失败");
    assert_eq!(got, "second", "后登记的应当覆盖前一个");
}

/// 托盘运行时控制：tooltip / badge 原样送达宿主。
#[tokio::test]
async fn tray_runtime_controls_reach_host() {
    let (runtime, host) = runtime_with(PermissionSet {
        tray: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(
            r#"
            $plugin.tray.setTooltip("正在处理")
            $plugin.tray.setBadge("3")
            $plugin.tray.setBadge(null)
            "#,
        )
        .await
        .expect("调用托盘控制失败");

    assert_eq!(
        host.trays(),
        vec![
            TrayRequest::SetTooltip {
                text: "正在处理".into()
            },
            TrayRequest::SetBadge {
                text: Some("3".into())
            },
            TrayRequest::SetBadge { text: None },
        ]
    );

    let message = eval_error(&runtime, r#"$plugin.tray.setTooltip("  ")"#).await;
    assert!(message.contains("不能为空"), "{message}");
}

// ── 宿主侧错误 ──────────────────────────────────────────────────────────────

/// 宿主返回错误时，JS 侧收到的是宿主写的说明（而不是「Exception generated by quickjs」）。
#[tokio::test]
async fn host_errors_surface_with_their_message() {
    let (runtime, host) = runtime_with(PermissionSet {
        feedback: true,
        ..PermissionSet::NONE
    })
    .await;

    host.set_feedback_error(Some(clipbeam_plugins::PluginError::Failed(
        "托管窗口已经不在了".into(),
    )));

    let message = eval_error(&runtime, r#"$plugin.toast("hi")"#).await;
    assert!(message.contains("托管窗口已经不在了"), "{message}");
}

/// 宿主不支持某能力时，错误文案说的是「环境不支持」而不是「权限不够」。
#[tokio::test]
async fn unsupported_capabilities_have_their_own_message() {
    let (runtime, _host) = runtime_with(PermissionSet {
        tray: true,
        ..PermissionSet::NONE
    })
    .await;

    // 假宿主的 `feedback` 是支持的；这里换成不支持 `tray` 的宿主验证文案
    struct OnlyFeedback(FakePluginHost);

    impl PluginHost for OnlyFeedback {
        fn meta(&self) -> &PluginMeta {
            self.0.meta()
        }
        fn feedback(
            &self,
            request: Feedback,
        ) -> Result<FeedbackOutcome, clipbeam_plugins::PluginError> {
            self.0.feedback(request)
        }
    }

    let host = Arc::new(OnlyFeedback(FakePluginHost::new(meta("demo"))));
    let runtime_unsupported = clipbeam_plugins::create_runtime(
        host as Arc<dyn PluginHost>,
        PluginRuntimeOptions::new(meta("demo")).permissions(PermissionSet {
            tray: true,
            ..PermissionSet::NONE
        }),
    )
    .await
    .expect("创建运行时失败");

    let message = eval_error(&runtime_unsupported, r#"$plugin.tray.setBadge("x")"#).await;
    assert!(message.contains("不支持"), "{message}");
    let _ = runtime;
}

// ── 插件身份 ────────────────────────────────────────────────────────────────

/// 全量权限下四个能力都能用（`all_permissions` 本身也要被用上，避免它变成死代码）。
#[tokio::test]
async fn all_capabilities_work_together() {
    let (runtime, host) = runtime_with(all_permissions()).await;

    runtime
        .run_entry(
            "index.js",
            r#"
            $plugin.tray.onAction(async (action) => {
              $plugin.toast("收到 " + action.id, { level: "success" });
              $plugin.notify("ClipBeam", action.id);
              await $plugin.alert("处理完了");
            });
            "#,
        )
        .await
        .expect("跑入口失败");

    host.set_dialog_answer(FeedbackOutcome::Chosen {
        choice: DialogChoice::Primary,
    });

    runtime
        .eval_action("sync", serde_json::json!({ "id": "sync" }))
        .await
        .expect("派发失败");

    assert_eq!(host.toasts().len(), 1);
    assert!(matches!(
        host.feedbacks().iter().find(|r| matches!(r, Feedback::Notify { .. })),
        Some(Feedback::Notify { title, body }) if title == "ClipBeam" && body.as_deref() == Some("sync")
    ));
    assert!(host
        .feedbacks()
        .iter()
        .any(|r| matches!(r, Feedback::Dialog { .. })));
}

// ── 窗口（$plugin.window）──────────────────────────────────────────────────

/// 未声明 `window` 权限时，五个窗口能力都要拒绝。
#[tokio::test]
async fn window_capabilities_are_denied_without_permission() {
    let (runtime, _host) = runtime_with(PermissionSet::NONE).await;

    let cases: [(&str, &str); 5] = [
        (r#"$plugin.window.open({})"#, "$plugin.window.open"),
        (
            r#"$plugin.window.post("w1", { a: 1 })"#,
            "$plugin.window.post",
        ),
        (
            r#"$plugin.window.onMessage("w1", () => {})"#,
            "$plugin.window.onMessage",
        ),
        (
            r#"$plugin.window.onClosed("w1", () => {})"#,
            "$plugin.window.onClosed",
        ),
        (r#"$plugin.window.close("w1")"#, "$plugin.window.close"),
    ];

    for (code, capability) in cases {
        let message = eval_error(&runtime, code).await;
        assert!(
            message.contains("未声明"),
            "{capability} 应当报权限错误：{message}"
        );
        assert!(message.contains("window"), "应当指出权限名：{message}");
        assert!(message.contains(capability), "应当指出能力名：{message}");
    }
}

/// `open` 把窗口请求送到宿主，并把宿主给的标签原样回给插件。
#[tokio::test]
async fn window_open_reaches_host_and_returns_the_label() {
    let (runtime, host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            globalThis.wins = [];
            globalThis.wins.push($plugin.window.open({ title: "面板", width: 400, page: "ui.html" }));
            globalThis.wins.push($plugin.window.open());
            "#,
        )
        .await
        .expect("跑入口失败");

    let requests = host.windows();
    assert_eq!(requests.len(), 2, "两次 open 应当都到宿主");

    match &requests[0] {
        clipbeam_plugins::WindowRequest::Open { window_id, options } => {
            assert_eq!(options.title.as_deref(), Some("面板"));
            assert_eq!(options.width, Some(400.0));
            assert_eq!(options.page.as_deref(), Some("ui.html"));
            assert!(!window_id.is_empty(), "宿主侧应当拿到窗口 id");
        }
        other => panic!("应当是 Open：{other:?}"),
    }

    // 两个窗口的 id 必须不同（重名会让回调互相覆盖）
    let ids: Vec<String> = runtime
        .runtime()
        .eval("globalThis.wins.map((w) => w.id)")
        .await
        .expect("取回窗口 id 失败");
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(ids.len(), 2, "两次 open 应当得到两个窗口：{ids:?}");
    assert_eq!(unique.len(), 2, "窗口 id 不该重复：{ids:?}");

    // 返回值里有 label / seq（插件不需要自己拼标签，但能看到宿主的决定）
    let has_label: bool = runtime
        .runtime()
        .eval("typeof globalThis.wins[0].label === 'string' && globalThis.wins[0].label.length > 0")
        .await
        .expect("求值失败");
    assert!(has_label, "应当回一个非空 label");
}

/// `post` / `close` 通过**窗口事件**这条通道到宿主，而不是窗口请求。
#[tokio::test]
async fn window_post_and_close_reach_the_host() {
    let (runtime, host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    let win_id: String = runtime
        .runtime()
        .eval(
            r#"
            const win = $plugin.window.open();
            $plugin.window.post(win.id, { text: "你好", list: [1, 2] });
            $plugin.window.close(win.id);
            win.id
            "#,
        )
        .await
        .expect("调用窗口能力失败");

    let notices = host.notices();
    assert_eq!(notices.len(), 1, "post 应当发一条窗口消息：{notices:?}");
    match &notices[0] {
        clipbeam_plugins::WindowNotice::Message { window_id, message } => {
            assert_eq!(window_id, &win_id);
            let payload = message.as_object().expect("载荷应当是对象");
            assert_eq!(payload["windowId"], win_id);
            assert_eq!(payload["message"]["text"], "你好");
            assert_eq!(payload["message"]["list"][1], 2);
        }
        other => panic!("应当是 Message：{other:?}"),
    }

    let requests = host.windows();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            clipbeam_plugins::WindowRequest::Close { window_id } if window_id == &win_id
        )),
        "close 应当到宿主：{requests:?}"
    );
}

/// `undefined` 载荷：**按 JS 语义**让 `message` 字段缺失，而不是报错、也不是变成 null。
///
/// 这是刻意的：`post(id, undefined)` 与 `post(id, null)` 在 JS 里是两件事，
/// 插件不该因为传了 undefined 就被打断；页面侧用 `"message" in event.data` 区分。
#[tokio::test]
async fn window_post_keeps_undefined_distinct_from_null() {
    let (runtime, host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .runtime()
        .eval::<()>(
            r#"
            const w = $plugin.window.open();
            $plugin.window.post(w.id, undefined);
            $plugin.window.post(w.id, null);
            "#,
        )
        .await
        .expect("post 不该因为 undefined 报错");

    let notices = host.notices();
    assert_eq!(notices.len(), 2, "两次 post 都应当送到：{notices:?}");

    let payload_of = |notice: &clipbeam_plugins::WindowNotice| match notice {
        clipbeam_plugins::WindowNotice::Message { message, .. } => {
            message.as_object().expect("载荷应当是对象").clone()
        }
        other => panic!("应当是 Message：{other:?}"),
    };

    let from_undefined = payload_of(&notices[0]);
    assert!(
        !from_undefined.contains_key("message"),
        "undefined 应当让 message 字段缺失：{from_undefined:?}"
    );

    let from_null = payload_of(&notices[1]);
    assert!(
        from_null
            .get("message")
            .is_some_and(serde_json::Value::is_null),
        "null 应当序列化成 null：{from_null:?}"
    );
}

/// 窗口回调登记进动作表：宿主能按 `window:<id>:message` / `:closed` 找到它们。
#[tokio::test]
async fn window_callbacks_are_registered_under_window_scoped_keys() {
    let (runtime, _host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            const win = $plugin.window.open();
            globalThis.got = [];
            $plugin.window.onMessage(win.id, (msg) => { globalThis.got.push("msg:" + msg.n) });
            $plugin.window.onClosed(win.id, () => { globalThis.got.push("closed") });
            globalThis.winId = win.id;
            "#,
        )
        .await
        .expect("跑入口失败");

    let keys = runtime.registered_action_keys().await;
    let win_id: String = runtime
        .runtime()
        .eval("globalThis.winId")
        .await
        .expect("取回窗口 id 失败");

    assert!(
        keys.contains(&format!("window:{win_id}:message")),
        "消息回调应当登记在窗口作用域键下：{keys:?}"
    );
    assert!(
        keys.contains(&format!("window:{win_id}:closed")),
        "关闭回调应当登记在窗口作用域键下：{keys:?}"
    );

    // 宿主按这个键唤醒 → 回调真的被调用，且载荷原样送达
    runtime
        .eval_action(
            &format!("window:{win_id}:message"),
            serde_json::json!({ "n": 7 }),
        )
        .await
        .expect("唤醒消息回调失败");
    runtime
        .eval_action(&format!("window:{win_id}:closed"), serde_json::json!({}))
        .await
        .expect("唤醒关闭回调失败");

    let got: String = runtime
        .runtime()
        .eval("globalThis.got.join('|')")
        .await
        .expect("取回记录失败");
    assert_eq!(got, "msg:7|closed");
}

/// 同一个窗口的 `onMessage` 重复登记：以后者为准（与托盘 onAction 同一语义）。
#[tokio::test]
async fn window_on_message_reregistration_replaces() {
    let (runtime, _host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            const win = $plugin.window.open();
            globalThis.got = [];
            $plugin.window.onMessage(win.id, () => { globalThis.got.push("first") });
            $plugin.window.onMessage(win.id, () => { globalThis.got.push("second") });
            globalThis.winId = win.id;
            "#,
        )
        .await
        .expect("跑入口失败");

    let win_id: String = runtime.runtime().eval("globalThis.winId").await.unwrap();
    runtime
        .eval_action(&format!("window:{win_id}:message"), serde_json::json!({}))
        .await
        .expect("唤醒失败");

    let got: String = runtime
        .runtime()
        .eval("globalThis.got.join(',')")
        .await
        .unwrap();
    assert_eq!(got, "second", "后登记的应当覆盖前一个");
}

/// 两个窗口各自的消息回调互不干扰（窗口是按 id 分流的）。
#[tokio::test]
async fn window_callbacks_are_isolated_per_window() {
    let (runtime, _host) = runtime_with(PermissionSet {
        window: true,
        ..PermissionSet::NONE
    })
    .await;

    runtime
        .run_entry(
            "index.js",
            r#"
            const a = $plugin.window.open();
            const b = $plugin.window.open();
            globalThis.got = [];
            $plugin.window.onMessage(a.id, () => { globalThis.got.push("a") });
            $plugin.window.onMessage(b.id, () => { globalThis.got.push("b") });
            globalThis.a = a.id;
            globalThis.b = b.id;
            "#,
        )
        .await
        .expect("跑入口失败");

    let a: String = runtime.runtime().eval("globalThis.a").await.unwrap();
    runtime
        .eval_action(&format!("window:{a}:message"), serde_json::json!({}))
        .await
        .expect("唤醒失败");

    let got: String = runtime
        .runtime()
        .eval("globalThis.got.join(',')")
        .await
        .unwrap();
    assert_eq!(got, "a", "只该触发那个窗口的回调");
}
