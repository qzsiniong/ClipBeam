//! ClipBeam 能力集的端到端测试：`md5` / `base32` / `zstd` / `typeStr` / `confirm`。
//!
//! 这里刻意不复用库内部实现，而是用 Rust 侧独立的依赖（`md-5` / `data-encoding` /
//! `zstandard`）去校验 JS 侧的结果，避免「两边一起错」。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clipbeam_script::{
    ConfirmChoice, HostError, RuntimeOptions, ScriptHost, ScriptRuntime,
};
use clipbeam_scripting::{capabilities, extensions, runtime_options};

/// 测试宿主：记录输出与确认请求；`confirm` 的答案由原子变量控制。
#[derive(Default)]
struct TestHost {
    typed: Mutex<String>,
    confirm_messages: Mutex<Vec<String>>,
    answer: AtomicUsize, // 0 = No, 1 = Yes, 2 = Abort
}

impl TestHost {
    fn with_answer(answer: ConfirmChoice) -> Self {
        let host = Self::default();
        host.set_answer(answer);
        host
    }

    fn set_answer(&self, answer: ConfirmChoice) {
        let code = match answer {
            ConfirmChoice::No => 0,
            ConfirmChoice::Yes => 1,
            ConfirmChoice::Abort => 2,
        };
        self.answer.store(code, Ordering::SeqCst);
    }

    fn typed(&self) -> String {
        self.typed.lock().unwrap().clone()
    }

    fn confirm_messages(&self) -> Vec<String> {
        self.confirm_messages.lock().unwrap().clone()
    }
}

impl ScriptHost for TestHost {
    fn type_str(&self, text: &str, _delay_ms: u64) -> Result<(), HostError> {
        self.typed.lock().unwrap().push_str(text);
        Ok(())
    }

    fn confirm(&self, message: &str) -> Result<ConfirmChoice, HostError> {
        self.confirm_messages.lock().unwrap().push(message.to_string());
        Ok(match self.answer.load(Ordering::SeqCst) {
            1 => ConfirmChoice::Yes,
            2 => ConfirmChoice::Abort,
            _ => ConfirmChoice::No,
        })
    }
}

/// 在系统临时目录里写一个测试用文件，返回路径。
fn write_temp_file(name: &str, data: &[u8]) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clipbeam-scripting-test-{}-{seq}-{name}",
        std::process::id()
    ));
    std::fs::write(&path, data).expect("写入临时文件失败");
    path
}

/// 把路径转成能安全嵌进 JS 字符串的字面量。
fn js_path(path: &std::path::Path) -> String {
    format!(
        "{:?}",
        path.to_str()
            .expect("临时路径不是合法 UTF-8")
            .replace('\\', "\\\\")
    )
}

/// 建一个「装了 ClipBeam 全部能力 + 测试宿主」的运行时。
async fn runtime_with(host: Arc<TestHost>) -> ScriptRuntime {
    ScriptRuntime::with_options(runtime_options().host(host))
        .await
        .expect("创建运行时失败")
}

/// `$.md5` / `$.base32` 与 Rust 侧独立实现一致（含二进制数据）。
#[tokio::test]
async fn digest_and_encoding_match_rust() {
    let payload: Vec<u8> = vec![0x00, 0xff, 0x10, b'a', b'b', b'c', 0x7f, 0x80];
    let path = write_temp_file("hash.bin", &payload);
    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.file({path});
            [$.md5(bytes), $.base32(bytes)]
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(&payload);
    let expected_md5 = hex::encode(hasher.finalize());
    let expected_base32 = data_encoding::BASE32_NOPAD.encode(&payload).to_lowercase();

    assert_eq!(values[0], expected_md5, "MD5 不一致");
    assert_eq!(values[1], expected_base32, "Base32 不一致");
}

/// 已知答案测试：钉死公认值，避免「JS 侧和 Rust 侧一起改错」还能通过。
#[tokio::test]
async fn known_answer_md5_and_base32() {
    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            // "abc" 的字节
            const bytes = new Uint8Array([97, 98, 99]).buffer;
            [$.md5(bytes), $.base32(bytes)]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "900150983cd24fb0d6963f7d28e17f72", "md5(\"abc\")");
    assert_eq!(values[1], "mfrgg", "base32(\"abc\")（RFC4648 无填充、小写）");
}

/// `$.zstd` 切出来的分片拼回去，应当能被标准解码器还原成原文；
/// 同时验证分片大小限制与默认分片大小确实生效。
#[tokio::test]
async fn zstd_chunks_roundtrip_and_respect_chunk_size() {
    // 用 xorshift64 生成高熵载荷，保证压缩后仍然会分成多片
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let payload: Vec<u8> = (0..4096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();

    let path = write_temp_file("zstd.bin", &payload);
    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    // 每片在 JS 侧编码成 base32 回传，避免在测试里处理 ArrayBuffer 数组
    let chunks: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.file({path});
            $.zstd(bytes, 1024).map((chunk) => $.base32(chunk))
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    assert!(chunks.len() >= 2, "4096 字节不可压缩数据应当切成多片");

    let mut compressed = Vec::new();
    for chunk in &chunks {
        let raw = data_encoding::BASE32_NOPAD
            .decode(chunk.to_uppercase().as_bytes())
            .expect("base32 解码失败");
        assert!(raw.len() <= 1024, "分片超过了请求的 1024 字节");
        compressed.extend_from_slice(&raw);
    }

    let restored = zstandard::decode_all(&compressed).expect("zstd 解码失败");
    assert_eq!(restored, payload, "解压结果与原文不一致");
}

/// 省略 `chunkSize` 时按默认值（1024）切片。
#[tokio::test]
async fn zstd_default_chunk_size_is_1024() {
    let payload: Vec<u8> = (0..3072).map(|i| (i % 251) as u8).collect();
    let path = write_temp_file("zstd-default.bin", &payload);
    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    let sizes: Vec<usize> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.file({path});
            $.zstd(bytes).map((chunk) => chunk.byteLength)
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    assert!(!sizes.is_empty(), "应当至少有一片");
    assert!(
        sizes.iter().all(|size| *size <= 1024),
        "每片都不应超过 1024 字节：{sizes:?}"
    );
}

/// `$.typeStr` 把文本交给宿主，`delayMs` 一并透传（这里只验证文本）。
#[tokio::test]
async fn type_str_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    runtime
        .eval::<()>(r#"$.typeStr("hello 世界\n", 0);"#)
        .await
        .expect("脚本执行失败");

    assert_eq!(host.typed(), "hello 世界\n");
}

/// `$.confirm` 的三值语义：Yes → true、No → false、Abort → 抛异常。
#[tokio::test]
async fn confirm_semantics() {
    let yes_host = Arc::new(TestHost::with_answer(ConfirmChoice::Yes));
    let runtime = runtime_with(yes_host.clone()).await;
    let value: bool = runtime
        .eval(r#"await $.confirm("继续吗？")"#)
        .await
        .expect("脚本执行失败");
    assert!(value, "Yes 应当映射成 true");
    assert_eq!(yes_host.confirm_messages(), vec!["继续吗？".to_string()]);

    let no_host = Arc::new(TestHost::with_answer(ConfirmChoice::No));
    let runtime = runtime_with(no_host).await;
    let value: bool = runtime
        .eval(r#"await $.confirm("继续吗？")"#)
        .await
        .expect("脚本执行失败");
    assert!(!value, "No 应当映射成 false");

    let abort_host = Arc::new(TestHost::with_answer(ConfirmChoice::Abort));
    let runtime = runtime_with(abort_host).await;
    let message: String = runtime
        .eval(
            r#"
            try {
                await $.confirm("继续吗？");
                "没有抛错"
            } catch (err) {
                err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");
    assert!(message.contains("已中止"), "Abort 应当抛中止异常：{message}");
}

/// 宿主返回 `Cancelled` 时，`$.typeStr` 抛异常终止脚本。
#[tokio::test]
async fn cancelled_host_aborts_type_str() {
    /// 第一次调用就报「已取消」的宿主。
    struct CancellingHost;

    impl ScriptHost for CancellingHost {
        fn type_str(&self, _text: &str, _delay_ms: u64) -> Result<(), HostError> {
            Err(HostError::Cancelled)
        }
        fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
            Ok(ConfirmChoice::No)
        }
        fn cancelled(&self) -> bool {
            true
        }
    }

    let runtime = ScriptRuntime::with_options(runtime_options().host(Arc::new(CancellingHost)))
        .await
        .expect("创建运行时失败");

    let err = runtime
        .eval::<()>(r#"$.typeStr("x"); console.log("不该执行到这里");"#)
        .await
        .expect_err("取消应当让脚本失败");

    assert!(err.to_string().contains("已中止"), "错误信息应说明中止：{err}");
}

/// 没有注入宿主（`RuntimeOptions` 默认的 `NoopHost`）时，交互能力给出可读的错误，
/// 而不是静默什么都不做。
#[tokio::test]
async fn interaction_without_host_reports_wiring_error() {
    let runtime = ScriptRuntime::with_options(runtime_options())
        .await
        .expect("创建运行时失败");

    let message: String = runtime
        .eval(
            r#"
            try {
                $.typeStr("x");
                "没有抛错"
            } catch (err) {
                err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(
        message.contains("不支持"),
        "错误信息应当说明环境不支持：{message}"
    );

    // `$.confirm` 走同一条路（NoopHost 返回 Unsupported → 脚本可捕获的异常）
    let confirm_error: String = runtime
        .eval(
            r#"
            try {
                await $.confirm("x");
                "没有抛错"
            } catch (err) {
                err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");
    assert!(
        confirm_error.contains("不支持"),
        "confirm 也应当报环境不支持：{confirm_error}"
    );
}

/// JS 示例脚本端到端跑通：读文件 → md5 → zstd 分片 → 逐个 `$.confirm` 询问。
///
/// `$.confirm` 回答「否」时脚本会 `continue` 跳过分片的输出，因此不会真的等待/敲字，
/// 测试可以在毫秒级跑完（同时验证了三项能力真的串起来了）。
#[tokio::test]
async fn quick_start_seed_script_runs_end_to_end() {
    let host = Arc::new(TestHost::with_answer(ConfirmChoice::No));
    let runtime = runtime_with(host.clone()).await;

    // 示例读的是 'README.md'（按进程工作目录解析）；测试里换成必定存在的 Cargo.toml
    let source = include_str!("../seed/01-quick-start.js").replace("'README.md'", "'Cargo.toml'");

    runtime
        .run_named_script("01-quick-start.js", &source)
        .await
        .expect("示例脚本应当跑通");

    let typed = host.typed();
    assert!(typed.contains("MD5:"), "应当输出整文件摘要：{typed}");
    assert!(typed.contains("片"), "应当输出分片数量：{typed}");
    assert!(
        !host.confirm_messages().is_empty(),
        "应当对每个分片发起确认（跳过分支也应被触发）"
    );
}

/// TS 示例脚本端到端跑通（转译 → 执行 → 能力可用）。
#[tokio::test]
async fn typescript_seed_script_runs_end_to_end() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    // 示例读的是 'README.md'（按进程工作目录解析）；测试里换成必定存在的 Cargo.toml
    let source = include_str!("../seed/02-ts-demo.ts").replace("'README.md'", "'Cargo.toml'");
    let js = clipbeam_scripting::ts::transpile(&source, std::path::Path::new("02-ts-demo.ts"))
        .expect("示例 TS 应当能转译");

    runtime
        .run_named_script("02-ts-demo.ts", &js)
        .await
        .expect("示例脚本应当跑通");

    assert!(
        host.typed().contains("======="),
        "应当输出预览分隔线：{}",
        host.typed()
    );
}

/// 能力清单必须与运行期注册一致（声明的名字都能在 `$` 上找到）。
#[tokio::test]
async fn capability_list_matches_runtime() {
    let runtime = runtime_with(Arc::new(TestHost::default())).await;
    let caps = capabilities();

    assert!(
        caps.iter().any(|cap| cap.name == "zstd"),
        "能力清单应当包含扩展能力"
    );

    for cap in caps {
        let present: bool = runtime
            .eval(&format!("typeof $.{} === 'function'", cap.name))
            .await
            .expect("脚本执行失败");
        assert!(present, "清单声明了 {} 但运行期不存在", cap.name);
        assert!(
            !cap.signature.is_empty() && !cap.doc.is_empty(),
            "{} 的签名与说明不应为空",
            cap.name
        );
    }
}

/// 扩展列表是稳定的（顺序变化会导致能力清单与注册顺序不一致）。
#[test]
fn extensions_are_listed() {
    let extensions = extensions();
    assert_eq!(extensions.len(), 5, "应当有 5 个使用方扩展");
    let names: Vec<String> = extensions
        .iter()
        .flat_map(|extension| extension.spec())
        .map(|spec| spec.name.to_string())
        .collect();
    assert_eq!(
        names,
        vec!["md5", "base32", "zstd", "typeStr", "confirm"],
        "扩展顺序应当稳定"
    );
}

/// `runtime_options()` 与手工组装的配置等价（避免两条入口漂移）。
#[test]
fn runtime_options_helper_has_all_extensions() {
    let options: RuntimeOptions = runtime_options();
    assert_eq!(options.extensions.len(), 5);
    assert_eq!(RuntimeOptions::default().extensions.len(), 0);
}
