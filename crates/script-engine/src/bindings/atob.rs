//! 标准全局 `atob` / `btoa`：**WHATWG 语义**的 base64，只处理 Latin-1 字符串。
//!
//! # 与使用方 `$.base64` 系列的区别
//!
//! 两者是同一套 base64 的两种接口形态，差别只在输入/输出的类型：
//!
//! | | `btoa(s)` / `atob(s)`（本模块） | `$.base64(data)` / `$.base64_decode(s)`（使用方能力） |
//! |---|---|---|
//! | 编码方向输入 | **字符串**，每个码位必须 <= `0xFF`；`btoa('中文')` 抛 `TypeError` | `ArrayBuffer` **或**字符串（字符串按 UTF-8 编码） |
//! | 解码方向输出 | **字符串**（每字节 → 一个码位，Latin-1） | `ArrayBuffer`（二进制安全） |
//!
//! 一句话：`btoa` 处理"二进制字符串"，`$.base64` 处理真实字节。同一份字节两者输出相同。
//!
//! 这里严格照规范实现（包括 `atob` 容忍缺失填充、对非法字符抛 `TypeError`），
//! 避免出现「引擎的 atob 和浏览器不一样」这种更糟的不一致。

use rquickjs::{Ctx, Exception, Function, Result as QjsResult};

/// 抛出真正的 `TypeError`（与浏览器一致；`throw_message` 造出来的是普通 `Error`）。
fn type_error<'js>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, message)
}

/// base64 字母表。
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 把 `atob` / `btoa` 挂到全局对象上。
pub fn register<'js>(ctx: &Ctx<'js>) -> QjsResult<()> {
    let globals = ctx.globals();
    globals.set("atob", Function::new(ctx.clone(), js_atob)?)?;
    globals.set("btoa", Function::new(ctx.clone(), js_btoa)?)?;
    Ok(())
}

/// `btoa(data) -> string`
///
/// 输入是「二进制字符串」：每个 UTF-16 码位必须落在 `0x00..=0xFF`。
fn js_btoa<'js>(ctx: Ctx<'js>, data: String) -> QjsResult<String> {
    let mut bytes = Vec::with_capacity(data.len());
    for ch in data.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return Err(type_error(
                &ctx,
                &format!(
                    "btoa 只接受 0x00~0xFF 的码位（二进制字符串），收到 U+{code:04X}；\
                     要编码 UTF-8 文本请用 $.base64(text)"
                ),
            ));
        }
        bytes.push(code as u8);
    }
    Ok(encode(&bytes))
}

/// `atob(data) -> string`
///
/// 解码成「二进制字符串」：每字节还原成一个码位。容忍缺失填充，非法字符抛异常。
fn js_atob<'js>(ctx: Ctx<'js>, data: String) -> QjsResult<String> {
    // 规范要求先剥掉 ASCII 空白
    let cleaned: String = data
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '\t' | '\n' | '\u{000C}' | '\r'))
        .collect();

    let bytes = decode(&cleaned).map_err(|message| type_error(&ctx, &message))?;

    // Latin-1：每字节 → 一个码位
    Ok(bytes.iter().map(|byte| char::from(*byte)).collect())
}

/// 标准 base64（带 `=` 填充）。
fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// 标准 base64 解码：非法字符/长度抛错，缺失填充由国家容忍（按规范补齐）。
fn decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 == 1 {
        return Err("atob 输入长度非法（base64 长度不能 mod 4 余 1）".to_string());
    }

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;

    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'=' {
            // 填充只能出现在末尾（最多两个）
            let rest = &bytes[index..];
            if rest.iter().any(|b| *b != b'=') || rest.len() > 2 {
                return Err("atob 输入的填充位置非法".to_string());
            }
            break;
        }

        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            other => {
                return Err(format!(
                    "atob 输入含非法字符 {:?}（位置 {index}）",
                    char::from(*other)
                ))
            }
        };

        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_pads_correctly() {
        // 三种长度（需要 0/1/2 个填充）都要正确
        assert_eq!(encode(b"a"), "YQ==");
        assert_eq!(encode(b"ab"), "YWI=");
        assert_eq!(encode(b"abc"), "YWJj");
        assert_eq!(decode("YQ==").unwrap(), b"a");
        assert_eq!(decode("YWI=").unwrap(), b"ab");
        assert_eq!(decode("YWJj").unwrap(), b"abc");
    }

    #[test]
    fn decode_tolerates_missing_padding() {
        assert_eq!(decode("YQ").unwrap(), b"a", "规范允许省略填充");
        assert_eq!(decode("YWI").unwrap(), b"ab");
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert!(decode("YQ===").is_err(), "填充过多");
        assert!(decode("Y=Q=").is_err(), "填充位置非法");
        assert!(decode("Y!Q=").is_err(), "非法字符");
        assert!(decode("Y").is_err(), "长度 mod 4 余 1");
    }
}
