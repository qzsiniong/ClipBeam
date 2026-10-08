//! ClipBeam 能力集的端到端测试。
//!
//! 断言策略：**不复用库内部实现**，而是用 Rust 侧独立的依赖（`md-5` / `crc32fast` /
//! `data-encoding` / `flate2` / `zstandard`）校验 JS 侧的结果，再补一组「公认值」
//! （`md5("abc")`、`base32("abc")`…）钉死，避免两边一起改错还能通过。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clipbeam_scripting::{
    capabilities, extensions, runtime_options, ConfirmChoice, FileDecision, HostError, PickKind,
    ScriptHost,
};
use script_engine::{ConsoleHook, RuntimeOptions, ScriptExtension, ScriptRuntime, StdoutConsole};

// ── 测试替身 ────────────────────────────────────────────────────────────────

/// 记录型宿主：输出、确认、选路径、文件授权都可观测，答案由原子量控制。
struct TestHost {
    typed: Mutex<String>,
    confirm_messages: Mutex<Vec<String>>,
    confirm_answer: AtomicUsize, // 0=No 1=Yes 2=Abort
    file_requests: Mutex<Vec<String>>,
    file_answer: Mutex<FileDecision>,
    focus_requests: Mutex<Vec<String>>,
    /// `$.pick_path` 的回答；`None` 表示模拟「用户取消 / 没有界面」。
    pick_answer: Mutex<Option<PathBuf>>,
    /// `$.pick_path` 收到的 `(prompt, kind)`，按调用顺序。
    pick_requests: Mutex<Vec<(String, PickKind)>>,
    /// `$.scan_qr` 的回答队列：每次调用弹一个；队列空时回退到 `qr_sticky`。
    ///
    /// 队列语义是刻意的：脚本会反复扫屏等状态变化，而「先 READY、后 FINISH」这种时序
    /// 只有按次给答案才能模拟。
    qr_answers: Mutex<std::collections::VecDeque<String>>,
    /// 队列空时一直返回的答案（`None` = 没扫到 / 环境不支持屏幕访问）。
    qr_sticky: Mutex<Option<String>>,
    /// `$.scan_qr` 被调用了几次。
    qr_calls: AtomicUsize,
}

impl Default for TestHost {
    fn default() -> Self {
        Self {
            typed: Mutex::new(String::new()),
            confirm_messages: Mutex::new(Vec::new()),
            confirm_answer: AtomicUsize::new(0),
            file_requests: Mutex::new(Vec::new()),
            // 默认放行一次：文件读写测试不该被授权逻辑挡住
            file_answer: Mutex::new(FileDecision::Allow),
            focus_requests: Mutex::new(Vec::new()),
            // 默认「没选到」：不调用 pick_path 的测试不受影响
            pick_answer: Mutex::new(None),
            pick_requests: Mutex::new(Vec::new()),
            // 默认没有二维码可读：等价于「命令行 / 没有屏幕访问」
            qr_answers: Mutex::new(std::collections::VecDeque::new()),
            qr_sticky: Mutex::new(None),
            qr_calls: AtomicUsize::new(0),
        }
    }
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
        self.confirm_answer.store(code, Ordering::SeqCst);
    }

    fn set_file_answer(&self, answer: FileDecision) {
        *self.file_answer.lock().unwrap() = answer;
    }

    fn typed(&self) -> String {
        self.typed.lock().unwrap().clone()
    }

    fn confirm_messages(&self) -> Vec<String> {
        self.confirm_messages.lock().unwrap().clone()
    }

    fn file_requests(&self) -> Vec<String> {
        self.file_requests.lock().unwrap().clone()
    }

    fn focus_requests(&self) -> Vec<String> {
        self.focus_requests.lock().unwrap().clone()
    }

    /// 设定 `$.pick_path` 的回答；传 `None` 模拟用户取消。
    fn set_pick_answer(&self, path: Option<PathBuf>) {
        *self.pick_answer.lock().unwrap() = path;
    }

    fn pick_requests(&self) -> Vec<(String, PickKind)> {
        self.pick_requests.lock().unwrap().clone()
    }
}

impl ScriptHost for TestHost {
    fn type_str(&self, text: &str, _delay_ms: u64) -> Result<(), HostError> {
        self.typed.lock().unwrap().push_str(text);
        Ok(())
    }

    fn confirm(&self, message: &str) -> Result<ConfirmChoice, HostError> {
        self.confirm_messages
            .lock()
            .unwrap()
            .push(message.to_string());
        Ok(match self.confirm_answer.load(Ordering::SeqCst) {
            1 => ConfirmChoice::Yes,
            2 => ConfirmChoice::Abort,
            _ => ConfirmChoice::No,
        })
    }

    fn allow_file_change(
        &self,
        action: &str,
        path: &Path,
        _scope_dir: &Path,
    ) -> Result<FileDecision, HostError> {
        self.file_requests
            .lock()
            .unwrap()
            .push(format!("{action} {}", path.display()));
        Ok(*self.file_answer.lock().unwrap())
    }

    fn request_focus(&self, hint: &str) -> Result<(), HostError> {
        self.focus_requests.lock().unwrap().push(hint.to_string());
        Ok(())
    }

    fn pick_path(&self, prompt: &str, kind: PickKind) -> Result<Option<PathBuf>, HostError> {
        self.pick_requests
            .lock()
            .unwrap()
            .push((prompt.to_string(), kind));
        Ok(self.pick_answer.lock().unwrap().clone())
    }

    /// `$.scan_qr`：先弹队列，队列空时用粘住的答案。
    ///
    /// 默认（队列空 + 没粘住）返回 `None`，与命令行宿主一致 ——
    /// 于是「宿主不支持屏幕访问」这条降级路径在测试里就是默认行为。
    fn scan_qr(&self) -> Result<Option<String>, HostError> {
        self.qr_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = self.qr_answers.lock().unwrap().pop_front() {
            return Ok(Some(answer));
        }
        Ok(self.qr_sticky.lock().unwrap().clone())
    }
}

/// 收集 `console.*` 的落点（避免测试把日志打到 stdout）。
#[derive(Default)]
struct TestConsole {
    lines: Mutex<Vec<(String, String)>>,
}

impl ConsoleHook for TestConsole {
    fn write(&self, level: &str, text: &str) {
        self.lines
            .lock()
            .unwrap()
            .push((level.to_string(), text.to_string()));
    }
}

// ── 测试脚手架 ──────────────────────────────────────────────────────────────

/// 在系统临时目录里写一个测试文件，返回路径。
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

/// 建一个独占的临时目录（每个测试一个，避免互相踩）。
fn temp_dir(name: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clipbeam-scripting-dir-{}-{seq}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建临时目录失败");
    path
}

/// 把路径转成能嵌进 JS 字符串的字面量。
/// 把路径变成可直接嵌进脚本字符串字面量的形式（含引号，已转义）。
///
/// **不要再自己 `.replace('\\', "\\\\")`**：`{:?}`（`str` 的 `Debug`）本身就会转义反斜杠，
/// 再替换一次会**双重转义** —— Windows 上脚本拿到的路径会变成
/// `C:\\Users\\…`（分隔符翻倍）而不是 `C:\Users\…`。
/// Unix 上路径没有反斜杠，所以这个错在那边的表现是「完全看不出来」。
fn js_path(path: &Path) -> String {
    format!("{:?}", path.to_string_lossy())
}

/// 建一个「装了 ClipBeam 全部能力 + 测试宿主」的运行时。
async fn runtime_with(host: Arc<TestHost>) -> ScriptRuntime {
    let console: Arc<dyn ConsoleHook> = Arc::new(TestConsole::default());
    clipbeam_scripting::create_runtime(host, console, None)
        .await
        .expect("创建运行时失败")
}

/// 只需要跑纯计算能力时的快捷方式（宿主不会被用到）。
async fn compute_runtime() -> ScriptRuntime {
    runtime_with(Arc::new(TestHost::default())).await
}

// ── 入参多态 ────────────────────────────────────────────────────────────────

/// 三种入参形态（字符串 / ArrayBuffer / 视图）走同一条字节路径；视图要按
/// `byteOffset` + `byteLength` 切，不能把整个底层 buffer 都算进去。
#[tokio::test]
async fn binary_input_accepts_string_buffer_and_views() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const view = new Uint8Array([0xff, 0x61, 0x62, 0x63, 0xff]);
            const slice = new Uint8Array(view.buffer, 1, 3);   // 中间三个字节 "abc"
            [
              $.hex("abc"),
              $.hex(slice),
              $.hex(slice.buffer.slice(1, 4)),
              $.md5("abc"),
              $.md5(slice),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "616263", "字符串按 UTF-8 编码");
    assert_eq!(values[1], "616263", "视图要按 byteOffset/byteLength 切片");
    assert_eq!(values[2], "616263", "ArrayBuffer 直接按字节");
    assert_eq!(
        values[3], "900150983cd24fb0d6963f7d28e17f72",
        "md5(\"abc\")"
    );
    assert_eq!(values[4], values[3], "视图与字符串结果一致");
}

/// 传入无法解释的类型时报 `TypeError`，而不是静默当成空字节。
#[tokio::test]
async fn binary_input_rejects_other_types() {
    let runtime = compute_runtime().await;

    let messages: Vec<String> = runtime
        .eval(
            r#"
            const out = [];
            for (const value of [42, {}, null, undefined, [1, 2]]) {
              try {
                $.md5(value);
                out.push("没有抛错");
              } catch (err) {
                out.push(err.constructor.name + ":" + err.message);
              }
            }
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    for message in messages {
        assert!(
            message.starts_with("TypeError:"),
            "应当抛 TypeError：{message}"
        );
    }
}

/// `$.bytes` 与 `$.str` 是一对互逆的转换（字符串 → 字节 → 文本）。
#[tokio::test]
async fn bytes_and_str_roundtrip() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const text = "中文与 emoji 🎯\n";
            const bytes = $.bytes(text);
            const plain = $.str(bytes);
            [String(bytes.byteLength), plain === text ? "same" : "diff", bytes.byteLength === new TextEncoder().encode(text).byteLength ? "utf8" : "other"]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[1], "same", "str(bytes(x)) 应当等于 x");
    assert_eq!(values[2], "utf8", "bytes 应当按 UTF-8 编码");
}

/// `$.str` 与 `$.read_text` 都支持非 UTF-8 标签（这里用 GBK 的真实字节验证）。
#[tokio::test]
async fn text_encoding_labels_are_honoured() {
    // Rust 侧生成 GBK 字节：让「按 GBK 解码」这件事无法靠 UTF-8 蒙对
    let (gbk_bytes, _, _) = encoding_rs::GBK.encode("中文");
    let path = write_temp_file("gbk.txt", &gbk_bytes);
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({path});
            [
              $.str(bytes, "gbk"),
              await $.read_text({path}, "gbk"),
              String(bytes.byteLength),
              String($.str(bytes).length),   // 默认 UTF-8：非法序列会变成替换字符
            ]
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    assert_eq!(values[0], "中文", "按 GBK 解码");
    assert_eq!(values[1], "中文", "read_text 也认标签");
    assert_eq!(values[2], "4", "GBK 下「中文」是 4 字节");
}

// ── 编码 ────────────────────────────────────────────────────────────────────

/// 已知答案：互相独立地钉死每个编码器的产物。
#[tokio::test]
async fn encoding_known_answers() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const bytes = new TextEncoder().encode("abc");
            [
              $.hex(bytes),
              $.hex_upper(bytes),
              $.base32(bytes),
              $.base32_nopad(bytes),
              $.base32_lower(bytes),
              $.base32_lower_nopad(bytes),
              $.base64(bytes),
              $.base64_nopad(bytes),
              $.base64_url(new Uint8Array([0xfb, 0xff]).buffer),
              $.base64_url_nopad(new Uint8Array([0xfb, 0xff]).buffer),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "616263");
    assert_eq!(values[1], "616263".to_uppercase());
    assert_eq!(values[2], "MFRGG===");
    assert_eq!(values[3], "MFRGG");
    assert_eq!(values[4], "mfrgg===");
    assert_eq!(values[5], "mfrgg");
    assert_eq!(values[6], "YWJj");
    assert_eq!(values[7], "YWJj", "3 字节不需要填充，两种写法相同");
    assert_eq!(values[8], "-_8=", "URL-safe 盘表把 +/ 换成 -_");
    assert_eq!(values[9], "-_8");
}

/// 每个编码器都有对应的解码器，往返必须闭合；解码结果与原始字节逐字节相同。
#[tokio::test]
async fn encoding_decoders_roundtrip() {
    let payload: Vec<u8> = (0u16..=255).map(|byte| byte as u8).collect();
    let path = write_temp_file("roundtrip.bin", &payload);
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({path});
            const pairs = [
              ["base32", "base32_decode"],
              ["base32_nopad", "base32_nopad_decode"],
              ["base32_lower", "base32_lower_decode"],
              ["base32_lower_nopad", "base32_lower_nopad_decode"],
              ["base64", "base64_decode"],
              ["base64_nopad", "base64_nopad_decode"],
              ["base64_url", "base64_url_decode"],
              ["base64_url_nopad", "base64_url_nopad_decode"],
              ["hex", "hex_decode"],
              ["hex_upper", "hex_decode"],
            ];
            pairs.map(([encode, decode]) => $.hex($[decode]($[encode](bytes))));
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    let expected = hex::encode(&payload);
    for (index, value) in values.iter().enumerate() {
        assert_eq!(value, &expected, "第 {index} 组解码结果与原文不一致");
    }
}

/// 解码器严格按名字校验格式：大小写与填充不匹配都要报错（不静默纠正）。
#[tokio::test]
async fn decoders_are_strict_about_case_and_padding() {
    let runtime = compute_runtime().await;

    let messages: Vec<String> = runtime
        .eval(
            r#"
            const out = [];
            const tryCall = (fn) => {
              try {
                fn();
                out.push("没有抛错");
              } catch (err) {
                out.push(err.message);
              }
            };
            tryCall(() => $.base32_decode("MFRGG"));          // 缺填充
            tryCall(() => $.base32_nopad_decode("MFRGG===")); // 多了填充
            tryCall(() => $.base32_lower_decode("MFRGG===")); // 大写
            tryCall(() => $.base64_nopad_decode("YWJj"));     // 无填充其实合法
            tryCall(() => $.base64_nopad_decode("YWJj=="));   // 多了填充
            tryCall(() => $.base64_url_decode("+/8="));       // 标准盘表
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(
        messages[0].contains("解码失败"),
        "base32_decode：{}",
        messages[0]
    );
    assert!(
        messages[1].contains("解码失败"),
        "base32_nopad_decode：{}",
        messages[1]
    );
    assert!(
        messages[2].contains("不接受大写"),
        "base32_lower_decode：{}",
        messages[2]
    );
    assert_eq!(messages[3], "没有抛错", "无填充的 base64 本来就合法");
    assert!(
        messages[4].contains("解码失败"),
        "base64_nopad_decode 不接受填充：{}",
        messages[4]
    );
    assert!(
        messages[5].contains("解码失败"),
        "base64_url_decode 不接受标准盘表：{}",
        messages[5]
    );
}

/// 与 `src-tauri::protocol` 的盘表约定一致：都是 RFC4648 无填充，只是大小写不同。
///
/// 协议层的 `b32_encode_upper` 就是 `BASE32_NOPAD.encode`（见 protocol.rs），
/// 这里用同一份实现交叉验证，防止脚本侧悄悄换盘表。
#[tokio::test]
async fn base32_nopad_matches_protocol_table() {
    let payload = b"ClipBeam";
    let path = write_temp_file("protocol.bin", payload);
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({path});
            [$.base32_nopad(bytes), $.base32_lower_nopad(bytes)]
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    let upper = data_encoding::BASE32_NOPAD.encode(payload);
    assert_eq!(values[0], upper);
    assert_eq!(values[1], upper.to_ascii_lowercase());
}

// ── 摘要 ────────────────────────────────────────────────────────────────────

/// `$.md5` 与 `md-5` 独立实现一致（含二进制数据）。
#[tokio::test]
async fn md5_matches_rust() {
    let payload: Vec<u8> = vec![0x00, 0xff, 0x10, b'a', b'b', b'c', 0x7f, 0x80];
    let path = write_temp_file("hash.bin", &payload);
    let runtime = compute_runtime().await;

    let value: String = runtime
        .eval(&format!(
            r#"$.md5(await $.read({path}))"#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(&payload);
    assert_eq!(value, hex::encode(hasher.finalize()));
}

/// `$.crc32` 的五种格式：值一致、长度正确、默认是 8 位大写十六进制。
#[tokio::test]
async fn crc32_formats() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const bytes = new TextEncoder().encode("123456789");
            [
              $.crc32(bytes),
              $.crc32(bytes, "hex"),
              $.crc32(bytes, "hex_lower"),
              $.crc32(bytes, "base32"),
              $.crc32(bytes, "base32_lower"),
              $.crc32(bytes, "base64"),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    // 公认值：crc32("123456789") == 0xCBF43926
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(b"123456789");
    let checksum = hasher.finalize();
    assert_eq!(checksum, 0xCBF4_3926, "先确认公认值本身");
    let raw = checksum.to_be_bytes();

    assert_eq!(values[0], "CBF43926", "默认格式");
    assert_eq!(values[1], values[0]);
    assert_eq!(values[2], "cbf43926");
    assert_eq!(values[3], data_encoding::BASE32_NOPAD.encode(&raw));
    assert_eq!(
        values[4],
        data_encoding::BASE32_NOPAD
            .encode(&raw)
            .to_ascii_lowercase()
    );
    assert_eq!(values[5], data_encoding::BASE64_NOPAD.encode(&raw));

    // 长度：8 / 8 / 7 / 7 / 6
    assert_eq!(values[0].len(), 8);
    assert_eq!(values[3].len(), 7);
    assert_eq!(values[5].len(), 6);
}

/// 未知格式名要报错并列出可用取值。
#[tokio::test]
async fn crc32_rejects_unknown_format() {
    let runtime = compute_runtime().await;

    let message: String = runtime
        .eval(
            r#"
            try {
              $.crc32("x", "base16");
              "没有抛错"
            } catch (err) {
              err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(
        message.contains("base16"),
        "错误里应当带上非法取值：{message}"
    );
    assert!(
        message.contains("hex_lower"),
        "错误里应当列出可用取值：{message}"
    );
}

// ── 压缩 ────────────────────────────────────────────────────────────────────

/// `$.zstd` 压缩后的字节能被标准 zstd 解码器还原（只压不解，所以用 Rust 侧解）。
#[tokio::test]
async fn zstd_compresses_for_standard_decoder() {
    let payload = b"clipbeam zstd payload clipbeam zstd payload".repeat(32);
    let path = write_temp_file("zstd-in.bin", &payload);
    let out_path = write_temp_file("zstd-out.bin", b"");
    let runtime = compute_runtime().await;

    runtime
        .eval::<()>(&format!(
            r#"
            const bytes = await $.read({input});
            await $.write({output}, $.zstd(bytes));
            "#,
            input = js_path(&path),
            output = js_path(&out_path)
        ))
        .await
        .expect("脚本执行失败");

    let compressed = std::fs::read(&out_path).expect("读取压缩结果失败");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&out_path);

    assert!(compressed.len() < payload.len(), "重复内容应当被压小");
    let restored = zstandard::decode_all(&compressed[..]).expect("zstd 解码失败");
    assert_eq!(restored, payload);
}

/// gzip / brotli / xz 三对压缩解压往返，并用 Rust 侧实现交叉验证 gzip 的产物。
#[tokio::test]
async fn compression_roundtrips() {
    // 高熵载荷：保证各算法都会真的走压缩路径
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let payload: Vec<u8> = (0..8192)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();

    let path = write_temp_file("compress-in.bin", &payload);
    let gz_path = write_temp_file("compress-out.gz", b"");
    let runtime = compute_runtime().await;

    let results: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({input});
            await $.write({gz}, $.gzip(bytes));
            const pairs = [
              ["gzip", "gunzip"],
              ["brotli", "unbrotli"],
              ["lzma", "unlzma"],
            ];
            const out = pairs.map(([encode, decode]) => {{
              const packed = $[encode](bytes);
              const plain = $[decode](packed);
              return packed.byteLength + ":" + (plain.byteLength === bytes.byteLength ? "same" : "diff");
            }});
            // xz 的 magic：前 6 个字节是 FD 37 7A 58 5A 00
            out.push($.hex($.lzma(bytes).slice(0, 6)));
            out
            "#,
            input = js_path(&path),
            gz = js_path(&gz_path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    for (index, value) in results.iter().take(3).enumerate() {
        let (size, same) = value.split_once(':').expect("格式应当是 size:same");
        assert_eq!(same, "same", "第 {index} 对解压后长度不一致：{value}");
        assert!(size.parse::<usize>().unwrap() > 0, "压缩结果不应为空");
    }
    assert_eq!(results[3], "fd377a585a00", "xz 容器 magic");

    // Rust 侧独立解压 gzip 产物
    let compressed = std::fs::read(&gz_path).expect("读取 gzip 结果失败");
    let _ = std::fs::remove_file(&gz_path);
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut restored = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut restored).expect("gzip 解码失败");
    assert_eq!(restored, payload, "gzip 产物应当能被标准解码器还原");
}

/// 压缩级别越界要报错（不是静默用默认值）。
#[tokio::test]
async fn compression_level_is_validated() {
    let runtime = compute_runtime().await;

    let messages: Vec<String> = runtime
        .eval(
            r#"
            const out = [];
            const tryCall = (fn) => {
              try {
                fn();
                out.push("没有抛错");
              } catch (err) {
                out.push(err.message);
              }
            };
            tryCall(() => $.gzip("x", 99));
            tryCall(() => $.brotli("x", 42));
            tryCall(() => $.zstd("x", 9999));
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    for message in &messages {
        assert!(
            message.contains("级别") || message.contains("质量"),
            "应当报范围错误：{message}"
        );
    }
}

/// `$.chunks`：默认 1024、可自定义、拼接后与原文一致。
#[tokio::test]
async fn chunks_split_and_reassemble() {
    let payload: Vec<u8> = (0..2500u32).map(|i| (i % 253) as u8).collect();
    let path = write_temp_file("chunks.bin", &payload);
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({path});
            const defaultSizes = $.chunks(bytes).map((part) => part.byteLength);
            const custom = $.chunks(bytes, 700);
            // 拼回去再算摘要，避免在测试里搬 ArrayBuffer 数组
            const merged = new Uint8Array(bytes.byteLength);
            let offset = 0;
            for (const part of custom) {{
              merged.set(new Uint8Array(part), offset);
              offset += part.byteLength;
            }}
            [defaultSizes.join(","), custom.length.toString(), $.md5(merged), $.md5(bytes)]
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_file(&path);

    assert_eq!(values[0], "1024,1024,452", "默认分片大小");
    assert_eq!(values[1], "4", "2500 / 700 向上取整");
    assert_eq!(values[2], values[3], "拼接后应当与原文一致");
}

/// `$.chunks` 传字符串时返回 `string[]`：按 **Unicode 码点**切，拼接回去与原文一致。
#[tokio::test]
async fn chunks_on_string_returns_strings() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const parts = $.chunks("ab👍cd", 2);
            const chinese = $.chunks("你好世界", 2);
            const empty = $.chunks("", 4);
            const zero = $.chunks("abc", 0);
            const huge = $.chunks("abc", 9999);
            const binary = $.chunks(new Uint8Array([1, 2, 3, 4, 5]), 2);
            [
              parts.every((part) => typeof part === "string").toString(),
              parts.join("|"),
              parts.join(""),
              chinese.join("|"),
              empty.length.toString(),
              zero.join("|"),
              huge.length.toString(),
              huge[0],
              binary.every((part) => part instanceof ArrayBuffer).toString(),
              binary.map((part) => part.byteLength).join(","),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "true", "字符串入参应当返回 string[]");
    // "ab👍cd" 是 5 个码点（emoji 算 1 个），按 2 切：ab / 👍c / d
    assert_eq!(values[1], "ab|👍c|d", "按码点切，emoji 不能被切开");
    assert_eq!(values[2], "ab👍cd", "拼接回去必须与原文一致");
    assert_eq!(values[3], "你好|世界");
    assert_eq!(values[4], "0", "空串返回空数组");
    assert_eq!(values[5], "a|b|c", "chunkSize 为 0 时按 1 处理");
    assert_eq!(values[6], "1", "chunkSize 超过长度时只有一片");
    assert_eq!(values[7], "abc");
    assert_eq!(values[8], "true", "二进制入参仍是 ArrayBuffer[]（未回归）");
    assert_eq!(values[9], "2,2,1", "二进制仍按字节切");
}

// ── 文件系统 ────────────────────────────────────────────────────────────────

/// 只读操作走一遍：read / read_text / exists / stat / list。
#[tokio::test]
async fn file_read_operations() {
    let dir = temp_dir("read");
    std::fs::write(dir.join("hello.txt"), "你好，ClipBeam\n").expect("写入测试文件失败");
    std::fs::write(dir.join("second.bin"), [0u8, 1, 2]).expect("写入测试文件失败");

    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const dir = {dir};
            const stat = await $.stat(dir + "/hello.txt");
            [
              $.str(await $.read(dir + "/hello.txt")),
              await $.read_text(dir + "/hello.txt"),
              String(await $.exists(dir + "/hello.txt")),
              String(await $.exists(dir + "/missing.txt")),
              String(await $.exists(dir)),
              stat.isFile + ":" + stat.isDir + ":" + (stat.size > 0) + ":" + (stat.modifiedMs > 0),
              (await $.list(dir)).join(","),
            ]
            "#,
            dir = js_path(&dir)
        ))
        .await
        .expect("脚本执行失败");

    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(values[0], "你好，ClipBeam\n");
    assert_eq!(values[1], "你好，ClipBeam\n");
    assert_eq!(values[2], "true");
    assert_eq!(values[3], "false");
    assert_eq!(values[4], "true");
    assert_eq!(values[5], "true:false:true:true");
    assert_eq!(values[6], "hello.txt,second.bin");
}

/// 修改操作走一遍：write / write_text / append / mkdir / copy / rename / remove。
#[tokio::test]
async fn file_write_operations() {
    let dir = temp_dir("write");
    let runtime = runtime_with(Arc::new(TestHost::default())).await;

    runtime
        .eval::<()>(&format!(
            r#"
            const dir = {dir};
            await $.write_text(dir + "/a.txt", "one");
            await $.append_text(dir + "/a.txt", "+two");
            await $.write(dir + "/b.bin", new Uint8Array([1, 2, 3]));
            await $.append(dir + "/b.bin", new Uint8Array([4]));
            await $.mkdir(dir + "/nested/deep");
            await $.copy(dir + "/a.txt", dir + "/nested/copy.txt");
            await $.rename(dir + "/b.bin", dir + "/nested/moved.bin");
            await $.remove(dir + "/a.txt");
            "#,
            dir = js_path(&dir)
        ))
        .await
        .expect("脚本执行失败");

    let text = std::fs::read_to_string(dir.join("nested/copy.txt")).expect("复制目标应当存在");
    assert_eq!(text, "one+two");
    let moved = std::fs::read(dir.join("nested/moved.bin")).expect("改名目标应当存在");
    assert_eq!(moved, vec![1, 2, 3, 4]);
    assert!(!dir.join("a.txt").exists(), "remove 应当删掉文件");
    assert!(!dir.join("b.bin").exists(), "rename 应当移走源文件");
    assert!(dir.join("nested/deep").is_dir(), "mkdir 应当递归创建");

    // 目录也能整体删除
    let runtime = runtime_with(Arc::new(TestHost::default())).await;
    runtime
        .eval::<()>(&format!(r#"await $.remove({});"#, js_path(&dir)))
        .await
        .expect("脚本执行失败");
    assert!(!dir.exists(), "remove 应当递归删除目录");
}

/// 相对路径一律拒绝（脚本的工作目录不可预期）。
#[tokio::test]
async fn relative_paths_are_rejected() {
    let runtime = compute_runtime().await;

    let messages: Vec<String> = runtime
        .eval(
            r#"
            const out = [];
            for (const path of ["Cargo.toml", "./a.txt", "../a.txt"]) {
              try {
                await $.read(path);
                out.push("没有抛错");
              } catch (err) {
                out.push(err.message);
              }
            }
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    for message in messages {
        assert!(
            message.contains("绝对路径"),
            "应当提示必须用绝对路径：{message}"
        );
    }
}

/// 宿主拒绝时修改类操作失败，但只读操作不受影响。
#[tokio::test]
async fn denied_file_change_fails_only_mutations() {
    let dir = temp_dir("denied");
    std::fs::write(dir.join("keep.txt"), "keep").expect("写入测试文件失败");

    let host = Arc::new(TestHost::default());
    host.set_file_answer(FileDecision::Deny);
    let runtime = runtime_with(host.clone()).await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const dir = {dir};
            const out = [];
            out.push(await $.read_text(dir + "/keep.txt"));
            try {{
              await $.write_text(dir + "/new.txt", "x");
              out.push("没有抛错");
            }} catch (err) {{
              out.push(err.message);
            }}
            out
            "#,
            dir = js_path(&dir)
        ))
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "keep", "只读操作不应当被授权挡住");
    assert!(values[1].contains("拒绝"), "拒绝时应当报错：{}", values[1]);
    assert_eq!(host.file_requests().len(), 1, "只读操作不应当发起询问");
    assert!(!dir.join("new.txt").exists(), "拒绝后不应写入");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 「本次运行内该目录都允许」只问一次；跨运行不保留。
#[tokio::test]
async fn allow_dir_asks_once_per_run() {
    let dir = temp_dir("allow-dir");

    let host = Arc::new(TestHost::default());
    host.set_file_answer(FileDecision::AllowDir);
    let runtime = runtime_with(host.clone()).await;

    runtime
        .eval::<()>(&format!(
            r#"
            const dir = {dir};
            await $.write_text(dir + "/a.txt", "a");
            await $.write_text(dir + "/b.txt", "b");
            await $.write_text(dir + "/c.txt", "c");
            "#,
            dir = js_path(&dir)
        ))
        .await
        .expect("脚本执行失败");

    assert_eq!(
        host.file_requests().len(),
        1,
        "同一个目录只需问一次：{:?}",
        host.file_requests()
    );
    assert!(dir.join("c.txt").exists());

    // 新的一次运行（新运行时 = 新上下文）必须重新询问
    let host = Arc::new(TestHost::default());
    host.set_file_answer(FileDecision::AllowDir);
    let runtime = runtime_with(host.clone()).await;
    runtime
        .eval::<()>(&format!(
            r#"await $.write_text({}, "x");"#,
            js_path(&dir.join("d.txt"))
        ))
        .await
        .expect("脚本执行失败");
    assert_eq!(host.file_requests().len(), 1, "新的运行应当重新询问");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `~`、git-bash 与 Cygwin 风格的路径都能解析（解析失败也要给出可读错误）。
#[tokio::test]
async fn path_flavours_are_accepted() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const out = [];
            // 主目录必定存在
            out.push(String(await $.exists("~/")));
            // git-bash / Cygwin 风格会解析成 D:/…，应当报「不存在」而不是「不是绝对路径」
            for (const path of ["/d/definitely/not/here", "/cygdrive/d/definitely/not/here", "C:\\definitely\\not\\here"]) {
              try {
                await $.read(path);
                out.push("没有抛错");
              } catch (err) {
                out.push(err.message.includes("绝对路径") ? "被当成相对路径" : "已知路径解析");
              }
            }
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "true", "~ 应当展开成主目录");
    for value in &values[1..] {
        assert_eq!(value, "已知路径解析", "盘符风格应当被接受：{values:?}");
    }
}

// ── 宿主交互 ────────────────────────────────────────────────────────────────

/// `$.request_focus` 把提示语交给宿主；省略参数时传空串（宿主用默认文案）。
#[tokio::test]
async fn request_focus_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    runtime
        .eval::<()>(
            r#"
            $.request_focus("请点击远程记事本");
            $.request_focus();
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(
        host.focus_requests(),
        vec!["请点击远程记事本".to_string(), String::new()]
    );
}

/// `$.type_str` 把文本交给宿主。
#[tokio::test]
async fn type_str_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    runtime
        .eval::<()>(r#"$.type_str("hello 世界\n", 0);"#)
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
    assert!(
        message.contains("已中止"),
        "Abort 应当抛中止异常：{message}"
    );
}

/// 宿主返回 `Cancelled` 时，`$.type_str` 抛异常终止脚本。
#[tokio::test]
async fn cancelled_host_aborts_type_str() {
    struct CancellingHost;

    impl ScriptHost for CancellingHost {
        fn type_str(&self, _text: &str, _delay_ms: u64) -> Result<(), HostError> {
            Err(HostError::Cancelled)
        }
        fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
            Ok(ConfirmChoice::No)
        }
        fn pick_path(&self, _prompt: &str, _kind: PickKind) -> Result<Option<PathBuf>, HostError> {
            Ok(None)
        }
        fn cancelled(&self) -> bool {
            true
        }
    }

    let console: Arc<dyn ConsoleHook> = Arc::new(TestConsole::default());
    let runtime = clipbeam_scripting::create_runtime(Arc::new(CancellingHost), console, None)
        .await
        .expect("创建运行时失败");

    let err = runtime
        .eval::<()>(r#"$.type_str("x"); console.log("不该执行到这里");"#)
        .await
        .expect_err("取消应当让脚本失败");

    assert!(
        err.to_string().contains("已中止"),
        "错误信息应说明中止：{err}"
    );
}

/// 没有注入宿主时，交互能力给出可读的接线错误（而不是静默什么都不做）。
#[tokio::test]
async fn interaction_without_host_reports_wiring_error() {
    // 配好命名空间、装好能力，但**不**注入宿主：等价于「忘了调 runtime_options(host, ..)」
    let runtime = ScriptRuntime::with_options(
        RuntimeOptions::default()
            .extensions(extensions())
            .namespace(clipbeam_scripting::NAMESPACE)
            .namespace_alias(clipbeam_scripting::NAMESPACE_ALIAS),
    )
    .await
    .expect("创建运行时失败");

    let message: String = runtime
        .eval(
            r#"
            try {
                $.type_str("x");
                "没有抛错"
            } catch (err) {
                err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(
        message.contains("没有注入宿主"),
        "错误信息应当指向接线问题：{message}"
    );
}

/// `$.pick_path`：prompt / kind 原样送达宿主，宿主的回答映射成 `string | null`。
#[tokio::test]
async fn pick_path_reaches_host() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    let picked = PathBuf::from("/tmp/clipbeam-选择的文件.bin");
    let picked_text = picked.to_string_lossy().to_string();
    host.set_pick_answer(Some(picked.clone()));

    let values: Vec<String> = runtime
        .eval(
            r#"
            const file = await $.pick_path("请选择一个文件");
            const dir = await $.pick_path("请选择一个文件夹", "dir");
            [typeof file, file, typeof dir, dir]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "string", "选到路径时应当是字符串");
    assert_eq!(values[1], picked_text, "应当原样返回宿主给的路径");
    assert_eq!(values[2], "string", "kind=dir 同样返回字符串");
    assert_eq!(values[3], picked_text);

    assert_eq!(
        host.pick_requests(),
        vec![
            ("请选择一个文件".to_string(), PickKind::File),
            ("请选择一个文件夹".to_string(), PickKind::Dir),
        ],
        "prompt 与 kind 应当原样送到宿主（kind 缺省是 file）"
    );

    // 宿主回答「没拿到路径」（用户取消 / 没有界面）→ null（而不是 undefined）
    host.set_pick_answer(None);
    let is_null: bool = runtime
        .eval(r#"await $.pick_path("随便") === null"#)
        .await
        .expect("脚本执行失败");
    assert!(is_null, "取消时应当是 null");

    // kind 写错：当场报错，而不是静默按文件处理
    let message: String = runtime
        .eval(
            r#"
            try {
              await $.pick_path("x", "files");
              "没有抛错"
            } catch (err) {
              err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");
    assert!(
        message.contains("kind"),
        "非法 kind 应当报错并说明取值：{message}"
    );
}

// ── 内置示例脚本 ────────────────────────────────────────────────────────────

/// JS 示例端到端跑通：写文件 → 读回来 → md5 / crc32 → zstd + chunks → 逐个确认。
///
/// `$.confirm` 一律回答「否」：脚本会跳过分片的输出，因此测试不用等键盘延迟就能
/// 验证「能力真的串起来了」。
#[tokio::test]
async fn quick_start_seed_script_runs_end_to_end() {
    let dir = temp_dir("seed-quick-start");
    let target = dir.join("sample.txt");

    let host = Arc::new(TestHost::with_answer(ConfirmChoice::No));
    let runtime = runtime_with(host.clone()).await;

    // 示例写的是 `~/clipbeam-quick-start.txt`；测试里换成临时目录，别动用户的主目录
    let source = include_str!("../seed/01-quick-start.js")
        .replace("'~/clipbeam-quick-start.txt'", &js_path(&target));

    runtime
        .run_named_script("01-quick-start.js", &source)
        .await
        .expect("示例脚本应当跑通");

    let typed = host.typed();
    assert!(typed.contains("MD5:"), "应当输出整文件摘要：{typed}");
    assert!(typed.contains("CRC32:"), "应当输出校验和：{typed}");
    assert!(typed.contains("片"), "应当输出分片数量：{typed}");
    assert!(
        !host.confirm_messages().is_empty(),
        "应当对每个分片发起确认（跳过分支也应被触发）"
    );
    assert!(target.exists(), "示例应当真的写出了文件");

    let _ = std::fs::remove_dir_all(&dir);
}

/// TS 示例端到端跑通（转译 → 执行 → 类型注解与能力都可用）。
#[tokio::test]
async fn typescript_seed_script_runs_end_to_end() {
    let host = Arc::new(TestHost::default());
    let runtime = runtime_with(host.clone()).await;

    let source = include_str!("../seed/02-ts-demo.ts");
    let js = clipbeam_scripting::ts::transpile(source, Path::new("02-ts-demo.ts"))
        .expect("示例 TS 应当能转译");

    runtime
        .run_named_script("02-ts-demo.ts", &js)
        .await
        .expect("示例脚本应当跑通");

    let typed = host.typed();
    assert!(typed.contains("======="), "应当输出预览分隔线：{typed}");
    assert!(typed.contains("编码与压缩往返"), "应当输出标题：{typed}");
}

/// 从 `$.type_str` 的 transcript 里拆出 heredoc：每段形如
/// `cat <<'EOF' > <名字>\n<正文>\nEOF\n`，返回 `(名字, 正文)`。
///
/// 这正是接收端 bash 脚本要做的事 —— 测试里用 Rust 重写一遍，不复用脚本侧实现。
fn parse_heredocs(transcript: &str) -> Vec<(String, String)> {
    const HEAD: &str = "cat <<'EOF' > ";

    let mut sections = Vec::new();
    let mut rest = transcript;

    while let Some(head) = rest.find(HEAD) {
        let after = &rest[head + HEAD.len()..];
        let (name, body_and_tail) = after.split_once('\n').expect("heredoc 头后面应当有换行");
        let (body, tail) = body_and_tail
            .split_once("\nEOF\n")
            .expect("heredoc 正文后应当有 EOF 定界符");
        sections.push((name.to_string(), body.to_string()));
        rest = tail;
    }

    sections
}

/// 按名字取一段 heredoc（拿不到就直接炸，错误信息里带上名字）。
fn section<'a>(sections: &'a [(String, String)], name: &str) -> &'a (String, String) {
    sections
        .iter()
        .find(|(section_name, _)| section_name == name)
        .unwrap_or_else(|| panic!("敲出的内容里缺少 {name}"))
}

/// 从 seed 源码里取出 `getRestoreScript()` 返回的模板字面量正文。
///
/// 正文按约定不写 `${}`、反引号、反斜杠（否则模板字面量就要转义），这里直接按标记切片，
/// 顺手把这三条约定钉死。
///
/// `${` 这条连注释都算：曾经在 bash 注释里写了个长度展开的写法，TS/oxc 把它当成
/// 真插值，模板一直延续到结尾那个反引号，于是整个脚本解析失败 —— 错误行号还会指到
/// 末尾，很难看出是注释的锅。
fn restore_script_literal(source: &str) -> String {
    const BEGIN: &str = "function getRestoreScript(): string {\n\treturn `";
    const END: &str = "\n`;\n}";

    let start = source.find(BEGIN).expect("找不到 getRestoreScript 函数") + BEGIN.len();
    let end = source[start..]
        .find(END)
        .expect("找不到 getRestoreScript 模板字面量的结尾")
        + start
        // +1：模板字面量的值以结尾那个换行收束（远端解出来的脚本也该这么结尾）
        + 1;
    let literal = &source[start..end];

    assert!(
        !literal.contains('`'),
        "正文里不能出现反引号（模板字面量会被截断）"
    );
    assert!(
        !literal.contains('\\'),
        "正文里不能出现反斜杠（模板字面量会转义）"
    );
    assert!(
        !literal.contains("${"),
        "正文里不能出现 ${{（模板字面量会插值）"
    );

    literal.to_string()
}

/// `03-pack-and-shard.ts` 端到端：读文件 → gzip → base32_lower_nopad → 分片 →
/// 逐片以 `cat <<'EOF' > xx.p0001` 敲出。
///
/// 断言覆盖链路两端：先看敲出去的 transcript 结构（分片名连续、正文只含 base32
/// 小写盘表、除最后一片都满片），再用 Rust 侧独立实现把它拼回去（base32 解码 +
/// gunzip）与源文件逐字节比对 —— 就是示例注释里那份接收端 bash 脚本的等价物。
#[tokio::test]
async fn pack_and_shard_seed_script_runs_end_to_end() {
    use md5::{Digest, Md5};

    let dir = temp_dir("seed-pack-and-shard");
    let target = dir.join("sample.bin");

    // 3000 字节 xorshift 流：接近不可压缩，能真的切出多片
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let payload: Vec<u8> = (0..3000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    std::fs::write(&target, &payload).unwrap();

    let host = Arc::new(TestHost::with_answer(ConfirmChoice::Yes));
    let runtime = runtime_with(host.clone()).await;

    // 示例写的是 `~/clipbeam-quick-start.txt`；测试里换成临时文件（与 01 示例同一手法）。
    // 文件已存在，所以示例的「不存在才写示例文件」分支不会覆盖这份 payload
    let source = include_str!("../seed/03-pack-and-shard.ts")
        .replace("'~/clipbeam-quick-start.txt'", &js_path(&target));

    // .ts 示例和 02-ts-demo.ts 一样先转译再跑（转译后报错行号指向生成代码）
    let source = clipbeam_scripting::ts::transpile(&source, Path::new("03-pack-and-shard.ts"))
        .expect("示例 TS 应当能转译");

    runtime
        .run_named_script("03-pack-and-shard.ts", &source)
        .await
        .expect("示例脚本应当跑通");

    let typed = host.typed();
    let sections = parse_heredocs(&typed);
    assert!(
        sections.len() >= 3,
        "应当先敲还原脚本、再敲元信息与多片：{typed}"
    );

    // 远端文件名一律从源文件名派生：sample.bin → sample.bin.gz.b32.p0001
    let base = "sample.bin";
    let packed_name = format!("{base}.gz.b32");

    // 还原脚本：只 base32、不压缩不分片，一个 heredoc —— 解出来必须与 seed 里的正文逐字节一致
    let restore_body = &section(&sections, "restore.sh.b32").1;
    assert!(
        restore_body
            .chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c) || c == '\n' || c == '='),
        "还原脚本负载应当只有 base32 小写、带 = 填充与换行：{restore_body}"
    );
    let decoded_script = String::from_utf8(
        data_encoding::BASE32
            .decode(
                restore_body
                    .split_whitespace()
                    .collect::<String>()
                    .to_ascii_uppercase()
                    .as_bytes(),
            )
            .expect("还原脚本 base32 解码失败"),
    )
    .expect("还原脚本应当是 UTF-8");
    assert_eq!(
        decoded_script,
        restore_script_literal(include_str!("../seed/03-pack-and-shard.ts")),
        "敲给远端的还原脚本必须与 seed 里的正文一致"
    );

    // 元信息
    let meta_body = &section(&sections, &format!("{base}.meta")).1;
    let value_of = |key: &str| -> String {
        let needle = format!("{key}=");
        meta_body
            .lines()
            .find_map(|line| line.strip_prefix(needle.as_str()))
            .unwrap_or_else(|| panic!("元信息里缺少 {key}：{meta_body}"))
            .to_string()
    };
    let parts: usize = value_of("parts").parse().expect("parts 应当是数字");
    let chars: usize = value_of("chars").parse().expect("chars 应当是数字");
    let b32_md5 = value_of("b32_md5");
    let raw_bytes: usize = value_of("raw_bytes").parse().expect("raw_bytes 应当是数字");
    let raw_md5 = value_of("raw_md5");

    assert_eq!(raw_bytes, payload.len(), "元信息里的原始大小应当正确");
    assert!(parts > 1, "测试数据应当切出多片：parts={parts}");

    // 分片：名字连续、正文只含小写 base32 盘表、除最后一片都满片
    let shards: Vec<&(String, String)> = sections
        .iter()
        .filter(|(name, _)| name.starts_with(&format!("{packed_name}.p")))
        .collect();
    assert_eq!(shards.len(), parts, "分片数应当与元信息一致");

    let mut b32 = String::new();
    for (index, (name, body)) in shards.iter().enumerate() {
        assert_eq!(
            name,
            &format!("{packed_name}.p{:04}", index + 1),
            "分片名应当从 p0001 起连续"
        );
        // 无填充变体：任何一片（含最后一片）都不该出现 `=`
        assert!(
            body.chars()
                .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
            "分片 {name} 只应包含 base32 小写盘表字符：{body}"
        );
        if index + 1 < parts {
            assert_eq!(body.chars().count(), 1024, "除最后一片外都应当是 1024 字符");
        }
        b32.push_str(body);
    }
    assert_eq!(b32.chars().count(), chars, "base32 字符数应当与元信息一致");

    // 传输层摘要：脚本的 `$.md5` 与 md-5 crate 交叉验证
    assert_eq!(
        hex::encode(Md5::digest(b32.as_bytes())),
        b32_md5,
        "base32 文本摘要应当一致"
    );

    // 接收端还原：base32 解码 → gunzip → 与源文件逐字节比对
    let decoded = data_encoding::BASE32_NOPAD
        .decode(b32.to_ascii_uppercase().as_bytes())
        .expect("base32 解码失败");
    let mut decoder = flate2::read::GzDecoder::new(decoded.as_slice());
    let mut restored = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut restored).expect("gzip 解码失败");

    assert_eq!(restored, payload, "还原出的字节应当与源文件一致");
    assert_eq!(
        hex::encode(Md5::digest(restored.as_slice())),
        raw_md5,
        "还原文件摘要应当与元信息一致"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `sleep` 是引擎提供的**标准全局**（必须 await），不在 `$` 上。
#[tokio::test]
async fn sleep_is_a_global_not_a_capability() {
    let runtime = compute_runtime().await;

    let value: String = runtime
        .eval(
            r#"
            [
              typeof sleep,
              typeof $.sleep,
              typeof ClipBeam.sleep,
            ].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");
    assert_eq!(value, "function|undefined|undefined");

    // 真的能等（而且必须 await 才算等到）
    let started = std::time::Instant::now();
    runtime
        .eval::<()>("await sleep(60);")
        .await
        .expect("脚本执行失败");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(60),
        "await sleep(60) 应当真的等过 60ms"
    );
}

// ── 清单一致性 ──────────────────────────────────────────────────────────────

/// 声明文件里的每个方法都必须在运行期存在（第三个真源不能漂移）。
#[tokio::test]
async fn spec_dts_matches_runtime() {
    let runtime = compute_runtime().await;
    let declared = methods_declared_in_spec();

    assert!(
        declared.contains(&"md5".to_string()) && declared.contains(&"write_text".to_string()),
        "解析声明文件失败：{declared:?}"
    );

    for name in &declared {
        let present: bool = runtime
            .eval(&format!("typeof $.{name} === 'function'"))
            .await
            .expect("脚本执行失败");
        assert!(present, "clipbeam.d.ts 声明了 {name}，但运行期不存在");
    }
}

/// 反过来：能力清单里的每个名字也都要在声明文件里（不能只写实现不写类型）。
#[test]
fn capabilities_are_all_declared_in_spec() {
    let declared = methods_declared_in_spec();
    for cap in capabilities()
        .into_iter()
        .filter(|cap| cap.source == "clipbeam")
    {
        assert!(
            declared.contains(&cap.name),
            "能力 {} 没有写进 clipbeam.d.ts：{declared:?}",
            cap.name
        );
    }
}

/// 从 `src/spec/clipbeam.d.ts` 的 `interface ClipBeam { … }` 块里抽方法名。
///
/// 只做最简单的文本解析（与引擎的 tests/spec_sync.rs 同一套规则）：声明文件是我们
/// 自己写的，格式受控。要求行首是标识符字符，因此注释行（`//`、`*`、`/**`）不会被算进来。
///
/// **这里不能做字节偏移运算**：`.d.ts` 是 `include_str!` 进来的，而 Windows 上
/// `core.autocrlf` 会把工作区里的文件变成 CRLF；按 `line.len() + 1` 累加偏移在 CRLF 下
/// 每行少算 1 字节，偏移整体前移，花括号深度会减到 0 以下（debug 构建直接 panic）。
/// 所以只按行读，不碰字节下标。
fn methods_declared_in_spec() -> Vec<String> {
    const MARKER: &str = "interface ClipBeam {";
    let source = include_str!("../src/spec/clipbeam.d.ts");

    let mut methods = Vec::new();
    let mut inside = false;
    let mut depth = 0usize;

    for line in source.lines() {
        if !inside {
            if line.starts_with(MARKER) {
                inside = true;
                depth = 1; // MARKER 末尾那个 `{`
            }
            continue;
        }

        // 方法名：行首是标识符字符且带 `(`（注释行以 `/` 或 `*` 开头，自然被排除）
        let trimmed = line.trim();
        if trimmed.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_' || ch == '$') {
            if let Some(paren) = trimmed.find('(') {
                let name: String = trimmed[..paren]
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
                    .collect();
                if !name.is_empty() {
                    methods.push(name);
                }
            }
        }

        // 花括号深度：`interface` 块收尾的那个 `}` 之后就不再看了
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if depth == 0 {
            break;
        }
    }

    assert!(
        inside,
        "clipbeam.d.ts 里应当有行首的 `interface ClipBeam {{` 声明"
    );
    assert!(
        depth == 0,
        "`interface ClipBeam {{` 花括号应当闭合（读到文件尾仍未闭合）"
    );
    methods
}

/// 扩展列表是稳定的（顺序变化会导致能力清单与注册顺序不一致）。
#[test]
fn extensions_are_listed() {
    let extensions = extensions();
    assert_eq!(extensions.len(), 19, "应当有 19 个使用方扩展");

    let names: Vec<String> = extensions
        .iter()
        .flat_map(|extension| extension.spec())
        .map(|spec| spec.name.to_string())
        .collect();

    assert_eq!(names.first().map(String::as_str), Some("bytes"));
    assert_eq!(names.last().map(String::as_str), Some("format_bytes"));
    assert!(names.contains(&"md5".to_string()));
    assert!(names.contains(&"type_str".to_string()));
    // 纯计算工具（不依赖宿主）：路径字符串与展示格式化
    for expected in [
        "basename",
        "dirname",
        "extname",
        "stem",
        "format_duration",
        "format_bytes",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "能力清单缺少 {expected}"
        );
    }
    assert!(
        !names.contains(&"sleep".to_string()),
        "sleep 是引擎提供的标准全局，不该出现在能力清单里"
    );
}

/// `runtime_options()` 与引擎默认配置的差别只在「装了能力 + 有宿主」。
#[test]
fn runtime_options_helper_has_all_extensions() {
    let console: Arc<dyn ConsoleHook> = Arc::new(StdoutConsole);
    let options = runtime_options(Arc::new(TestHost::default()), console);
    assert_eq!(options.extensions.len(), 19);
    assert!(options.prepare.is_some(), "宿主应当通过 prepare 钩子注入");
    assert_eq!(
        options.namespace.as_deref(),
        Some(clipbeam_scripting::NAMESPACE)
    );
    assert_eq!(
        options.namespace_alias.as_deref(),
        Some(clipbeam_scripting::NAMESPACE_ALIAS)
    );
    assert_eq!(RuntimeOptions::default().extensions.len(), 0);
}

/// 让 `ScriptExtension` 保持可见（trait 对象在注册表里到处都是）。
#[test]
fn extension_trait_is_object_safe() {
    let extension: Arc<dyn ScriptExtension> =
        Arc::new(clipbeam_scripting::extensions::md5::Md5Extension);
    assert_eq!(extension.spec().len(), 1);
}

// ── 纯计算工具：路径字符串与展示格式化 ──────────────────────────────────────

/// `$.basename/dirname/extname/stem` 的边界表（与 `extensions/path.rs` 的单测同一张表）。
///
/// 在能力层再跑一遍是有意义的：`extensions/path.rs` 的单测直接调内部函数，
/// 而这里走的是「JS 能看见的绑定」—— 参数个数、返回类型、命名都不能漂移。
#[tokio::test]
async fn path_helpers_boundary_table() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const cases = [
              ["/a/b/c.txt", "c.txt", "/a/b", ".txt", "c"],
              ["/a/b/", "b", "/a", "", "b"],
              ["/a", "a", "", "", "a"],
              ["c.txt", "c.txt", "", ".txt", "c"],
              ["C:\\x\\y.ZIP", "y.ZIP", "C:\\x", ".ZIP", "y"],
              ["a/b/.bashrc", ".bashrc", "a/b", "", ".bashrc"],
              ["a/b/c.tar.gz", "c.tar.gz", "a/b", ".gz", "c.tar"],
              ["a/b/c.", "c.", "a/b", ".", "c"],
              ["", "", "", "", ""],
              ["/", "", "", "", ""],
              ["//a//b//", "b", "//a", "", "b"],
            ];
            const out = [];
            for (const [input, base, dir, ext, stem] of cases) {
              out.push([
                $.basename(input) === base ? "ok" : `basename=${JSON.stringify($.basename(input))} want ${JSON.stringify(base)}`,
                $.dirname(input) === dir ? "ok" : `dirname=${JSON.stringify($.dirname(input))} want ${JSON.stringify(dir)}`,
                $.extname(input) === ext ? "ok" : `extname=${JSON.stringify($.extname(input))} want ${JSON.stringify(ext)}`,
                $.stem(input) === stem ? "ok" : `stem=${JSON.stringify($.stem(input))} want ${JSON.stringify(stem)}`,
              ].join(","));
            }
            out
            "#,
        )
        .await
        .expect("脚本执行失败");

    for (i, result) in values.iter().enumerate() {
        assert!(
            result.split(',').all(|part| part == "ok"),
            "边界表第 {i} 条不通过：{result}"
        );
    }
}

/// 两种分隔符混用也按同一规则切（跨平台行为一致）。
#[tokio::test]
async fn path_helpers_mixed_separators() {
    let runtime = compute_runtime().await;
    let values: Vec<String> = runtime
        .eval(
            r#"
            [
              $.basename("a\\b/c.txt"),
              $.dirname("a\\b/c.txt"),
              $.basename("a/b\\c.txt"),
              $.dirname("a/b\\c.txt"),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(values[0], "c.txt");
    assert_eq!(values[1], "a\\b");
    assert_eq!(values[2], "c.txt");
    assert_eq!(values[3], "a/b");
}

/// `$.format_duration` / `$.format_bytes`：与 Rust 侧实现交叉验证 + 边界。
#[tokio::test]
async fn format_helpers_boundary_table() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            [
              $.format_duration(0),
              $.format_duration(45),
              $.format_duration(59.9),
              $.format_duration(60),
              $.format_duration(83),
              $.format_duration(3600),
              $.format_duration(3661),
              $.format_duration(-5),
              $.format_bytes(0),
              $.format_bytes(512),
              $.format_bytes(1023),
              $.format_bytes(1024),
              $.format_bytes(1536),
              $.format_bytes(1048576),
              $.format_bytes(1610612736),
              $.format_bytes(-1),
            ]
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(
        values,
        vec![
            "0s", "45s", "59s", "1m0s", "1m23s", "1h0m0s", "1h1m1s", "0s", "0B", "512B", "1023B",
            "1.0KB", "1.5KB", "1.00MB", "1.50GB", "0B",
        ]
    );
}

/// `$.md5` 的五种格式：与 Rust 侧独立实现（`md-5` / `data-encoding`）交叉验证。
#[tokio::test]
async fn md5_formats_match_rust() {
    let payload: Vec<u8> = (0u8..=255).collect();
    let path = write_temp_file("md5-formats.bin", &payload);
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(&format!(
            r#"
            const bytes = await $.read({path});
            [
              $.md5(bytes),
              $.md5(bytes, "hex"),
              $.md5(bytes, "hex_upper"),
              $.md5(bytes, "base32"),
              $.md5(bytes, "base32_lower"),
              $.md5(bytes, "base64"),
            ]
            "#,
            path = js_path(&path)
        ))
        .await
        .expect("脚本执行失败");
    let _ = std::fs::remove_file(&path);

    use md5::{Digest, Md5};
    let digest = Md5::digest(&payload);
    let hex_lower = hex::encode(digest);
    let hex_upper = hex::encode_upper(digest);

    assert_eq!(values[0], hex_lower, "默认仍是小写十六进制（向后兼容）");
    assert_eq!(values[1], hex_lower, "显式 hex 与默认一致");
    assert_eq!(values[2], hex_upper, "hex_upper");
    assert_eq!(values[3], data_encoding::BASE32_NOPAD.encode(&digest));
    assert_eq!(
        values[4],
        data_encoding::BASE32_NOPAD
            .encode(&digest)
            .to_ascii_lowercase()
    );
    assert_eq!(values[5], data_encoding::BASE64_NOPAD.encode(&digest));

    // 长度：32 / 32 / 32 / 26 / 26 / 22
    assert_eq!(values[3].len(), 26);
    assert_eq!(values[4].len(), 26);
    assert_eq!(values[5].len(), 22);
}

/// **协议指纹的约定**：`$.md5(x, "base32_lower")` 编的是**原始 16 字节**，
/// 不是十六进制字符串。这条专门钉住曾经踩过的坑。
#[tokio::test]
async fn md5_base32_encodes_raw_digest_not_hex_string() {
    let runtime = compute_runtime().await;

    let values: Vec<String> = runtime
        .eval(
            r#"
            const data = new TextEncoder().encode("clipbeam");
            const viaFormat = $.md5(data, "base32_lower");   // 正确：编原始 16 字节
            const hexString = $.md5(data);                   // 32 字符十六进制
            const wrongWay = $.base32_lower_nopad(hexString); // 错误：编 hex 串
            [viaFormat, String(viaFormat.length), wrongWay, String(wrongWay.length), hexString]
            "#,
        )
        .await
        .expect("脚本执行失败");

    // 正确写法编的是 16 字节 → 26 字符；错误写法编 32 字符的 hex 串 → 52 字符
    assert_eq!(values[1], "26", "正确写法应当是 26 字符");
    assert_eq!(values[3], "52", "对 hex 串做 base32 会得到 52 字符");
    assert_ne!(values[0], values[2], "两者必须不同（这正是要防的错）");

    // 正确结果只含 `[a-z2-7]`，可直接进协议帧的字段
    assert!(
        values[0]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)),
        "base32_lower 结果只应含 [a-z2-7]，实际 {}",
        values[0]
    );

    // 而**十六进制**里会出现 `0`/`1` —— 所以它绝不能直接当协议字段用
    // （base32 的输出当然不含 0/1，那正是它安全的原因）
    assert!(
        values[4].contains('0') || values[4].contains('1'),
        "这个例子的 hex 摘要应当含 0/1 才说明问题，实际 {}",
        values[4]
    );

    // 与 Rust 侧独立实现对齐：base32(md5 的 16 字节)
    use md5::{Digest, Md5};
    let expected = data_encoding::BASE32_NOPAD
        .encode(&Md5::digest(b"clipbeam"))
        .to_ascii_lowercase();
    assert_eq!(values[0], expected);
}

/// 未知的 md5 格式要报错并列出可用取值（不静默回退）。
#[tokio::test]
async fn md5_rejects_unknown_format() {
    let runtime = compute_runtime().await;
    let message: String = runtime
        .eval(
            r#"
            try {
              $.md5("x", "base16");
              "没有抛错"
            } catch (err) {
              err.message
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert!(message.contains("base16"), "错误里应带非法取值：{message}");
    assert!(
        message.contains("base32_lower"),
        "错误里应列可用取值：{message}"
    );
}
