//! 协议层：RFC4648 base32 无填充编解码、CRC32（IEEE，与 zlib/JS 自实现一致）、
//! 键盘帧（协议 A）与二维码帧（协议 B）的构造与解析。
//!
//! 统一约定：**CRC32 的输入是拼包后的 base32 字符串字节**（不解码先校验）。

use data_encoding::BASE32_NOPAD;

/// 键盘帧魔术头（含协议版本）。
pub const CLIP_MAGIC: &str = "clipbeam1";
/// 键盘帧魔术头（v2：zstd 压缩）。
pub const CLIP_MAGIC_V2: &str = "clipbeam2";
/// 二维码帧魔术前缀（ClipBeam v1）。
pub const QR_MAGIC: &str = "CB1";
/// 键盘帧：起始 / 字段分隔（数字行 0，免 Shift）。
pub const FIELD_SEP: char = '0';
/// 键盘帧：整帧结束（数字行 1，免 Shift）。
pub const FRAME_END: char = '1';
/// 二维码帧字段分隔符（属于 QR alphanumeric 字符集）。
pub const QR_SEP: char = '.';

// ---------------------------------------------------------------------------
// base32
// ---------------------------------------------------------------------------

/// 编码为小写无填充 base32（键盘通道使用）。
pub fn b32_encode_lower(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes).to_ascii_lowercase()
}

/// 编码为大写无填充 base32（二维码 alphanumeric 模式使用）。
#[allow(unused)]
pub fn b32_encode_upper(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes)
}

/// 解码无填充 base32，大小写不敏感；忽略首尾空白。
pub fn b32_decode(input: &str) -> Result<Vec<u8>, String> {
    let norm: String = input
        .trim()
        .as_bytes()
        .iter()
        .map(|&b| b.to_ascii_uppercase() as char)
        .collect();
    if norm.is_empty() {
        return Ok(Vec::new());
    }
    // 无填充 base32 合法长度 mod 8 ∈ {0,2,4,5,7}；末尾补 = 供解码。
    let pad = (8 - norm.len() % 8) % 8;
    let mut padded = norm;
    padded.extend(std::iter::repeat_n('=', pad));
    BASE32_NOPAD
        .decode(padded.trim_end_matches('=').as_bytes())
        .map_err(|e| format!("base32 解码失败: {e}"))
}

// ---------------------------------------------------------------------------
// CRC32（IEEE 802.3，多项式 0xEDB88320，与常见 crc32 / JS 表驱动实现一致）
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = CRC_TABLE[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// 4 字节大端 → 7 字符小写 base32。
pub fn crc_b32(crc: u32) -> String {
    b32_encode_lower(&crc.to_be_bytes())
}

// ---------------------------------------------------------------------------
// 协议 A：键盘帧
// ---------------------------------------------------------------------------

/// 构造键盘帧：`0 clipbeam1 0 <payload> 0 <digest> 1`（无空格）。
pub fn build_keyboard_frame(text: &str) -> String {
    let payload = b32_encode_lower(text.as_bytes());
    let digest = crc_b32(crc32(payload.as_bytes()));
    format!("{FIELD_SEP}{CLIP_MAGIC}{FIELD_SEP}{payload}{FIELD_SEP}{digest}{FRAME_END}")
}

/// 构造 zstd 压缩键盘帧：`0 clipbeam2 0 <payload> 0 <digest> 1`。
///
/// 压缩等级 3；若压缩后反而更大（短文本常见），回退到 v1 未压缩帧。
pub fn build_keyboard_frame_compressed(text: &str) -> String {
    match zstd::encode_all(text.as_bytes(), 3) {
        Ok(compressed) if compressed.len() < text.len() => {
            let payload = b32_encode_lower(&compressed);
            let digest = crc_b32(crc32(payload.as_bytes()));
            format!("{FIELD_SEP}{CLIP_MAGIC_V2}{FIELD_SEP}{payload}{FIELD_SEP}{digest}{FRAME_END}")
        }
        _ => build_keyboard_frame(text),
    }
}

// ---------------------------------------------------------------------------
// 协议 B：二维码帧
// ---------------------------------------------------------------------------

/// 一帧二维码的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrFrame {
    pub total: usize,
    pub index: usize,
    pub crc: u32,
    /// 本分块的大写 base32 数据。
    pub data: String,
}

/// 构造一帧二维码文本（发送端在 JS 里实现同构逻辑；Rust 侧仅测试用）。
#[cfg(test)]
pub fn build_qr_frame(total: usize, index: usize, crc: u32, chunk: &str) -> String {
    // chunk 已是大写 base32；crc 摘要用大写 7 字符。
    let digest = b32_encode_upper(&crc.to_be_bytes());
    format!("{QR_MAGIC}{QR_SEP}{total}{QR_SEP}{index}{QR_SEP}{digest}{QR_SEP}{chunk}")
}

/// 解析一帧二维码文本；非本协议 / 字段非法返回 None（调用方直接忽略）。
pub fn parse_qr_frame(raw: &str) -> Option<QrFrame> {
    let s = raw.trim();
    let parts: Vec<&str> = s.split(QR_SEP).collect();
    if parts.len() != 5 || parts[0] != QR_MAGIC {
        return None;
    }
    let total: usize = parts[1].parse().ok()?;
    let index: usize = parts[2].parse().ok()?;
    if total == 0 || index >= total || parts[3].len() != 7 {
        return None;
    }
    let crc_bytes = b32_decode(parts[3]).ok()?;
    if crc_bytes.len() != 4 {
        return None;
    }
    let crc = u32::from_be_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
    let data = parts[4].to_ascii_uppercase();
    let valid_b32_upper = data
        .bytes()
        .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b));
    if !valid_b32_upper {
        return None;
    }
    Some(QrFrame {
        total,
        index,
        crc,
        data,
    })
}

/// 把大写 base32 分块切成等长块（最后一块可能更短）。
#[cfg(test)]
pub fn chunk_base32(data: &str, chunk_size: usize) -> Vec<String> {
    data.as_bytes()
        .chunks(chunk_size)
        .map(|c| String::from_utf8(c.to_vec()).expect("ascii"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn base32_roundtrip() {
        for text in [
            "",
            "a",
            "ab",
            "abc",
            "hello world",
            "中文/emoji 🎉 测试",
            "x",
        ] {
            let enc = b32_encode_lower(text.as_bytes());
            assert!(enc.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')));
            let dec = b32_decode(&enc).unwrap();
            assert_eq!(String::from_utf8(dec).unwrap(), text);
        }
        // 大写输入也应能解码
        let upper = b32_encode_upper("ClipBeam".as_bytes());
        assert_eq!(
            String::from_utf8(b32_decode(&upper).unwrap()).unwrap(),
            "ClipBeam"
        );
    }

    #[test]
    fn crc_digest_is_seven_chars() {
        assert_eq!(crc_b32(crc32(b"")).len(), 7);
        assert_eq!(crc_b32(crc32(b"abc")).len(), 7);
    }

    #[test]
    fn keyboard_frame_roundtrip() {
        let frame = build_keyboard_frame("你好，ClipBeam 123");
        assert!(frame.starts_with(FIELD_SEP));
        assert!(frame.ends_with(FRAME_END));
        // 解析各字段：0 magic 0 payload 0 digest 1
        let fields: Vec<&str> = frame.split(FIELD_SEP).collect();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0], "");
        assert_eq!(fields[1], CLIP_MAGIC);
        assert!(fields[2]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(fields[3]
            .chars()
            .take(fields[3].len() - 1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_eq!(fields[3].chars().last(), Some(FRAME_END));
        // payload 与 digest 校验
        let payload = fields[2];
        let digest_field = fields[3].trim_end_matches(FRAME_END);
        assert_eq!(digest_field, crc_b32(crc32(payload.as_bytes())));
        let decoded = b32_decode(payload).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "你好，ClipBeam 123");
    }

    #[test]
    fn qr_frame_roundtrip() {
        let payload = b32_encode_upper("二维码通道测试".as_bytes());
        let crc = crc32(payload.as_bytes());
        let chunks = chunk_base32(&payload, 8);
        for (i, chunk) in chunks.iter().enumerate() {
            let text = build_qr_frame(chunks.len(), i, crc, chunk);
            let f = parse_qr_frame(&text).expect("应能解析");
            assert_eq!(f.total, chunks.len());
            assert_eq!(f.index, i);
            assert_eq!(f.crc, crc);
            assert_eq!(&f.data, chunk);
        }
    }

    #[test]
    fn qr_frame_rejects_garbage() {
        assert!(parse_qr_frame("https://example.com").is_none());
        assert!(parse_qr_frame("CB1.0.0.aaaaaaa.x").is_none()); // total=0
        assert!(parse_qr_frame("CB1.2.5.aaaaaaa.x").is_none()); // index>=total
        assert!(parse_qr_frame("CB1.2.0.short.x").is_none()); // digest 长度错
        assert!(parse_qr_frame("CB1.2.0.aaaaaaa.A!B").is_none()); // 非法字符
    }

    #[test]
    fn reassembly_and_verify() {
        // 模拟接收端组包全过程
        let text = "组包完整性校验：hello 🌏";
        let payload = b32_encode_upper(text.as_bytes());
        let crc = crc32(payload.as_bytes());
        let chunks = chunk_base32(&payload, 8);
        let mut got = vec![String::new(); chunks.len()];
        for (i, chunk) in chunks.iter().enumerate() {
            let f = parse_qr_frame(&build_qr_frame(chunks.len(), i, crc, chunk)).unwrap();
            got[f.index] = f.data;
        }
        let joined = got.join("");
        assert_eq!(crc32(joined.as_bytes()), crc);
        let decoded = b32_decode(&joined).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), text);
    }
}
