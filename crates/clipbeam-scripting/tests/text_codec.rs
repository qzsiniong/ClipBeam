//! `TextDecoder` / `TextEncoder` / `console` 的集成测试（搬运自 clipbeam_js）。
//!
//! 这些 API 是 `prelude.js` 用 Rust 原语补出来的，所以测试从 JS 侧走公开 API，
//! 校验的是完整的「JS 壳 + Rust 实现」链路。

use clipbeam_script::{RuntimeOptions, ScriptRuntime};
use clipbeam_scripting::runtime_options;

/// 建一个装了 ClipBeam 能力集的运行时（文本编解码不依赖宿主）。
async fn runtime() -> ScriptRuntime {
    ScriptRuntime::with_options(runtime_options())
        .await
        .expect("创建运行时失败")
}

/// `TextEncoder.encode` 返回的是真的 `Uint8Array`，字节内容与 Rust 侧一致。
#[tokio::test]
async fn encode_produces_expected_utf8_bytes() {
    let runtime = runtime().await;

    let bytes: Vec<u8> = runtime
        .eval(
            r#"
            const bytes = new TextEncoder().encode("a中");
            Array.from(bytes)
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(bytes, "a中".as_bytes().to_vec());
}

/// 中文 + emoji 的 UTF-8 往返。
#[tokio::test]
async fn utf8_roundtrip_with_cjk_and_astral() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const enc = new TextEncoder();
            const dec = new TextDecoder();
            const text = "中文 abc 🚀 é";
            const bytes = enc.encode(text);
            [dec.encoding, bytes.length, dec.decode(bytes)].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    let text = "中文 abc 🚀 é";
    assert_eq!(value, format!("utf-8|{}|{text}", text.len()));
}

/// 非 UTF-8 编码（gb18030 / shift_jis）可用 —— 这是引入 encoding_rs 的主要目的。
#[tokio::test]
async fn decodes_non_utf8_encodings() {
    let runtime = runtime().await;

    // "中文" 的 GBK 编码；"日本" 的 Shift_JIS 编码
    let value: String = runtime
        .eval(
            r#"
            const gbk = new Uint8Array([0xD6, 0xD0, 0xCE, 0xC4]).buffer;
            const sjis = new Uint8Array([0x93, 0xFA, 0x96, 0x7B]).buffer;
            [
                new TextDecoder("GBK").encoding,
                new TextDecoder("gbk").decode(gbk),
                new TextDecoder("shift_jis").decode(sjis),
            ].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "gbk|中文|日本");
}

/// 未知编码标签抛 `RangeError`（构造即校验）。
#[tokio::test]
async fn unknown_label_throws_range_error() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                new TextDecoder("no-such-encoding");
                "没有抛错"
            } catch (err) {
                [err.name, err instanceof RangeError].join("|")
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "RangeError|true");
}

/// `fatal: true` 遇非法 UTF-8 抛 `TypeError`；默认替换成 U+FFFD。
#[tokio::test]
async fn fatal_mode_throws_on_invalid_utf8() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const bad = new Uint8Array([0x41, 0xc3, 0x28]).buffer;
            const lenient = new TextDecoder().decode(bad);
            let fatalName = "没有抛错";
            try {
                new TextDecoder("utf-8", { fatal: true }).decode(bad);
            } catch (err) {
                fatalName = [err.name, err instanceof TypeError].join(":");
            }
            [lenient, fatalName].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    let (lenient, fatal) = value.split_once('|').expect("结果格式不对");
    assert_eq!(lenient, "A\u{FFFD}(");
    assert_eq!(fatal, "TypeError:true");
}

/// `ignoreBOM` 语义：默认去掉 BOM，显式 true 时保留。
#[tokio::test]
async fn bom_handling_respects_ignore_bom() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const withBom = new Uint8Array([0xef, 0xbb, 0xbf, 0x41]).buffer;
            const stripped = new TextDecoder().decode(withBom);
            const kept = new TextDecoder("utf-8", { ignoreBOM: true }).decode(withBom);
            [stripped, kept.length, kept.charCodeAt(0) === 0xfeff].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "A|2|true");
}

/// `decode()` 接受各种 BufferSource。
#[tokio::test]
async fn decode_accepts_buffer_sources() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const bytes = new Uint8Array([0x78, 0x41, 0x42, 0x43, 0x79]); // xABCy
            const view = new Uint8Array(bytes.buffer, 1, 3);              // ABC
            const dataView = new DataView(bytes.buffer, 1, 3);            // ABC
            const dec = new TextDecoder();
            [dec.decode(), dec.decode(bytes.buffer), dec.decode(view), dec.decode(dataView)].join("|")
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "|xABCy|ABC|ABC");
}

/// 传错类型抛 `TypeError`。
#[tokio::test]
async fn decode_rejects_non_buffer_source() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            try {
                new TextDecoder().decode("not a buffer");
                "没有抛错";
            } catch (err) {
                [err.name, err instanceof TypeError].join("|");
            }
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "TypeError|true");
}

/// `console` 的五个方法都存在；对象参数走 JSON.stringify 且不会抛。
#[tokio::test]
async fn console_is_available() {
    let runtime = runtime().await;

    let value: String = runtime
        .eval(
            r#"
            const methods = ["log", "info", "debug", "warn", "error"];
            const allFunctions = methods.every((name) => typeof console[name] === "function");
            const cyclic = {};
            cyclic.self = cyclic;
            console.log("console 测试", { a: 1 }, [1, 2], cyclic, 42);
            String(allFunctions)
            "#,
        )
        .await
        .expect("脚本执行失败");

    assert_eq!(value, "true");
}

/// `RuntimeOptions` 的默认值不带任何使用方扩展：能力集是显式注入的。
#[test]
fn extensions_are_opt_in() {
    assert!(
        RuntimeOptions::default().extensions.is_empty(),
        "core 默认不应包含使用方扩展"
    );
    assert_eq!(runtime_options().extensions.len(), 5);
}
