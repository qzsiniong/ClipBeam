//! 能力清单：把各扩展声明的签名汇总成可序列化数据，交给前端做补全与文档。
//!
//! # 为什么在 Rust 侧声明
//!
//! 「能力有哪些、签名长什么样」只有一个真源 —— 各扩展的
//! [`ScriptExtension::spec`](script_engine::ScriptExtension::spec)。
//! 前端不再手写一份列表，而是通过 Tauri 命令拿到这份 JSON 生成 CodeMirror 补全，
//! 因此**绑定改了、补全一定跟着改**。
//!
//! 引擎**没有任何能力**，所以这份清单就是 `$` 的全部。命名空间的名字（[`NAMESPACE`]）也
//! 从这里下发给前端 —— 前端不再硬编码 `ClipBeam` / `$`。
//!
//! [`NAMESPACE`]: crate::NAMESPACE

use serde::Serialize;

/// 一条能力的签名声明（前端消费的形态）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    /// JS 侧的方法名，例如 `md5`。
    pub name: String,
    /// 人类可读签名，例如 `md5(data: ArrayBuffer) -> string`。
    pub signature: String,
    /// 一句话说明。
    pub doc: String,
    /// 来源标记。目前只有 `clipbeam`（引擎不提供能力）；留着字段是为了将来若有第二个
    /// 来源时不必改前端协议。
    pub source: &'static str,
}

/// 一条能力清单 + 命名空间名字（`list_capabilities` 命令的返回体）。
///
/// 名字随清单一并下发，前端就不必再写死 `ClipBeam` / `$`。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CapabilityList {
    /// 能力命名空间的正式名（与 [`crate::NAMESPACE`] 一致）。
    pub namespace: String,
    /// 能力命名空间的别名；空串表示没有别名。
    pub alias: String,
    /// 全部能力（按注册顺序）。
    pub capabilities: Vec<Capability>,
}

/// 汇总本 crate 注入的全部能力（按注册顺序）。
pub fn capabilities() -> Vec<Capability> {
    let mut all: Vec<Capability> = Vec::new();

    for extension in crate::extensions::extensions() {
        for spec in extension.spec() {
            all.push(Capability {
                name: spec.name.to_string(),
                signature: spec.signature.to_string(),
                doc: spec.doc.to_string(),
                source: "clipbeam",
            });
        }
    }

    all
}

/// 前端消费的完整载荷：命名空间名字 + 全部能力。
pub fn capability_list() -> CapabilityList {
    CapabilityList {
        namespace: crate::NAMESPACE.to_string(),
        alias: crate::NAMESPACE_ALIAS.to_string(),
        capabilities: capabilities(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_cover_all_injected_ones() {
        let names: Vec<String> = capabilities().into_iter().map(|c| c.name).collect();

        for expected in [
            "bytes",
            "str",
            "chunks",
            "md5",
            "crc32",
            "base32",
            "base32_nopad",
            "base64_url_nopad",
            "hex_decode",
            "zstd",
            "gunzip",
            "unbrotli",
            "unlzma",
            "read_text",
            "write_text",
            "type_str",
            "confirm",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "能力清单缺少 {expected}：{names:?}"
            );
        }
    }

    #[test]
    fn capabilities_are_serializable_and_unique() {
        let caps = capabilities();
        let json = serde_json::to_string(&caps).expect("能力清单必须可序列化");
        assert!(json.contains("MD5"));

        let mut names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "能力名不应重复：{names:?}");
    }
}
