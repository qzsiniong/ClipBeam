//! ClipBeam 能力集的端到端测试。
//!
//! 断言策略：**不复用库内部实现**，而是用 Rust 侧独立的依赖（`md-5` / `crc32fast` /
//! `data-encoding` / `flate2` / `zstandard`）校验 JS 侧的结果，再补一组「公认值」
//! （`md5("abc")`、`base32("abc")`…）钉死，避免两边一起改错还能通过。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clipbeam_scripting::{
    capabilities, extensions, runtime_options, ConfirmChoice, FileDecision, HostError, ScriptHost,
};
use script_engine::{ConsoleHook, RuntimeOptions, ScriptExtension, ScriptRuntime, StdoutConsole};

// ── 测试替身 ────────────────────────────────────────────────────────────────

/// 记录型宿主：输出、确认、文件授权都可观测，答案由原子量控制。
struct TestHost {
    typed: Mutex<String>,
    confirm_messages: Mutex<Vec<String>>,
    confirm_answer: AtomicUsize, // 0=No 1=Yes 2=Abort
    file_requests: Mutex<Vec<String>>,
    file_answer: Mutex<FileDecision>,
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
fn js_path(path: &Path) -> String {
    format!("{:?}", path.to_string_lossy().replace('\\', "\\\\"))
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
    assert_eq!(extensions.len(), 14, "应当有 14 个使用方扩展");

    let names: Vec<String> = extensions
        .iter()
        .flat_map(|extension| extension.spec())
        .map(|spec| spec.name.to_string())
        .collect();

    assert_eq!(names.first().map(String::as_str), Some("bytes"));
    assert_eq!(names.last().map(String::as_str), Some("confirm"));
    assert!(names.contains(&"md5".to_string()));
    assert!(names.contains(&"type_str".to_string()));
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
    assert_eq!(options.extensions.len(), 14);
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
