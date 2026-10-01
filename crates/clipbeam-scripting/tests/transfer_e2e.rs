//! 协议 v2 的端到端测试：跑真正的 `04-file-transfer.ts`，再用**独立实现**把宿主敲出的
//! 帧解回原始字节。
//!
//! 这是整个特性最重要的守卫：脚本（发送端）、帧格式、接收页（解析端）三者的约定
//! 必须严格吻合。这里的解码器是**独立重写**的 —— 不复用 `src-tauri::protocol`，
//! 也不复用 `packages/shared` 的 TS 实现，更不调用脚本里的组帧函数；两边一起改错就通不过。
//!
//! 覆盖：正常完成 / 断点续传（只补页面报的缺口）/ **不连续缺口** / 页面已有整文件 /
//! 读不到反馈时降级 / 整文件压缩 / 0 字节文件 / 页面报错时失败。
//!
//! # K 通道切帧规则（宿主 → 远程，逐键敲出）
//!
//! 线上形态：`0 <magic> 0 <flags> 0 <crc> 0 <tail…> 1`。
//!
//! * `magic` **就是**消息类型：`kbb` 文件探测、`kbc` 文件分片（`kba` 是剪贴板文本）。
//!   旧协议的 `fuyq` 探测哨兵与版本字段都没有了。
//! * `flags` 是单个 base32 数字（`a`=0、`b`=1…，5 bit），bit0 = 整条载荷 / 整文件 zstd；
//!   **其余位保留**：置了保留位说明对端在讲本版本不认识的方言，必须整帧拒绝而不是猜着解。
//! * `crc` 是 7 字符 base32 小写。**CRC 覆盖除引导符与 `crc` 字段本身之外的全部内容**
//!   （`magic` / `flags` / `tail`），因此 `flags` 被改坏也会当场发现 —— 它决定载荷要不要
//!   解压，漏在校验之外就会静默走进错误的解码分支。注意被覆盖的不是帧里的连续子串
//!   （`0<crc>0` 夹在中间），而是一个明确定义的拼接 `magic 0 flags 0 tail`。
//! * 字符集纪律：**帧内任何字段都不含 `0` / `1`**。数字先转十进制串再 base32；
//!   文件名、载荷、MD5（原始 16 字节）同样先 base32。base32 小写字母表是 `[a-z2-7]`，
//!   于是 `0` 只可能是字段分隔、`1` 只可能是整帧终结 —— 按 `0` 切即可，没有特例，
//!   也没有旧协议那种「必须按定长读指纹字段」的补丁。
//! * tail 都是 5 个字段：
//!   - `kbb`：`<total> 0 <size> 0 <chunkSize> 0 <name> 0 <fileMd5>`；
//!   - `kbc`：`<seq> 0 <total> 0 <size> 0 <chunkSize> 0 <payload>`。
//!
//! `payload` 允许为空（0 字节文件切出的唯一一片就是空载荷）：它是最后一个字段、
//! 后面紧跟终结符 `1`，所以没有歧义。其余字段一律非空。
//!
//! # Q 通道（远程 → 宿主，二维码）
//!
//! 帧：`QBB.<total>.<index>.<crc>.<payload>`。`crc` 是**整条消息**载荷的摘要，
//! 同一批各帧一致（它同时是批量身份）；`payload` 是消息的一段，按 index 拼起来
//! 再 base32 解码，得到 `{"type":"feedback","status":…}` 的 JSON。
//! 这与 K 通道「每帧各算各的 crc」是刻意相反的。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clipbeam_scripting::{ConfirmChoice, HostError, PickKind, ScriptHost};
use data_encoding::BASE32_NOPAD;
use script_engine::ConsoleHook;

// ── 帧结构常量 ──────────────────────────────────────────────────────────────

/// K 通道字段分隔符。
const K_SEP: char = '0';
/// K 通道整帧终结符。
const K_END: char = '1';
/// `kbb`：文件探测帧的 magic。
const MAGIC_PROBE: &str = "kbb";
/// `kbc`：文件分片帧的 magic。
const MAGIC_CHUNK: &str = "kbc";
/// `flags` 的 bit0：整条载荷 / 整文件经 zstd。
const FLAG_ZSTD: u8 = 1;
/// base32 小写字母表 —— 也是 `flags` 的「数字」表（`a`=0、`b`=1…）。
const B32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// `fileMd5` 的定长：md5 的 16 字节 → base32 无填充 = 26 字符。
const MD5_B32_CHARS: usize = 26;
/// `kbb` 与 `kbc` 的 tail 都是 5 个字段。
const TAIL_FIELDS: usize = 5;

/// `kbb` tail 的字段下标：`total / size / chunkSize / name / fileMd5`。
const PROBE_TOTAL: usize = 0;
const PROBE_SIZE: usize = 1;
const PROBE_CHUNK_SIZE: usize = 2;
const PROBE_NAME: usize = 3;
const PROBE_FILE_MD5: usize = 4;

/// `kbc` tail 的字段下标：`seq / total / size / chunkSize / payload`。
const CHUNK_SEQ: usize = 0;
const CHUNK_TOTAL: usize = 1;
const CHUNK_SIZE: usize = 2;
const CHUNK_CHUNK_SIZE: usize = 3;
const CHUNK_PAYLOAD: usize = 4;

// ── 独立参照实现 ────────────────────────────────────────────────────────────

/// base32 小写无填充的盘表：`[a-z2-7]`（不含 `0`/`1`）。
fn is_b32_lower(ch: char) -> bool {
    ch.is_ascii_lowercase() || ('2'..='7').contains(&ch)
}

/// 字段**非空**且只含 base32 小写字符。
fn b32_field_ok(field: &str) -> bool {
    !field.is_empty() && field.chars().all(is_b32_lower)
}

/// 字段只含 base32 小写字符，**允许为空** —— 只有 `kbc` 的 `payload` 该这么放宽。
fn b32_field_ok_or_empty(field: &str) -> bool {
    field.chars().all(is_b32_lower)
}

/// 帧字段的 base32 小写无填充解码。空串合法（空文件的空载荷）。
fn b32_lower_decode(text: &str) -> Vec<u8> {
    assert!(
        text.chars().all(is_b32_lower),
        "待解码字段应当是 base32 小写，实际 {text:?}"
    );
    BASE32_NOPAD
        .decode(text.to_ascii_uppercase().as_bytes())
        .unwrap_or_else(|e| panic!("字段 {text:?} 不是合法 base32：{e}"))
}

/// 帧字段的 base32 小写无填充编码（做脚本的 `$.base32_lower_nopad` 做的同一件事）。
fn b32_lower_encode(bytes: &[u8]) -> String {
    BASE32_NOPAD.encode(bytes).to_ascii_lowercase()
}

/// CRC32 的 4 字节**大端**（与脚本的 `$.crc32`、生产实现同字节序）。
fn crc32_be(data: &[u8]) -> [u8; 4] {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize().to_be_bytes()
}

/// K 通道 `crc` 字段的取值：大端 CRC32 → base32 小写无填充（恒为 7 字符）。
fn crc32_b32_lower(data: &[u8]) -> String {
    b32_lower_encode(&crc32_be(data))
}

/// CRC 的被覆盖内容：`magic 0 flags 0 tail`。
///
/// 即**除引导符与 `crc` 字段本身之外的全部内容**。它不是帧里的连续子串
/// （`0<crc>0` 夹在中间），而是一个明确定义的拼接。把 `flags` 纳入覆盖是刻意的：
/// 它决定载荷要不要解压，漏在校验之外的话，一个字符被改坏就会静默走进错误的解码分支。
fn crc_body(magic: &str, flags: char, tail: &str) -> String {
    format!("{magic}{K_SEP}{flags}{K_SEP}{tail}")
}

/// [`crc_body`] 的摘要（K 通道的 `crc` 字段）。
fn crc_of_body(magic: &str, flags: char, tail: &str) -> String {
    crc32_b32_lower(crc_body(magic, flags, tail).as_bytes())
}

/// 组一帧（与 seed 的 `kFrame` / 生产实现同一规则；测试里独立实现，用于构造与篡改样本）。
fn build_k_frame(magic: &str, flags: u8, tail: &str) -> String {
    let flags_char = B32_ALPHABET[flags as usize] as char;
    let crc = crc_of_body(magic, flags_char, tail);
    format!("{K_SEP}{magic}{K_SEP}{flags_char}{K_SEP}{crc}{K_SEP}{tail}{K_END}")
}

/// 十六进制 → 字节（把系统 md5 工具输出的十六进制还原成 16 字节，再 base32 比对）。
fn hex_decode(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("md5 应当是合法十六进制"))
        .collect()
}

/// `num(n)` 的逆：base32 解出十进制串 → 数值；解不出返回 `None`（解析器用它做拒绝路径）。
fn try_decode_num(field: &str) -> Option<u64> {
    let bytes = BASE32_NOPAD
        .decode(field.to_ascii_uppercase().as_bytes())
        .ok()?;
    std::str::from_utf8(&bytes).ok()?.parse().ok()
}

/// 断言式的 [`try_decode_num`]：测试断言里直接用。
fn decode_num(field: &str) -> u64 {
    try_decode_num(field).unwrap_or_else(|| panic!("数值字段 {field:?} 不是 base32 十进制数"))
}

/// 一帧 K：`0 <magic> 0 <flags> 0 <crc> 0 <tail…> 1`。
#[derive(Debug)]
struct Frame {
    magic: String,
    /// `flags` 的 zstd 位。
    zstd: bool,
    /// `tail` 按 `0` 切出来的字段。
    fields: Vec<String>,
}

/// 解析一帧（从引导符 `0` 到终结符 `1` 的完整字面量）。
///
/// 任何不合法都返回 `Err`：字符集越界、字段数不对、`crc` 与 `magic 0 flags 0 tail` 不符、
/// `flags` 置了保留位……严格是刻意的 —— 键盘通道会丢键 / 重复键，把坏帧当好帧只会更难定位。
fn parse_frame(raw: &str) -> Result<Frame, String> {
    if !raw.starts_with(K_SEP) {
        return Err(format!("帧应当以引导符 0 开头：{raw:?}"));
    }
    if !raw.ends_with(K_END) {
        return Err(format!("帧应当以终结符 1 收尾：{raw:?}"));
    }
    let body = &raw[1..raw.len() - 1];
    // 字段里不可能出现 `1`：出现了就说明这一帧被提前截断或混入了别的字节
    if body.contains(K_END) {
        return Err(format!("帧内出现了多余的终结符 1：{raw:?}"));
    }

    let parts: Vec<&str> = body.split(K_SEP).collect();
    // magic / flags / crc / 至少一个 tail 字段
    if parts.len() < 4 {
        return Err(format!(
            "帧至少要有 magic/flags/crc/一个 tail 字段：{raw:?}"
        ));
    }
    let (magic, flags_text, crc) = (parts[0], parts[1], parts[2]);
    let tail = parts[3..].join(&K_SEP.to_string());

    // 字符集不变量（协议正确性的根基）：除定界符外每个字段都落在 [a-z2-7]
    if !b32_field_ok(magic) {
        return Err(format!("magic 不是 base32 小写字段：{magic:?}"));
    }
    if flags_text.chars().count() != 1 || !b32_field_ok(flags_text) {
        return Err(format!("flags 应当是单个 base32 数字：{flags_text:?}"));
    }
    if crc.len() != 7 || !b32_field_ok(crc) {
        return Err(format!("crc 应当是 7 字符 base32 小写：{crc:?}"));
    }
    let flags_char = flags_text.chars().next().expect("上面已确认恰好一个字符");

    // 每帧的 CRC 都必须等于 `magic 0 flags 0 tail` 的 CRC —— flags 也在覆盖范围内
    if crc_of_body(magic, flags_char, &tail) != crc {
        return Err(format!(
            "crc 与 `magic 0 flags 0 tail` 不符（flags 也在覆盖范围内）：{raw:?}"
        ));
    }

    // flags 只有 bit0（zstd）有定义，其余位保留：置位 = 对端在讲未来版本的信令。
    // magic 就是消息类型、没有版本字段，所以只能整帧拒绝，不能猜着解。
    let flags = B32_ALPHABET
        .iter()
        .position(|&b| b == flags_char as u8)
        .ok_or_else(|| format!("flags {flags_char:?} 不在 base32 字母表里"))? as u8;
    if flags > FLAG_ZSTD {
        return Err(format!(
            "flags {flags_char:?}（={flags}）置了保留位，本版本只认 0/1"
        ));
    }

    let fields: Vec<String> = parts[3..].iter().map(|s| (*s).to_string()).collect();
    match magic {
        MAGIC_PROBE => {
            if fields.len() != TAIL_FIELDS {
                return Err(format!(
                    "kbb tail 应当是 {TAIL_FIELDS} 个字段，实际 {fields:?}"
                ));
            }
            for (i, field) in fields.iter().enumerate() {
                if !b32_field_ok(field) {
                    return Err(format!(
                        "kbb tail 第 {i} 个字段不是非空 base32 小写：{field:?}"
                    ));
                }
            }
            if fields[PROBE_FILE_MD5].len() != MD5_B32_CHARS {
                return Err(format!(
                    "fileMd5 应当是 base32(md5 的 16 字节) = {MD5_B32_CHARS} 字符，实际 {:?}",
                    fields[PROBE_FILE_MD5]
                ));
            }
        }
        MAGIC_CHUNK => {
            if fields.len() != TAIL_FIELDS {
                return Err(format!(
                    "kbc tail 应当是 {TAIL_FIELDS} 个字段，实际 {fields:?}"
                ));
            }
            for (i, field) in fields.iter().enumerate() {
                // 载荷是唯一允许为空的字段：0 字节文件切出的空载荷，
                // 它是最后一个字段、后面紧跟终结符 `1`，所以没有歧义
                let ok = if i == CHUNK_PAYLOAD {
                    b32_field_ok_or_empty(field)
                } else {
                    b32_field_ok(field)
                };
                if !ok {
                    return Err(format!("kbc tail 第 {i} 个字段字符集越界：{field:?}"));
                }
            }
            // seq / total / chunkSize 必须自洽（说不通的片号直接丢掉）
            let seq = try_decode_num(&fields[CHUNK_SEQ])
                .ok_or_else(|| format!("kbc 的 seq 不是十进制数：{:?}", fields[CHUNK_SEQ]))?;
            let total = try_decode_num(&fields[CHUNK_TOTAL])
                .ok_or_else(|| format!("kbc 的 total 不是十进制数：{:?}", fields[CHUNK_TOTAL]))?;
            let chunk_size = try_decode_num(&fields[CHUNK_CHUNK_SIZE]).ok_or_else(|| {
                format!(
                    "kbc 的 chunkSize 不是十进制数：{:?}",
                    fields[CHUNK_CHUNK_SIZE]
                )
            })?;
            if total < 1 || chunk_size < 1 || seq >= total {
                return Err(format!(
                    "kbc 的 seq/total/chunkSize 说不通（seq={seq} total={total} chunkSize={chunk_size}）"
                ));
            }
        }
        other => return Err(format!("未知 magic {other:?}（v2 只有 kba/kbb/kbc）")),
    }

    Ok(Frame {
        magic: magic.to_string(),
        zstd: flags & FLAG_ZSTD != 0,
        fields,
    })
}

/// 字符集不变量（协议正确性的根基）的整体版：整条 transcript 里除定界符 `0`/`1` 外，
/// 只能出现 base32 小写字母表的字符。逐字段的断言是它的强化版（还知道字段边界）。
fn assert_wire_charset(transcript: &str) {
    for ch in transcript.chars() {
        assert!(
            ch == K_SEP || ch == K_END || is_b32_lower(ch),
            "transcript 里出现越界字符 {ch:?}：这条流必须只含 [a-z2-7] 加上定界符 0/1"
        );
    }
}

/// 把整段 transcript 切成帧（切法见模块头的「K 通道切帧规则」）。
///
/// 每帧到它的终结符 `1` 为止 —— 字段里不会出现 `1`，所以这是无歧义的。
fn split_frames(transcript: &str) -> Vec<Frame> {
    assert_wire_charset(transcript);
    let mut frames = Vec::new();
    let mut rest = transcript;

    while !rest.is_empty() {
        let head: String = rest.chars().take(40).collect();
        let end = rest
            .find(K_END)
            .unwrap_or_else(|| panic!("帧（从 {head:?} 起）没有终结符 1"));
        let (raw, next) = rest.split_at(end + 1);
        frames.push(parse_frame(raw).unwrap_or_else(|why| panic!("{why}")));
        rest = next;
    }

    frames
}

fn probe_frames(frames: &[Frame]) -> Vec<&Frame> {
    frames.iter().filter(|f| f.magic == MAGIC_PROBE).collect()
}

fn chunk_frames(frames: &[Frame]) -> Vec<&Frame> {
    frames.iter().filter(|f| f.magic == MAGIC_CHUNK).collect()
}

fn probe_of(frames: &[Frame]) -> &Frame {
    let probes = probe_frames(frames);
    assert_eq!(
        probes.len(),
        1,
        "transcript 里应当恰好有一个 kbb 探测帧，实际 {}",
        probes.len()
    );
    probes[0]
}

/// 所有分片帧都必须与探测帧在 `total` / `size` / `chunkSize` / zstd 位上完全一致。
///
/// 这是 v2「每个分片冗余携带会话头」的价值所在：接收端不必相信任何跨帧状态，
/// 只靠单帧 + 探测帧就能判定它属不属于这次传输。
fn assert_frames_agree(frames: &[Frame]) {
    let probe = probe_of(frames);
    let total = decode_num(&probe.fields[PROBE_TOTAL]);
    let size = decode_num(&probe.fields[PROBE_SIZE]);
    let chunk_size = decode_num(&probe.fields[PROBE_CHUNK_SIZE]);
    for frame in chunk_frames(frames) {
        let seq = decode_num(&frame.fields[CHUNK_SEQ]);
        assert_eq!(
            decode_num(&frame.fields[CHUNK_TOTAL]),
            total,
            "第 {seq} 片的 total 与探测帧不符"
        );
        assert_eq!(
            decode_num(&frame.fields[CHUNK_SIZE]),
            size,
            "第 {seq} 片的 size 与探测帧不符"
        );
        assert_eq!(
            decode_num(&frame.fields[CHUNK_CHUNK_SIZE]),
            chunk_size,
            "第 {seq} 片的 chunkSize 与探测帧不符"
        );
        assert_eq!(
            frame.zstd, probe.zstd,
            "第 {seq} 片的 flags zstd 位与探测帧不符"
        );
    }
}

/// 按帧重建原始文件字节（做接收端做的事）：逐片 base32 解码 → 按序拼接 → **整体**解压。
fn reassemble(frames: &[Frame]) -> Vec<u8> {
    assert_frames_agree(frames);

    let probe = probe_of(frames);
    let total = decode_num(&probe.fields[PROBE_TOTAL]) as usize;
    let size = decode_num(&probe.fields[PROBE_SIZE]) as usize;
    let chunk_size = decode_num(&probe.fields[PROBE_CHUNK_SIZE]) as usize;
    assert!(total >= 1, "total 至少是 1");
    assert!(chunk_size >= 1, "chunkSize 至少是 1");

    let mut chunks: Vec<Option<Vec<u8>>> = vec![None; total];
    for frame in chunk_frames(frames) {
        let seq = decode_num(&frame.fields[CHUNK_SEQ]) as usize;
        assert!(seq < total, "seq {seq} 超出 total {total}");

        // 该片在**线上**的字节：base32 解码即可。
        // zstd 位为真时它是「整条 zstd 流的一段」，**不能**在这里解压 ——
        // 发送端压的是整个文件，分片只是同一条流的切片。
        let slice = b32_lower_decode(&frame.fields[CHUNK_PAYLOAD]);

        assert!(chunks[seq].is_none(), "seq {seq} 重复出现");
        chunks[seq] = Some(slice);
    }

    // 顺序拼接所有分片 —— 得到的是「压缩整条流」（未压缩时就是原文件）
    let mut stream = Vec::new();
    for (seq, chunk) in chunks.iter().enumerate() {
        stream.extend_from_slice(chunk.as_ref().unwrap_or_else(|| panic!("缺第 {seq} 片")));
    }

    // 整文件压缩：收齐后**整体**解压一次（逐片解压是解不出来的）
    let mut joined = if probe.zstd {
        zstandard::decode_all(stream.as_slice()).expect("整条 zstd 流解压失败")
    } else {
        stream
    };
    // 接收页按探测帧里的原始大小核对
    joined.truncate(size);
    assert_eq!(joined.len(), size, "重建出的字节数应当等于探测帧里的 size");
    joined
}

/// 用系统 md5 工具独立算出文件摘要的十六进制（不复用脚本的 `$.md5`，避免两边一起错）。
fn md5_of_file(path: &Path) -> String {
    let output = std::process::Command::new("md5")
        .arg("-q")
        .arg(path)
        .output()
        .or_else(|_| std::process::Command::new("md5sum").arg(path).output())
        .expect("应当有 md5 或 md5sum 可用");
    assert!(output.status.success(), "md5 命令失败");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("md5 输出应当有摘要")
        .to_string()
}

/// 探测帧里 `fileMd5` 的期望值：base32 小写无填充(原始 16 字节 md5)。
fn file_md5_b32(path: &Path) -> String {
    b32_lower_encode(&hex_decode(&md5_of_file(path)))
}

// ── 协议完整性回归（针对解析器本身，不跑 seed）───────────────────────────────

/// 金标向量：整帧字面量是**用独立实现**（Python 的 `base64.b32encode` + `zlib.crc32`）
/// 算出来再抄进来的，不是跑本文件的组帧代码打印出来的 —— 否则实现写错时金标会跟着一起错。
/// 改动它等于改协议，必须是有意为之。
#[test]
fn golden_frames_match_independent_vectors() {
    // kbc：seq=0 total=1 size=10 chunkSize=4 payload=base32lower([0,1,2,3])="aaaqeay"
    // CRC 覆盖 `kbc 0 a 0 <tail>` = "kbc0a0ga0ge0geya0gq0aaaqeay" → "rdi6yqi"
    // （旧规则只覆盖 tail，得到的是 "4disjpi" —— 两个值不同本身就是这次修正的证据）
    assert_eq!(
        build_k_frame(MAGIC_CHUNK, 0, "ga0ge0geya0gq0aaaqeay"),
        "0kbc0a0rdi6yqi0ga0ge0geya0gq0aaaqeay1"
    );
    let parsed = parse_frame("0kbc0a0rdi6yqi0ga0ge0geya0gq0aaaqeay1")
        .expect("独立算出的金标分片帧必须能被解析");
    assert_eq!(parsed.magic, MAGIC_CHUNK);
    assert!(!parsed.zstd, "flags=a 不该置 zstd 位");
    assert_eq!(parsed.fields[CHUNK_SEQ], "ga");
    assert_eq!(parsed.fields[CHUNK_PAYLOAD], "aaaqeay");

    // 同一个 tail 只把 flags 换成 b（zstd），CRC 随之改变 —— flags 确实在覆盖范围内
    assert_eq!(
        build_k_frame(MAGIC_CHUNK, FLAG_ZSTD, "ga0ge0geya0gq0aaaqeay"),
        "0kbc0b07y2nk7a0ga0ge0geya0gq0aaaqeay1"
    );
    assert!(
        parse_frame("0kbc0b07y2nk7a0ga0ge0geya0gq0aaaqeay1")
            .unwrap()
            .zstd
    );

    // kbb：total=1 size=4 chunkSize=2 name="a.bin"（base32=mexge2lo）fileMd5=26×"a"
    assert_eq!(
        build_k_frame(
            MAGIC_PROBE,
            0,
            "ge0gq0gi0mexge2lo0aaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
        "0kbb0a0ifyihly0ge0gq0gi0mexge2lo0aaaaaaaaaaaaaaaaaaaaaaaaaa1"
    );
    assert!(parse_frame("0kbb0a0ifyihly0ge0gq0gi0mexge2lo0aaaaaaaaaaaaaaaaaaaaaaaaaa1").is_ok());
}

/// `flags` 必须在 CRC 的覆盖范围内，且保留位必须为 0。
///
/// 这是「flags 漏在校验之外」那次修正的回归测试：`flags` 的 bit0 决定载荷要不要解压，
/// 一个字符被改坏却验不出来，接收端就会静默走进错误的解码分支。
#[test]
fn rejects_flags_corruption_and_reserved_bits() {
    let tail = "ga0ge0geya0gq0aaaqeay";
    let frame = build_k_frame(MAGIC_CHUNK, 0, tail);
    assert!(parse_frame(&frame).is_ok(), "基准帧本身要能解析：{frame}");

    // 只把 flags 从 a(0) 改成 b(1)，**不**重算 crc → 必须被 crc 当场抓住
    let mut chars: Vec<char> = frame.chars().collect();
    assert_eq!(chars[5], 'a', "flags 应当在下标 5（0 k b c 0 <flags>）");
    chars[5] = 'b';
    let flipped: String = chars.into_iter().collect();
    let err = parse_frame(&flipped).expect_err("只改 flags 却不重算 crc 的帧必须被拒");
    assert!(
        err.contains("crc"),
        "拒绝原因应当是 crc 不符（说明 flags 在覆盖范围内）：{err}"
    );

    // flags 置保留位（c=2）：即使 crc 被正确地重算，也必须整帧拒绝 ——
    // magic 就是类型、没有版本字段，所以「将来才定义的位」只能拒绝，不能猜着解
    let reserved = build_k_frame(MAGIC_CHUNK, 2, tail);
    let err = parse_frame(&reserved).expect_err("flags 置保留位的帧必须被拒");
    assert!(err.contains("保留位"), "拒绝原因应当是保留位：{err}");

    // 同为「未来版本信令」，flags=31（字符 7）同样拒绝
    assert!(parse_frame(&build_k_frame(MAGIC_CHUNK, 31, tail)).is_err());
}

// ── 测试替身 ────────────────────────────────────────────────────────────────

/// 模拟的远程页面：它在三个阶段显示不同的反馈。
///
/// 阶段由**敲入的帧**推进（`type_str` 看到 `kbb` 进第 1 段、看到 `kbc` 进第 2 段），
/// 而不是由测试按 `scan_qr` 的调用次数硬编码。这样 seed 在一段里轮询多少次、
/// 补发多少轮，拿到的都是「这个阶段应有的」反馈 —— v2 的 seed 会在每个发送窗口后
/// 反复读反馈，一次性队列 / 粘性答案那种写法应付不了这个节奏。
///
/// 每个阶段是一**组轮播的二维码帧**（同一条消息拆成多帧）：每次 `scan_qr` 返回下一帧。
/// 三个阶段都为空 = 读不到屏幕（命令行 / headless / 没授权录屏）。
#[derive(Default)]
struct Page {
    /// 还没敲探测帧：seed 拿它判断「屏幕到底能不能读」。
    ///
    /// seed 的这次判断**只读一张二维码**，而一条反馈又可能拆成多帧轮播，所以单扫只会
    /// 拿到第 0 帧。只要这一帧形状合法，`sawFrame` 就为真 —— 不能要求「已经攒齐整条
    /// 消息」，否则第一条反馈一多帧就会被误判成「读不到屏幕」而退回盲发
    /// （见 `transfer_reads_multiframe_first_feedback_without_going_blind`）。
    before_probe: Vec<String>,
    /// 敲了探测帧、还没发分片：`ready` / `partial` / `complete` / `error` 在这里表态。
    after_probe: Vec<String>,
    /// 已经出现分片：通常轮到 `complete`（或 `partial` 继续补缺）。
    after_chunks: Vec<String>,
}

/// 记录型宿主：记录敲入的文本，`scan_qr` 按页面当前阶段轮播反馈。
#[derive(Default)]
struct Host {
    typed: Mutex<String>,
    page: Mutex<Page>,
    /// 页面当前阶段（0 探测前 / 1 探测后 / 2 已发片）与阶段内的轮播游标。
    playhead: Mutex<(usize, usize)>,
    qr_calls: AtomicU64,
}

impl Host {
    fn set_page(&self, page: Page) {
        *self.page.lock().unwrap() = page;
    }

    fn typed(&self) -> String {
        self.typed.lock().unwrap().clone()
    }

    fn qr_calls(&self) -> u64 {
        self.qr_calls.load(Ordering::SeqCst)
    }
}

impl ScriptHost for Host {
    fn type_str(&self, text: &str, _delay_ms: u64) -> Result<(), HostError> {
        self.typed.lock().unwrap().push_str(text);
        // 每次 type_str 拿到的是一整帧：敲的是哪种帧，页面就进入哪个阶段
        let phase = if text.starts_with("0kbc0") {
            2
        } else if text.starts_with("0kbb0") {
            1
        } else {
            return Ok(());
        };
        let mut head = self.playhead.lock().unwrap();
        if head.0 != phase {
            // 换消息了：新的一套二维码从第 0 帧开始播
            *head = (phase, 0);
        }
        Ok(())
    }

    fn confirm(&self, _message: &str) -> Result<ConfirmChoice, HostError> {
        Ok(ConfirmChoice::Yes)
    }

    fn pick_path(&self, _prompt: &str, _kind: PickKind) -> Result<Option<PathBuf>, HostError> {
        Ok(None)
    }

    fn scan_qr(&self) -> Result<Option<String>, HostError> {
        self.qr_calls.fetch_add(1, Ordering::SeqCst);
        let mut head = self.playhead.lock().unwrap();
        let page = self.page.lock().unwrap();
        let frames = match head.0 {
            0 => &page.before_probe,
            1 => &page.after_probe,
            _ => &page.after_chunks,
        };
        if frames.is_empty() {
            return Ok(None); // 屏幕上没有可用二维码
        }
        let frame = frames[head.1 % frames.len()].clone();
        head.1 += 1;
        Ok(Some(frame))
    }
}

#[derive(Default)]
struct SilentConsole;

impl ConsoleHook for SilentConsole {
    fn write(&self, _level: &str, _text: &str) {}
}

// ── 脚手架 ──────────────────────────────────────────────────────────────────

static SEQ: AtomicU64 = AtomicU64::new(0);
/// seed 里的 `srcPath` 占位常量（与 `script_runner` 同一手法替换）。
const PLACEHOLDER: &str = "const srcPath: string | null = null";

fn temp_file(name: &str, data: &[u8]) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "clipbeam-transfer-e2e-{}-{seq}-{name}",
        std::process::id()
    ));
    std::fs::write(&path, data).expect("写入测试文件失败");
    path
}

/// 替换路径与分片大小，并转译（`.ts` 必须先转译才能喂给引擎）。
fn seed_with_path(path: &Path, chunk_bytes: usize) -> String {
    let template = include_str!("../seed/04-file-transfer.ts");
    assert_eq!(
        template.matches(PLACEHOLDER).count(),
        1,
        "seed 里应当恰好有一处 srcPath 占位常量"
    );
    let with_path = template
        .replace(
            PLACEHOLDER,
            &format!(
                "const srcPath: string | null = {}",
                serde_json::to_string(&path.to_string_lossy().into_owned()).unwrap()
            ),
        )
        // 分片调小，让测试数据也能切出多片（不必写几 MB 的临时文件）
        .replace(
            "const chunkBytes: number = 4096",
            &format!("const chunkBytes: number = {chunk_bytes}"),
        );
    clipbeam_scripting::ts::transpile(&with_path, Path::new("04-file-transfer.ts"))
        .expect("文件传输脚本应当能转译")
        .to_string()
}

async fn run_seed(host: Arc<Host>, source: &str) -> Result<(), String> {
    let console: Arc<dyn ConsoleHook> = Arc::new(SilentConsole);
    let runtime = clipbeam_scripting::create_runtime(host, console, None)
        .await
        .expect("创建运行时失败");
    runtime
        .run_named_script("04-file-transfer.ts", source)
        .await
        .map_err(|e| e.to_string())
}

/// 把 base32 载荷尽量均分成 `parts` 段（每段非空）—— 模拟页面把一条消息拆成多帧轮播。
fn split_evenly(text: &str, parts: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    assert!(
        parts >= 1 && parts <= chars.len(),
        "载荷长度 {} 切不成 {parts} 段",
        chars.len()
    );
    let base = chars.len() / parts;
    let extra = chars.len() % parts;
    let mut at = 0usize;
    (0..parts)
        .map(|i| {
            let take = base + usize::from(i < extra);
            let piece: String = chars[at..at + take].iter().collect();
            at += take;
            piece
        })
        .collect()
}

/// 把一条 Q 控制消息编成远程页面会轮播的 QBB 二维码帧（接收页要做的编码，测试里独立实现）。
///
/// `crc` 是**整条消息**载荷（base32 大写）的摘要，同批各帧一致 —— 与 K 通道
/// 「每帧各算各的」刻意相反，它既是批量身份、也是拼装完成后的整体校验。
fn qbb(message: &str, parts: usize) -> Vec<String> {
    let payload = BASE32_NOPAD.encode(message.as_bytes());
    let crc = BASE32_NOPAD.encode(&crc32_be(payload.as_bytes()));
    split_evenly(&payload, parts)
        .into_iter()
        .enumerate()
        .map(|(index, frag)| format!("QBB.{parts}.{index}.{crc}.{frag}"))
        .collect()
}

/// 组一条页面反馈（v2 控制消息）并编成 QBB 帧：`{"type":"feedback","status":…}`。
///
/// `extra` 放 `missing` / `saved` / `reason`，`parts` 是拆成几帧轮播。
fn feedback(status: &str, extra: &[(&str, &str)], parts: usize) -> Vec<String> {
    let mut message = serde_json::json!({ "type": "feedback", "status": status });
    let map = message.as_object_mut().expect("json! 造出来的一定是对象");
    for (key, value) in extra {
        map.insert(
            (*key).to_string(),
            serde_json::Value::String((*value).to_string()),
        );
    }
    qbb(&message.to_string(), parts)
}

/// 随机（不可压缩）字节流：保证走多片且 zstd 位为 0。
fn xorshift(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

// ── 用例 ────────────────────────────────────────────────────────────────────

/// 正常完成：探测帧（kbb）→ 页面 ready → 全部分片（kbc）→ 页面 complete；
/// 重建出的字节与源文件逐字节一致。
#[tokio::test]
async fn transfer_completes_and_reassembles() {
    let payload = xorshift(3000, 0x2545_F491_4F6C_DD1D);
    let path = temp_file("roundtrip.bin", &payload);

    let host = Arc::new(Host::default());
    // 页面三阶段：探测前能读（ready）→ 探测后 ready（从头发）→ 收片后 complete。
    // complete 故意拆成两帧轮播：顺带覆盖 Q 通道「一条消息分多帧、按 total/crc 攒齐」
    // 的路径（seed 会先拿到第 0 帧、判定不成消息，再拿到第 1 帧才拼齐）。
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("ready", &[], 1),
        after_chunks: feedback("complete", &[("saved", "roundtrip.bin")], 2),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("文件传输脚本应当跑通");

    let frames = split_frames(&host.typed());
    let probe = probe_of(&frames);

    assert_eq!(
        decode_num(&probe.fields[PROBE_TOTAL]) as usize,
        3,
        "3000/1024 应当切出 3 片"
    );
    assert_eq!(
        decode_num(&probe.fields[PROBE_SIZE]) as usize,
        payload.len(),
        "size 应当是原始字节数"
    );
    assert_eq!(
        decode_num(&probe.fields[PROBE_CHUNK_SIZE]) as usize,
        1024,
        "探测帧应当携带配置的分片大小"
    );
    assert_eq!(
        String::from_utf8(b32_lower_decode(&probe.fields[PROBE_NAME])).unwrap(),
        path.file_name().unwrap().to_string_lossy(),
        "探测帧应当携带文件名"
    );
    assert_eq!(
        probe.fields[PROBE_FILE_MD5],
        file_md5_b32(&path),
        "探测帧里的整文件 MD5 应当与源文件一致"
    );

    // 关键断言：接收端视角重建出的字节与源文件一致
    assert_eq!(
        reassemble(&frames),
        payload,
        "重建出的字节应当与源文件逐字节一致"
    );
    assert_eq!(chunk_frames(&frames).len(), 3, "三片都该发出去");
    assert!(host.qr_calls() >= 3, "应当反复扫屏拿反馈");

    let _ = std::fs::remove_file(&path);
}

/// 断点续传：页面报 `partial` 且 `missing:"2-3"`，脚本必须**只补**第 2、3 片 ——
/// 页面已有的第 0、1 片一片都不能重敲。
#[tokio::test]
async fn transfer_sends_only_the_reported_gap() {
    let payload = xorshift(4000, 0x9E37_79B9_7F4A_7C15);
    let path = temp_file("resume.bin", &payload);

    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        // 4000/1024 = 4 片，缺 2-3 意味着前两片页面已经收到过
        after_probe: feedback("partial", &[("missing", "2-3")], 1),
        after_chunks: feedback("complete", &[("saved", "resume.bin")], 1),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("文件传输脚本应当跑通");

    let frames = split_frames(&host.typed());
    let probe = probe_of(&frames);
    assert_eq!(
        decode_num(&probe.fields[PROBE_TOTAL]),
        4,
        "4000/1024 应当切出 4 片"
    );

    // 每个分片帧自己也要与探测帧自洽
    assert_frames_agree(&frames);

    let sent: Vec<u64> = chunk_frames(&frames)
        .iter()
        .map(|f| decode_num(&f.fields[CHUNK_SEQ]))
        .collect();
    assert_eq!(sent, vec![2, 3], "报 missing 2-3 之后只该补发第 2、3 片");
    assert!(
        !sent.contains(&0) && !sent.contains(&1),
        "页面已有的第 0、1 片不该被重发，实际发了 {sent:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// 回归：**第一条反馈就是多帧**时，不能把「读不到屏幕」误判成真。
///
/// seed 判断屏幕能不能读时只扫一张二维码，而一条反馈可能拆成多帧轮播 —— 单扫只会拿到
/// 第 0 帧。旧实现要求「已经攒齐一条完整消息」才算读到，于是这里会误判为盲发：整份文件
/// 一次性全敲出去，既不续传、也不看页面报的缺口。
///
/// 本用例让探测前、探测后的反馈**都**拆成两帧，并断言脚本仍然只补页面报的缺口
/// （`missing:"2-3"`）而不是盲发全部 4 片。对着旧实现，`sent` 会是 `[0,1,2,3]`、
/// `qr_calls` 只会是 1，两条断言都会失败。
#[tokio::test]
async fn transfer_reads_multiframe_first_feedback_without_going_blind() {
    let payload = xorshift(4000, 0x0F1E_2D3C_4B5A_6978);
    let path = temp_file("multiframe.bin", &payload);

    let host = Arc::new(Host::default());
    host.set_page(Page {
        // 探测前的「能不能读屏」只扫到第 0 帧：形状合法即算读到（sawFrame）
        before_probe: feedback("ready", &[], 2),
        // 探测后的表态同样分两帧：要等 waitFor 的下一轮轮询才攒齐
        after_probe: feedback("partial", &[("missing", "2-3")], 2),
        after_chunks: feedback("complete", &[("saved", "multiframe.bin")], 1),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("第一条反馈分两帧时也应当跑通");

    let frames = split_frames(&host.typed());
    assert_eq!(decode_num(&probe_of(&frames).fields[PROBE_TOTAL]), 4);
    assert_frames_agree(&frames);

    let sent: Vec<u64> = chunk_frames(&frames)
        .iter()
        .map(|f| decode_num(&f.fields[CHUNK_SEQ]))
        .collect();
    // 关键负向断言：走的是「反馈可用、只补缺口」这条路，而不是盲发全部 4 片
    assert_eq!(
        sent,
        vec![2, 3],
        "多帧反馈不能被误判成『读不到屏幕』——否则会盲发全部片（旧实现即如此）"
    );
    // 盲发路径只会在开头扫一次屏；能续传必然要反复扫屏攒帧
    assert!(
        host.qr_calls() >= 4,
        "反馈可用时应当反复扫屏攒多帧，实际只扫了 {} 次",
        host.qr_calls()
    );

    let _ = std::fs::remove_file(&path);
}

/// **v2 的关键能力**：页面可以报**不连续的**缺失区间。
///
/// 报 `missing:"1,5"` 时，脚本必须恰好只发第 1、5 片。旧协议只能表达
/// 「从某个前缀之后全缺」，根本描述不出「中间缺一片、末尾再来一片」这种情况 ——
/// 这条断言正是本次协议升级的重点。
#[tokio::test]
async fn transfer_sends_only_interior_gaps() {
    let payload = xorshift(6500, 0xDEAD_BEEF_DEAD_BEEF);
    let path = temp_file("interior.bin", &payload);

    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("partial", &[("missing", "1,5")], 1),
        after_chunks: feedback("complete", &[("saved", "interior.bin")], 1),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("文件传输脚本应当跑通");

    let frames = split_frames(&host.typed());
    let probe = probe_of(&frames);
    assert_eq!(
        decode_num(&probe.fields[PROBE_TOTAL]),
        7,
        "6500/1024 应当切出 7 片"
    );
    assert_frames_agree(&frames);

    let sent: Vec<u64> = chunk_frames(&frames)
        .iter()
        .map(|f| decode_num(&f.fields[CHUNK_SEQ]))
        .collect();
    assert_eq!(
        sent,
        vec![1, 5],
        "报 missing 1,5 时只该发第 1、5 片（不连续的缺口要被精确表达）"
    );
    // 缺口之外的每一片都不该出现在 transcript 里
    for seq in [0u64, 2, 3, 4, 6] {
        assert!(
            !sent.contains(&seq),
            "第 {seq} 片不在 missing 里，不该被发送；实际发了 {sent:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}

/// 页面在探测帧之后就报 `complete`（远端已有同一文件）：脚本必须**一片都不发**。
#[tokio::test]
async fn transfer_skips_every_chunk_when_page_already_has_file() {
    let payload = xorshift(4000, 0x0123_4567_89AB_CDEF);
    let path = temp_file("already-there.bin", &payload);

    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("complete", &[("saved", "already-there.bin")], 1),
        // 走不到第 2 段：探测后立刻 complete，脚本在校验前就干净返回
        after_chunks: Vec::new(),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("远端已有同一文件时应当干净结束");

    let frames = split_frames(&host.typed());
    assert_eq!(frames.len(), 1, "只该有探测帧，实际 {} 帧", frames.len());
    assert!(
        chunk_frames(&frames).is_empty(),
        "页面报 complete 时不该敲任何分片"
    );
    assert_eq!(decode_num(&probe_of(&frames).fields[PROBE_TOTAL]), 4);

    let _ = std::fs::remove_file(&path);
}

/// 降级：读不到反馈（命令行 / 没有屏幕访问）时一次盲发全部片，不报错。
#[tokio::test]
async fn transfer_falls_back_to_blind_send_without_feedback() {
    // 用不可压缩的随机数据：整文件压缩帮不上忙（zstd 位为 0），片数就是原始字节切出来的片数。
    // （若用高重复文本，整文件压缩后可能只剩 1 片，测不出「盲发全部片」。）
    let payload = xorshift(3000, 0xDEAD_BEEF_1234_5678);
    let path = temp_file("blind.bin", &payload);

    // 页面三个阶段都没有二维码 —— 正是 `$.scan_qr()` 恒为 None 的「没有屏幕访问」
    let host = Arc::new(Host::default());

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("读不到反馈时也应当跑通（降级为盲发）");

    let frames = split_frames(&host.typed());
    let probe = probe_of(&frames);
    let total = decode_num(&probe.fields[PROBE_TOTAL]) as usize;
    assert!(total > 1, "测试数据应当切出多片，实际 {total}");
    assert_eq!(
        chunk_frames(&frames).len(),
        total,
        "降级路径应当把全部 {total} 片都发出去"
    );
    // 不可压缩数据 → flags 的 zstd 位为 0（整文件压缩无收益，脚本会跳过压缩）
    assert!(!probe.zstd, "高熵数据不该启用压缩");
    assert_eq!(
        probe.fields[PROBE_FILE_MD5],
        file_md5_b32(&path),
        "整文件摘要仍要与源文件对得上"
    );
    // 读不到反馈就只该在开头探一次屏幕，之后不再扫
    assert_eq!(host.qr_calls(), 1, "降级路径只该探一次屏幕");
    // 即使读不到反馈，帧本身仍然自洽：重建后与源文件一致
    assert_eq!(reassemble(&frames), payload);

    let _ = std::fs::remove_file(&path);
}

/// 可压缩文件：整文件走 `flags` 的 zstd 位 —— 分片是**同一条 zstd 流**的切片，
/// 收齐后整体解压一次。这条用例专门覆盖「整体解压」路径（逐片解压在这里必失败）。
#[tokio::test]
async fn transfer_compresses_whole_file() {
    // 高重复内容：整文件压缩收益远超 5%，必然启用 zstd
    let payload = b"ClipBeam whole-file zstd compression. ".repeat(200);
    let path = temp_file("compressible.txt", &payload);

    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("ready", &[], 1),
        after_chunks: feedback("complete", &[("saved", "compressible.txt")], 1),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("可压缩文件应当跑通");

    let frames = split_frames(&host.typed());
    let probe = probe_of(&frames);
    assert!(probe.zstd, "高重复内容应当启用 flags 的 zstd 位");
    assert_eq!(
        probe.fields[PROBE_FILE_MD5],
        file_md5_b32(&path),
        "整文件摘要应当对得上"
    );
    // `size` 是**原始**大小（不是压缩后大小）—— 接收端拿它核对解压结果
    assert_eq!(
        decode_num(&probe.fields[PROBE_SIZE]) as usize,
        payload.len()
    );

    // 关键：把「同一条 zstd 流的若干切片」拼起来整体解压，必须还原出原文件
    // （reassemble 里还会逐片核对 total/size/chunkSize/zstd 位）
    assert_eq!(
        reassemble(&frames),
        payload,
        "整体解压后应当与源文件逐字节一致"
    );

    let _ = std::fs::remove_file(&path);
}

/// 0 字节文件：补一片**空载荷**，接收端据此落一个空文件（而不是永远等不到第 0 片）。
///
/// 空载荷是协议里唯一允许为空的字段 —— 它是 `kbc` tail 的最后一个字段，后面紧跟
/// 终结符 `1`，所以没有歧义。独立解析器必须接受它，并且**不**因此放过别处的空字段。
#[tokio::test]
async fn transfer_handles_zero_byte_file() {
    let path = temp_file("empty.bin", b"");

    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("ready", &[], 1),
        after_chunks: feedback("complete", &[("saved", "empty.bin")], 1),
    });

    run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect("0 字节文件也应当跑通");

    let frames = split_frames(&host.typed());
    assert_eq!(
        frames.len(),
        2,
        "应当两帧（探测帧 + 唯一的分片帧），实际 {}",
        frames.len()
    );
    let probe = probe_of(&frames);
    assert_eq!(decode_num(&probe.fields[PROBE_SIZE]), 0, "size 应当是 0");
    assert_eq!(decode_num(&probe.fields[PROBE_TOTAL]), 1, "应当只有 1 片");
    assert!(!probe.zstd, "空文件压不小，不该启用压缩");

    let chunks = chunk_frames(&frames);
    assert_eq!(chunks.len(), 1, "0 字节文件应当恰好发 1 片");
    assert_eq!(
        chunks[0].fields[CHUNK_PAYLOAD], "",
        "这一片的载荷应当是空的"
    );
    assert_eq!(decode_num(&chunks[0].fields[CHUNK_SEQ]), 0);
    assert_eq!(
        reassemble(&frames),
        Vec::<u8>::new(),
        "重建结果应当是 0 字节"
    );

    let _ = std::fs::remove_file(&path);
}

/// 页面报 `error` 时脚本必须失败退出，而不是假装成功；而且一片都不该发。
#[tokio::test]
async fn transfer_aborts_on_reported_error() {
    let path = temp_file("error.bin", b"trigger error");
    let host = Arc::new(Host::default());
    host.set_page(Page {
        before_probe: feedback("ready", &[], 1),
        after_probe: feedback("error", &[("reason", "磁盘已满")], 1),
        after_chunks: Vec::new(),
    });

    let err = run_seed(host.clone(), &seed_with_path(&path, 1024))
        .await
        .expect_err("页面报 error 时脚本应当失败");
    assert!(
        err.contains("页面报错"),
        "错误信息应当说明是页面报的错：{err}"
    );
    assert!(
        err.contains("磁盘已满"),
        "错误信息应当带上页面的 reason：{err}"
    );

    let frames = split_frames(&host.typed());
    assert!(
        chunk_frames(&frames).is_empty(),
        "页面在探测后就报错，不该再敲分片"
    );

    let _ = std::fs::remove_file(&path);
}
