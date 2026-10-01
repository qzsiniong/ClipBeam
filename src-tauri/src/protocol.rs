//! 协议 v2：按**方向**分组的统一协议。
//!
//! | 组 | 方向 | 介质 | 帧 |
//! |---|---|---|---|
//! | **K** | 宿主 → 远程 | 键盘逐字符 | `0 <magic> 0 <flags> 0 <crc> 0 <tail…> 1` |
//! | **Q** | 远程 → 宿主 | 二维码截屏 | `<magic>.<total>.<index>.<crc>.<payload>` |
//!
//! 协议 C（自解压引导页）是**一次性引导载体**，不是协议，不在这里。
//!
//! # K 通道的字符集纪律
//!
//! K 通道是**逐键敲出来**的，所以只能用免 Shift 的字符，并且要有无歧义的定界符：
//!
//! > **帧内任何字段都不含 `0` / `1`。凡是自然表示里可能出现它们的字段，
//! > 一律先 `base32 小写无填充`。**
//!
//! 于是 `0` 只可能是字段分隔符、`1` 只可能是整帧终结符。base32 小写字母表是
//! `[a-z2-7]`（RFC4648），它是 `[a-z0-9]` 的子集 —— 整条流只用字母数字、无一需要 Shift。
//!
//! | 字段 | 编码 | 例 |
//! |---|---|---|
//! | 数值 | [`num_b32`]：十进制串 → UTF-8 → base32 | `4096` → `gqydsnq` |
//! | 摘要 | base32(原始字节) | CRC32 4B → 7 字符；MD5 16B → 26 字符 |
//! | 文件名 / 文本 / 载荷 | base32(UTF-8 字节) | — |
//! | `flags` | 单个 base32 数字（5 bit → `a`..`v`） | zstd 开 = `b` |
//!
//! **CRC 的计算对象是「tail 在线上出现的那个子串」**（编码后的字符），不是解码后的字节。
//! 好处有二：不必先解码就能验帧；而且它一并覆盖 `seq`/`total`/`size` 这些头部字段，
//! 字段被改坏也能当场发现。
//!
//! # 为什么 magic 就是消息类型
//!
//! `kba` / `kbb` / `kbc` 既是「这是 ClipBeam 的帧」也是「这是什么消息」。不另设
//! `type` 与版本字段：新增消息 = 新增 magic，旧接收端遇到不认识的 magic **整帧忽略**，
//! 不会半解析出错。这比「一个 magic + 可选的 type/version」少两个字段、少一层分支。
//!
//! # 关于本模块里的 `#[allow(dead_code)]`
//!
//! 这里是两条协议的**唯一真源**，但 Rust 侧的生产代码只用到其中一部分：
//! 只**构造** `kba`（`send.rs`）与**解析** `QBA`（`receive.rs`）；`kbb`/`kbc` 的构造在
//! 宿主机脚本里（`seed/04-file-transfer.ts`，跑在 QuickJS 里，调不到 Rust 函数）。
//! 与其把这些条目删掉让格式失去单一出处（然后测试各写一份解析器），不如保留并**显式**
//! 标注「有意留的」。用逐条 `allow` 而不是模块级 `#![allow(dead_code)]`，
//! 是为了别把将来真正长出来的死代码也一起盖住。

use data_encoding::BASE32_NOPAD;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// K 通道字段分隔符。
pub const K_SEP: char = '0';
/// K 通道整帧终结符。
pub const K_END: char = '1';

/// K：剪贴板文本。
pub const K_MAGIC_TEXT: &str = "kba";
/// K：文件探测（开启一次文件传输）。
#[allow(dead_code)]
pub const K_MAGIC_PROBE: &str = "kbb";
/// K：文件分片。
#[allow(dead_code)]
pub const K_MAGIC_CHUNK: &str = "kbc";

/// Q 通道字段分隔符（属于 QR alphanumeric 字符集）。
pub const Q_SEP: char = '.';
/// Q：剪贴板载荷。
pub const Q_MAGIC_CLIP: &str = "QBA";
/// Q：控制消息（JSON）。
pub const Q_MAGIC_CONTROL: &str = "QBB";

/// `flags` 的 bit0：该帧载荷（或整文件）经 zstd。
pub const FLAG_ZSTD: u8 = 1;

/// base32 小写字母表 —— 也是「字段只允许出现哪些字符」的真源。
const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

// ---------------------------------------------------------------------------
// base32
// ---------------------------------------------------------------------------

/// 编码为小写无填充 base32（K 通道使用）。
pub fn b32_encode_lower(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes).to_ascii_lowercase()
}

/// 编码为大写无填充 base32（Q 通道使用：属于 QR alphanumeric 字符集）。
pub fn b32_encode_upper(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes)
}

/// 解码无填充 base32，大小写不敏感；忽略首尾空白。
pub fn b32_decode(input: &str) -> Result<Vec<u8>, String> {
    let norm: String = input
        .trim()
        .bytes()
        .map(|b| b.to_ascii_uppercase() as char)
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

/// 字段是否**非空**且只含 base32 小写字母表字符。
#[allow(dead_code)]
fn is_b32_field(field: &str) -> bool {
    !field.is_empty() && field.bytes().all(|b| B32.contains(&b))
}

/// 字段是否只含 base32 小写字符（**允许为空** —— 只有载荷字段该这么放宽）。
#[allow(dead_code)]
fn is_b32_chars(field: &str) -> bool {
    field.bytes().all(|b| B32.contains(&b))
}

// ---------------------------------------------------------------------------
// CRC32
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

/// CRC-32（IEEE 802.3，多项式 `0xEDB88320`）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = CRC_TABLE[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// CRC32 → 7 字符小写 base32（K 通道的 `crc` 字段）。
pub fn crc_b32(crc: u32) -> String {
    b32_encode_lower(&crc.to_be_bytes())
}

/// CRC32 → 7 字符大写 base32（Q 通道的 `crc` 字段）。
pub fn crc_b32_upper(crc: u32) -> String {
    b32_encode_upper(&crc.to_be_bytes())
}

// ---------------------------------------------------------------------------
// 数值与 flags
// ---------------------------------------------------------------------------

/// 数值字段的统一编码：十进制串的 UTF-8 字节 → base32 小写。
///
/// 十进制数字里会出现 `0`/`1`，但它们只存在于**输入侧**；base32 的输出必然落在
/// `[a-z2-7]`，所以编码结果永远不会撞上定界符。
#[allow(dead_code)]
pub fn num_b32(n: u64) -> String {
    b32_encode_lower(n.to_string().as_bytes())
}

/// [`num_b32`] 的逆；解出的必须恰好是十进制数字串。
#[allow(dead_code)]
pub fn num_parse_b32(s: &str) -> Option<u64> {
    if !is_b32_field(s) {
        return None;
    }
    let bytes = b32_decode(s).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    // `u64::from_str` 会拒绝 `+`/`-`/空白，正是我们要的严格度
    text.parse::<u64>().ok()
}

/// `flags` 值（0..=31）→ 单个 base32 字符。
fn flags_char(value: u8) -> Option<char> {
    B32.get(value as usize).map(|&b| b as char)
}

/// 单个 base32 字符 → `flags` 值。
#[allow(dead_code)]
fn parse_flags(ch: u8) -> Option<u8> {
    B32.iter().position(|&b| b == ch).map(|v| v as u8)
}

// ---------------------------------------------------------------------------
// K 帧
// ---------------------------------------------------------------------------

/// 解析出来的 K 帧。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum KFrame {
    /// `kba`：剪贴板文本。`payload` 是 base32 文本（`zstd` 为真时是压缩后的字节）。
    Text { zstd: bool, payload: String },
    /// `kbb`：文件探测。`file_md5` 是 base32(原始 16 字节) = 26 字符。
    Probe {
        zstd: bool,
        total: u64,
        size: u64,
        chunk_size: u64,
        name: String,
        file_md5: String,
    },
    /// `kbc`：文件分片。`payload` 是该片**线上字节**的 base32。
    Chunk {
        zstd: bool,
        seq: u64,
        total: u64,
        size: u64,
        chunk_size: u64,
        payload: String,
    },
}

/// 组装一帧：`0 <magic> 0 <flags> 0 <crc> 0 <tail> 1`。
///
/// CRC 覆盖 `magic 0 flags 0 tail` —— 也就是**除引导符与 `crc` 字段本身之外的全部内容**。
/// 注意它不是帧里的连续子串（`0<crc>0` 夹在中间），而是一个明确定义的拼接。
///
/// 为什么把 `flags` 也纳入覆盖：`flags` 的 bit0 决定「载荷是否要解压」，若它落在校验之外，
/// 一个字符被改坏就会**静默**走进错误的解码分支（把未压缩的当压缩解，反之亦然）。
/// 一个以完整性为目的的协议不该留这种字段在校验之外。
fn build_k(magic: &str, flags: u8, tail: &str) -> String {
    let flags = flags_char(flags).expect("flags 必须在 0..=31");
    let crc = crc_b32(crc32(crc_body(magic, flags, tail).as_bytes()));
    format!("{K_SEP}{magic}{K_SEP}{flags}{K_SEP}{crc}{K_SEP}{tail}{K_END}")
}

/// CRC 的被覆盖内容：`magic 0 flags 0 tail`。
fn crc_body(magic: &str, flags: char, tail: &str) -> String {
    format!("{magic}{K_SEP}{flags}{K_SEP}{tail}")
}

/// 组剪贴板文本帧。
///
/// `compress` 为真时尝试 zstd；**压不小就自动回退未压缩**（短文本压实常更大），
/// 并相应地设置 `flags` 的 zstd 位。
pub fn build_text_frame(text: &str, compress: bool) -> String {
    if compress {
        if let Ok(compressed) = zstd::encode_all(text.as_bytes(), 3) {
            if compressed.len() < text.len() {
                return build_k(K_MAGIC_TEXT, FLAG_ZSTD, &b32_encode_lower(&compressed));
            }
        }
        // 压缩无收益或失败：退回未压缩 —— 一个可选优化不该影响送达
    }
    build_k(K_MAGIC_TEXT, 0, &b32_encode_lower(text.as_bytes()))
}

/// 组文件探测帧。`name` 是**明文**文件名（内部做 base32）。
/// `file_md5` 是 base32(原始 16 字节) 的 26 字符串。
#[allow(dead_code)]
pub fn build_probe_frame(
    total: u64,
    size: u64,
    chunk_size: u64,
    name: &str,
    file_md5: &str,
    zstd: bool,
) -> String {
    let tail = [
        num_b32(total),
        num_b32(size),
        num_b32(chunk_size),
        b32_encode_lower(name.as_bytes()),
        file_md5.to_string(),
    ]
    .join(&K_SEP.to_string());
    build_k(K_MAGIC_PROBE, if zstd { FLAG_ZSTD } else { 0 }, &tail)
}

/// 组文件分片帧。`payload` 是该片**线上字节**的 base32。
#[allow(dead_code)]
pub fn build_chunk_frame(
    seq: u64,
    total: u64,
    size: u64,
    chunk_size: u64,
    payload: &str,
    zstd: bool,
) -> String {
    let tail = [
        num_b32(seq),
        num_b32(total),
        num_b32(size),
        num_b32(chunk_size),
        payload.to_string(),
    ]
    .join(&K_SEP.to_string());
    build_k(K_MAGIC_CHUNK, if zstd { FLAG_ZSTD } else { 0 }, &tail)
}

/// 解析 K 帧。不是本协议的帧、字段非法、或 CRC 不符时返回 `None`。
///
/// 严格是刻意的：键盘通道会丢键/重复键，宽松解析会把坏帧当好帧，反而更难定位。
/// 宿主端有缺失区间重传兜底，所以「丢弃坏帧」是正确策略。
#[allow(dead_code)]
pub fn parse_k_frame(raw: &str) -> Option<KFrame> {
    if !raw.starts_with(K_SEP) || !raw.ends_with(K_END) {
        return None;
    }
    let body = &raw[1..raw.len() - 1];
    let parts: Vec<&str> = body.split(K_SEP).collect();
    // magic / flags / crc / 至少一个 tail 字段
    if parts.len() < 4 {
        return None;
    }

    let magic = parts[0];
    // flags 与 crc 都是单字符/定宽字段，宽度先卡死
    if parts[1].len() != 1 || parts[2].len() != 7 {
        return None;
    }
    let flags = parse_flags(parts[1].as_bytes()[0])?;
    // 保留位必须为 0：没有版本字段，所以「将来才定义的位」只能拒绝，不能猜
    if flags > FLAG_ZSTD {
        return None;
    }
    let zstd = flags & FLAG_ZSTD != 0;

    // CRC 覆盖 `magic 0 flags 0 tail`（除引导符与 crc 字段本身外的一切）
    let tail = parts[3..].join(&K_SEP.to_string());
    let expect = crc_b32(crc32(
        crc_body(magic, parts[1].as_bytes()[0] as char, &tail).as_bytes(),
    ));
    if expect != parts[2] {
        return None;
    }

    let fields = &parts[3..];
    match magic {
        K_MAGIC_TEXT => {
            if fields.len() != 1 {
                return None;
            }
            let payload = fields[0];
            // 载荷允许为空（空文本）；其余情况一律要求 base32 小写
            if !is_b32_chars(payload) {
                return None;
            }
            Some(KFrame::Text {
                zstd,
                payload: payload.to_string(),
            })
        }
        K_MAGIC_PROBE => {
            if fields.len() != 5 {
                return None;
            }
            let (total, size, chunk_size, name, file_md5) =
                (fields[0], fields[1], fields[2], fields[3], fields[4]);
            if ![total, size, chunk_size, name, file_md5]
                .iter()
                .all(|f| is_b32_field(f))
            {
                return None;
            }
            let total = num_parse_b32(total)?;
            let size = num_parse_b32(size)?;
            let chunk_size = num_parse_b32(chunk_size)?;
            if total < 1 || chunk_size < 1 {
                return None;
            }
            if file_md5.len() != 26 {
                return None;
            }
            let name = String::from_utf8(b32_decode(name).ok()?).ok()?;
            Some(KFrame::Probe {
                zstd,
                total,
                size,
                chunk_size,
                name,
                file_md5: file_md5.to_string(),
            })
        }
        K_MAGIC_CHUNK => {
            if fields.len() != 5 {
                return None;
            }
            let (seq, total, size, chunk_size, payload) =
                (fields[0], fields[1], fields[2], fields[3], fields[4]);
            if ![seq, total, size, chunk_size]
                .iter()
                .all(|f| is_b32_field(f))
            {
                return None;
            }
            let seq = num_parse_b32(seq)?;
            let total = num_parse_b32(total)?;
            let size = num_parse_b32(size)?;
            let chunk_size = num_parse_b32(chunk_size)?;
            if total < 1 || chunk_size < 1 || seq >= total {
                return None;
            }
            // 载荷允许为空（0 字节文件切出一片空载荷）
            if !is_b32_chars(payload) {
                return None;
            }
            Some(KFrame::Chunk {
                zstd,
                seq,
                total,
                size,
                chunk_size,
                payload: payload.to_string(),
            })
        }
        // 未知 magic：整帧忽略（前向兼容）
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Q 帧
// ---------------------------------------------------------------------------

/// 解析出来的 Q 帧（一个二维码就是一个完整帧，所以不需要终结符）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QFrame {
    pub magic: String,
    pub total: usize,
    pub index: usize,
    pub crc: String,
    /// base32 **大写** 的载荷。
    pub payload: String,
}

/// 整条消息 payload 的摘要（`base32` 大写，7 字符）。
///
/// **同一批的每一帧都带同一个值** —— 它既是批量身份（换消息就换批次），
/// 也是重组完成后的整体校验。这与 K 通道「每帧各算各的」是**刻意不同**的：
/// Q 通道的若干帧是一组轮播的二维码，不是独立投递；把校验放在整条消息上，
/// 才能同时回答「这几帧是不是一起的」和「拼出来对不对」。
pub fn q_payload_crc(payload: &str) -> String {
    crc_b32_upper(crc32(payload.as_bytes()))
}

/// 组一帧 Q：`<magic>.<total>.<index>.<crc>.<payload>`。
///
/// * `crc` 用 [`q_payload_crc`] 算整条消息的摘要，同批各帧一致；
/// * `total`/`index` 用十进制：Q 通道是二维码，不受键盘的免 Shift 约束，
///   没必要为它们做 base32 膨胀。
#[allow(dead_code)]
pub fn build_q_frame(magic: &str, total: usize, index: usize, crc: &str, payload: &str) -> String {
    format!("{magic}{Q_SEP}{total}{Q_SEP}{index}{Q_SEP}{crc}{Q_SEP}{payload}")
}

/// 载荷是否只含 base32 **大写** 字符（允许为空 —— 空剪贴板）。
fn is_b32_upper_chars(field: &str) -> bool {
    field
        .bytes()
        .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
}

/// 解析 Q 帧。不是本协议的帧、字段非法、或 CRC 不符时返回 `None`。
pub fn parse_q_frame(raw: &str) -> Option<QFrame> {
    let parts: Vec<&str> = raw.trim().split(Q_SEP).collect();
    if parts.len() != 5 {
        return None;
    }
    let (magic, total, index, crc, payload) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    if magic != Q_MAGIC_CLIP && magic != Q_MAGIC_CONTROL {
        return None;
    }
    if crc.len() != 7 || !is_b32_upper_chars(crc) {
        return None;
    }
    let total: usize = total.parse().ok()?;
    let index: usize = index.parse().ok()?;
    if total < 1 || index >= total {
        return None;
    }
    if !is_b32_upper_chars(payload) {
        return None;
    }
    // 这里**不**逐帧验 crc：它是整条消息的摘要，只有重组完成后才能验
    // （见 `q_payload_crc` 的说明）。所以本函数只做形状与字符集校验。
    Some(QFrame {
        magic: magic.to_string(),
        total,
        index,
        crc: crc.to_string(),
        payload: payload.to_string(),
    })
}

/// Q 帧的批次键：换了一批数据（magic/total/crc 变了）就整批重来。
#[allow(dead_code)]
pub fn q_batch_key(frame: &QFrame) -> (String, usize, String) {
    (frame.magic.clone(), frame.total, frame.crc.clone())
}

// ---------------------------------------------------------------------------
// 缺失区间
// ---------------------------------------------------------------------------

/// `[(0,2),(7,7),(9,11)]` → `"0-2,7,9-11"`；空切片 → 空串。
#[allow(dead_code)]
pub fn format_missing_ranges(ranges: &[(usize, usize)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// [`format_missing_ranges`] 的逆。
///
/// 严格校验：只允许 `[0-9,-]`；区间必须 `a <= b`、**递增且不重叠**；不接受空段
/// （`"1,,2"`）、首尾逗号、以及任何非数字。空串 → `Some(vec![])`。
/// 严格是刻意的 —— 一个畸形的区间清单比没有清单更危险：宿主会照着重传。
#[allow(dead_code)]
pub fn parse_missing_ranges(s: &str) -> Option<Vec<(usize, usize)>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b',' || b == b'-')
    {
        return None;
    }

    let mut out: Vec<(usize, usize)> = Vec::new();
    for part in s.split(',') {
        if part.is_empty() {
            return None;
        }
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => {
                // 多一个 `-`、或任一侧为空，都算非法
                if a.is_empty() || b.is_empty() || b.contains('-') {
                    return None;
                }
                (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)
            }
            None => {
                let n = part.parse::<usize>().ok()?;
                (n, n)
            }
        };
        if start > end {
            return None;
        }
        // 递增且不重叠
        if let Some(&(_, prev_end)) = out.last() {
            if start <= prev_end {
                return None;
            }
        }
        out.push((start, end));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 金标向量（golden vectors）─────────────────────────────────────────
    //
    // 下面这些整帧字面量是**用独立实现**（Python 的 base64.b32encode + zlib.crc32）
    // 算出来再抄进来的，不是跑本模块打印出来的 —— 否则实现写错时金标会跟着一起错。
    // 改动它们就等于改协议，必须是有意为之。

    #[test]
    fn golden_text_frame() {
        // tail = base32lower("hi") = "nbuq"；crc 覆盖 `kba0a0nbuq`
        let frame = build_text_frame("hi", false);
        assert_eq!(frame, "0kba0a0rnjqzoq0nbuq1");
        assert_eq!(
            parse_k_frame(&frame),
            Some(KFrame::Text {
                zstd: false,
                payload: "nbuq".to_string()
            })
        );
    }

    #[test]
    fn golden_chunk_frame() {
        // payload = base32lower([0,1,2,3]) = "aaaqeay"
        // seq=0 total=1 size=10 chunkSize=4 → ga / ge / geya / gq
        let payload = b32_encode_lower(&[0u8, 1, 2, 3]);
        assert_eq!(payload, "aaaqeay");
        let frame = build_chunk_frame(0, 1, 10, 4, &payload, false);
        assert_eq!(frame, "0kbc0a0rdi6yqi0ga0ge0geya0gq0aaaqeay1");
        assert_eq!(
            parse_k_frame(&frame),
            Some(KFrame::Chunk {
                zstd: false,
                seq: 0,
                total: 1,
                size: 10,
                chunk_size: 4,
                payload,
            })
        );
    }

    #[test]
    fn golden_probe_frame() {
        // total=1 size=4 chunkSize=2 name="a.bin" fileMd5=26×"a"
        let frame = build_probe_frame(1, 4, 2, "a.bin", &"a".repeat(26), false);
        assert_eq!(
            frame,
            "0kbb0a0ifyihly0ge0gq0gi0mexge2lo0aaaaaaaaaaaaaaaaaaaaaaaaaa1"
        );
        match parse_k_frame(&frame) {
            Some(KFrame::Probe { name, file_md5, .. }) => {
                assert_eq!(name, "a.bin");
                assert_eq!(file_md5, "a".repeat(26));
            }
            other => panic!("应当解析为探测帧，实际 {other:?}"),
        }
    }

    #[test]
    fn golden_q_frame() {
        // payload = base32upper("hello") = "NBSWY3DP"；crc = 整条 payload 的摘要
        let payload = b32_encode_upper(b"hello");
        assert_eq!(payload, "NBSWY3DP");
        assert_eq!(q_payload_crc(&payload), "4XPME4A");
        let frame = build_q_frame(Q_MAGIC_CLIP, 1, 0, &q_payload_crc(&payload), &payload);
        assert_eq!(frame, "QBA.1.0.4XPME4A.NBSWY3DP");
        let parsed = parse_q_frame(&frame).unwrap();
        assert_eq!(parsed.magic, Q_MAGIC_CLIP);
        assert_eq!(parsed.payload, payload);
        assert_eq!(parsed.total, 1);
        assert_eq!(parsed.index, 0);
    }

    // ── 字符集不变量 ───────────────────────────────────────────────────────

    /// 除定界符外，每个字段都必须落在 `[a-z2-7]` —— 协议正确性的根基。
    #[test]
    fn every_field_is_base32_lowercase() {
        let text = "Hello 世界 123";
        let payload = b32_encode_lower(text.as_bytes());
        let frames = [
            build_text_frame(text, false),
            build_probe_frame(3, 10_000, 4096, "报告 final.pdf", &"a".repeat(26), true),
            build_chunk_frame(1, 3, 10_000, 4096, &payload, false),
        ];
        for frame in frames {
            assert!(
                frame.starts_with(K_SEP) && frame.ends_with(K_END),
                "{frame}"
            );
            let body = &frame[1..frame.len() - 1];
            for field in body.split(K_SEP) {
                assert!(
                    is_b32_field(field),
                    "字段 {field:?} 超出 base32 小写字母表：{frame}"
                );
            }
        }
    }

    /// 任何数值/文件名/单字节都不会在编码后带进 `0`/`1`（定界成立的前提）。
    #[test]
    fn encoded_fields_never_contain_separators() {
        for n in 0..5000u64 {
            let s = num_b32(n);
            assert!(is_b32_field(&s), "num_b32({n}) = {s}");
        }
        for name in ["a", "报告", "with space.pdf", "emoji-🎯.bin"] {
            let s = b32_encode_lower(name.as_bytes());
            assert!(is_b32_field(&s), "name {name:?} → {s}");
        }
        for byte in 0..=255u8 {
            let s = b32_encode_lower(&[byte]);
            assert!(is_b32_field(&s), "单字节 {byte} → {s}");
        }
    }

    // ── CRC 的计算对象 ─────────────────────────────────────────────────────

    /// CRC 覆盖 `magic 0 flags 0 tail`：改坏其中任何字段（含 `flags` 本身）都会失败。
    #[test]
    fn crc_covers_everything_but_the_crc_field() {
        let payload = b32_encode_lower(&[9u8, 8, 7]);
        let frame = build_chunk_frame(0, 2, 3, 2, &payload, false);

        // 篡改 seq（`0ga0` → `0gb0`），crc 不动
        let broken = frame.replacen("0ga0", "0gb0", 1);
        assert_ne!(broken, frame, "测试自身没改到东西");
        assert_eq!(parse_k_frame(&broken), None, "seq 被改坏却没被发现");

        // **篡改 flags**（`0kbc0a0` → `0kbc0b0`），crc 不动。
        // 这正是「flags 必须在校验覆盖范围内」的回归测试：它决定载荷要不要解压，
        // 漏在校验之外会让一个字符的改动静默走进错误的解码分支。
        let flags_broken = frame.replacen("0kbc0a0", "0kbc0b0", 1);
        assert_ne!(flags_broken, frame, "测试自身没改到 flags");
        assert_eq!(parse_k_frame(&flags_broken), None, "flags 被改坏却没被发现");

        // 篡改 payload 的最后一个字符
        let mut chars: Vec<char> = frame.chars().collect();
        let at = chars.len() - 2;
        chars[at] = if chars[at] == 'a' { 'b' } else { 'a' };
        let broken2: String = chars.into_iter().collect();
        assert_eq!(parse_k_frame(&broken2), None, "payload 被改坏却没被发现");
    }

    // ── 空载荷 ─────────────────────────────────────────────────────────────

    /// 0 字节文件切出一片空载荷：必须能往返，且**不会**顺带放过别处的空字段。
    #[test]
    fn empty_payload_is_valid_but_empty_other_fields_are_not() {
        let frame = build_chunk_frame(0, 1, 0, 4096, "", false);
        match parse_k_frame(&frame) {
            Some(KFrame::Chunk { payload, size, .. }) => {
                assert_eq!(payload, "");
                assert_eq!(size, 0);
            }
            other => panic!("空载荷应当可解析，实际 {other:?}"),
        }

        // 空 seq：把 seq 字段留空 → 字段数不变但该字段为空 → 拒绝
        let tail = "0ge0ga0aaaqeay0";
        let bad = format!(
            "0kbc0a0{}0{tail}1",
            crc_b32(crc32(format!("kbc0a0{tail}").as_bytes()))
        );
        assert_eq!(parse_k_frame(&bad), None, "空 seq 应被拒");
    }

    // ── 拒绝路径 ───────────────────────────────────────────────────────────

    #[test]
    fn rejects_malformed_frames() {
        let good = build_chunk_frame(0, 1, 10, 4, "aaaqeay", false);
        assert!(parse_k_frame(&good).is_some());

        assert_eq!(parse_k_frame(""), None, "空串");
        assert_eq!(parse_k_frame("kbc0a1"), None, "缺引导符");
        assert_eq!(parse_k_frame(&good[..good.len() - 1]), None, "缺终结符");
        assert_eq!(parse_k_frame("0kbd0a0aaaaaaaa0ga1"), None, "未知 magic");
        assert_eq!(
            parse_k_frame("0kbc0aa0aaaaaaaa0ga1"),
            None,
            "flags 必须是 1 字符"
        );
        assert_eq!(
            parse_k_frame("0kbc0a0aaaaaa0ga1"),
            None,
            "crc 必须是 7 字符"
        );
        // 字段数不对（少一个 tail 字段）
        let tail = "ga0ge";
        let short = format!(
            "0kbc0a0{}0{tail}1",
            crc_b32(crc32(format!("kbc0a0{tail}").as_bytes()))
        );
        assert_eq!(parse_k_frame(&short), None, "字段数不符应被拒");
    }

    #[test]
    fn rejects_implausible_numbers() {
        let payload = b32_encode_lower(b"x");
        // num(10)=geya, num(4)=gq, num(0)=ga, num(1)=ge
        for (tail, why) in [
            (format!("ga0ga0geya0gq0{payload}"), "total=0"),
            (format!("ge0ge0geya0gq0{payload}"), "seq>=total"),
            (format!("ga0ge0geya0ga0{payload}"), "chunkSize=0"),
        ] {
            let frame = format!(
                "0kbc0a0{}0{tail}1",
                crc_b32(crc32(format!("kbc0a0{tail}").as_bytes()))
            );
            assert_eq!(parse_k_frame(&frame), None, "{why} 应被拒");
        }
    }

    // ── 数值编码 ───────────────────────────────────────────────────────────

    #[test]
    fn num_roundtrip_and_charset() {
        for n in [0u64, 1, 4, 9, 31, 32, 255, 4096, 100_000, u32::MAX as u64] {
            let encoded = num_b32(n);
            assert!(is_b32_field(&encoded), "num_b32({n}) = {encoded}");
            assert_eq!(num_parse_b32(&encoded), Some(n), "num_b32({n}) 往返");
        }
        assert_eq!(num_b32(4096), "gqydsnq");
        assert_eq!(num_parse_b32("nbswy"), None, "「hi」的 base32 不是数字");
        assert_eq!(num_parse_b32(""), None, "空串");
        assert_eq!(num_parse_b32("AA"), None, "大写不在小写字母表");
    }

    // ── flags ──────────────────────────────────────────────────────────────

    /// 保留位必须为 0：没有版本字段，所以「将来才定义的位」只能拒绝，不能猜。
    ///
    /// 注意这条与上面的 CRC 测试互补：CRC 能抓「被改坏」，但抓不了「合法地算出了
    /// 一个我们还不认识的 flags」—— 那正是保留位校验要挡的。
    #[test]
    fn reserved_flag_bits_are_rejected() {
        // flags = 2（bit1）—— crc 算得完全正确，仍然必须拒
        let tail = "ga0ge0geya0gq0aaaqeay";
        let covered = format!("kbc0c0{tail}");
        let frame = format!("0kbc0c0{}0{tail}1", crc_b32(crc32(covered.as_bytes())));
        assert_eq!(parse_k_frame(&frame), None, "带保留位的 flags 应被拒");

        // flags = 3（bit0+bit1）同理
        let covered = format!("kbc0d0{tail}");
        let frame = format!("0kbc0d0{}0{tail}1", crc_b32(crc32(covered.as_bytes())));
        assert_eq!(parse_k_frame(&frame), None, "带保留位的 flags 应被拒");

        // flags = 1 仍然合法
        let covered = format!("kbc0b0{tail}");
        let frame = format!("0kbc0b0{}0{tail}1", crc_b32(crc32(covered.as_bytes())));
        assert!(parse_k_frame(&frame).is_some(), "flags=1 应当合法");
    }

    #[test]
    fn flags_roundtrip() {
        assert_eq!(flags_char(0), Some('a'));
        assert_eq!(flags_char(FLAG_ZSTD), Some('b'));
        assert_eq!(parse_flags(b'a'), Some(0));
        assert_eq!(parse_flags(b'b'), Some(FLAG_ZSTD));
        assert_eq!(parse_flags(b'0'), None, "`0` 是定界符，不能当 flags");

        // 重复文本压得动 → zstd 位置上
        match parse_k_frame(&build_text_frame(&"a".repeat(500), true)).unwrap() {
            KFrame::Text { zstd, .. } => assert!(zstd, "重复文本应当启用压缩"),
            other => panic!("应为文本帧，实际 {other:?}"),
        }
        // 短文本压不小 → 自动回退未压缩
        match parse_k_frame(&build_text_frame("hi", true)).unwrap() {
            KFrame::Text { zstd, payload } => {
                assert!(!zstd, "短文本不该启用压缩");
                assert_eq!(payload, "nbuq");
            }
            other => panic!("应为文本帧，实际 {other:?}"),
        }
    }

    /// 压缩帧解回来必须与原文逐字节一致。
    #[test]
    fn compressed_text_roundtrip() {
        let text = "ClipBeam 协议 v2 压缩往返 ".repeat(50);
        match parse_k_frame(&build_text_frame(&text, true)).unwrap() {
            KFrame::Text { zstd, payload } => {
                assert!(zstd);
                let bytes = b32_decode(&payload).unwrap();
                let plain = zstd::decode_all(bytes.as_slice()).unwrap();
                assert_eq!(String::from_utf8(plain).unwrap(), text);
            }
            other => panic!("应为文本帧，实际 {other:?}"),
        }
    }

    // ── 探测帧 ─────────────────────────────────────────────────────────────

    #[test]
    fn probe_frame_roundtrip_with_utf8_name() {
        let file_md5 = b32_encode_lower(&[0xAB; 16]);
        let frame = build_probe_frame(12, 49_152, 4096, "季度报告 final.pdf", &file_md5, true);
        match parse_k_frame(&frame).unwrap() {
            KFrame::Probe {
                zstd,
                total,
                size,
                chunk_size,
                name,
                file_md5: parsed_md5,
            } => {
                assert!(zstd);
                assert_eq!(total, 12);
                assert_eq!(size, 49_152);
                assert_eq!(chunk_size, 4096);
                assert_eq!(name, "季度报告 final.pdf");
                assert_eq!(parsed_md5, file_md5);
                assert_eq!(parsed_md5.len(), 26);
            }
            other => panic!("应为探测帧，实际 {other:?}"),
        }
    }

    // ── Q 帧 ───────────────────────────────────────────────────────────────

    #[test]
    fn q_frame_rejects_malformed() {
        let payload = b32_encode_upper(b"hi");
        let good = build_q_frame(Q_MAGIC_CLIP, 1, 0, &q_payload_crc(&payload), &payload);
        assert!(parse_q_frame(&good).is_some());

        assert_eq!(parse_q_frame(""), None);
        assert_eq!(parse_q_frame("QBC.1.0.ABCDEFG.NBSWY"), None, "未知 magic");
        assert_eq!(
            parse_q_frame(&good.replace("QBA.1.0.", "QBA.0.0.")),
            None,
            "total=0"
        );
        assert_eq!(
            parse_q_frame(&good.replace("QBA.1.0.", "QBA.1.1.")),
            None,
            "index 越界"
        );
        assert_eq!(parse_q_frame("QBA.1.0.SHORT.NBSWY"), None, "crc 长度");
        assert_eq!(parse_q_frame("QBA.1.0.ABCDEFG.nbswy"), None, "载荷必须大写");
        assert_eq!(
            parse_q_frame("QBA.1.0.abcdef g.NBSWY"),
            None,
            "crc 必须大写 base32"
        );
        assert_eq!(parse_q_frame("QBA.1.0.4XPME4A"), None, "字段数不足");
    }

    /// 同一批的每一帧带**同一个** crc（批量身份），换消息才换 crc。
    #[test]
    fn q_frames_of_one_message_share_the_same_crc() {
        // Q 的载荷在线上必然是大写 base32（纯 ASCII），按字节切分安全
        let whole = "NBSWY3DPNBSWY3DPNBSWY3DP".to_string();
        let crc = q_payload_crc(&whole);
        let f0 = build_q_frame(Q_MAGIC_CLIP, 2, 0, &crc, &whole[..10]);
        let f1 = build_q_frame(Q_MAGIC_CLIP, 2, 1, &crc, &whole[10..]);
        let p0 = parse_q_frame(&f0).unwrap();
        let p1 = parse_q_frame(&f1).unwrap();
        assert_eq!(p0.crc, p1.crc, "同一批各帧的 crc 必须一致");
        assert_eq!(q_batch_key(&p0), q_batch_key(&p1), "批次键应当相同");
    }

    /// 整条 Q 链路的往返：文本 → base32 大写 → 帧 → 解析 → 还原文本。
    #[test]
    fn q_clipboard_roundtrip() {
        let text = "远程剪贴板内容 with 中文 🎯";
        let payload = b32_encode_upper(text.as_bytes());
        let frame = build_q_frame(Q_MAGIC_CLIP, 3, 1, &q_payload_crc(&payload), &payload);
        let parsed = parse_q_frame(&frame).unwrap();
        assert_eq!(parsed.total, 3);
        assert_eq!(parsed.index, 1);
        let bytes = b32_decode(&parsed.payload).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), text);
    }

    // ── 缺失区间 ───────────────────────────────────────────────────────────

    #[test]
    fn missing_ranges_roundtrip_and_rejects() {
        let cases: &[(&[(usize, usize)], &str)] = &[
            (&[], ""),
            (&[(0, 0)], "0"),
            (&[(0, 5)], "0-5"),
            (&[(0, 2), (7, 7), (9, 11)], "0-2,7,9-11"),
        ];
        for &(ranges, text) in cases {
            assert_eq!(format_missing_ranges(ranges), text);
            assert_eq!(
                parse_missing_ranges(text),
                Some(ranges.to_vec()),
                "解析 {text:?}"
            );
        }

        for bad in [
            "1,,2",    // 空段
            ",",       // 只有逗号
            "1,",      // 尾逗号
            ",1",      // 首逗号
            "2-1",     // 起点大于终点
            "0-5,5-9", // 重叠
            "5,3",     // 递减
            "a",       // 非数字
            "1-",      // 缺终点
            "-1",      // 缺起点
            "1 - 2",   // 空白
            "1.5",     // 小数点
            "-1-2",    // 负数
            "1-2-3",   // 多一个横杠
        ] {
            assert_eq!(parse_missing_ranges(bad), None, "{bad:?} 应当被拒绝");
        }
    }

    #[test]
    fn missing_ranges_empty_string_is_empty_list() {
        assert_eq!(parse_missing_ranges(""), Some(Vec::new()));
        assert_eq!(format_missing_ranges(&[]), "");
    }
}
