//! 能力清单：把各扩展声明的签名汇总成可序列化数据，交给前端做补全与文档。
//!
//! # 为什么在 Rust 侧声明
//!
//! 「能力有哪些、签名长什么样」只有一个真源 —— 各扩展的
//! [`ScriptExtension::spec`](clipbeam_script::ScriptExtension::spec)。
//! 前端不再手写一份列表，而是通过 Tauri 命令拿到这份 JSON 生成 CodeMirror 补全，
//! 因此**绑定改了、补全一定跟着改**。
//!
//! 引擎 core 自带的能力（`$.file` / `$.sleep`）也一并汇总，前端看到的是完整的 `$`。

use clipbeam_script::{BasicExtension, ScriptExtension};
use serde::Serialize;

use crate::extensions;

/// 一条能力的签名声明（前端消费的形态）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    /// JS 侧的方法名，例如 `md5`。
    pub name: String,
    /// 人类可读签名，例如 `md5(data: ArrayBuffer) -> string`。
    pub signature: String,
    /// 一句话说明。
    pub doc: String,
    /// 来源：`core`（引擎自带）或 `clipbeam`（本 crate 扩展）。
    pub source: &'static str,
}

/// 汇总 core 内置能力与 ClipBeam 扩展能力。
///
/// core 先、扩展后，与运行期注册顺序一致。
pub fn capabilities() -> Vec<Capability> {
    let mut all: Vec<Capability> = BasicExtension
        .spec()
        .into_iter()
        .map(|spec| Capability {
            name: spec.name.to_string(),
            signature: spec.signature.to_string(),
            doc: spec.doc.to_string(),
            source: "core",
        })
        .collect();

    for extension in extensions() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_cover_core_and_extensions() {
        let names: Vec<String> = capabilities().into_iter().map(|c| c.name).collect();

        for expected in ["file", "sleep", "md5", "base32", "zstd", "typeStr", "confirm"] {
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
        assert!(json.contains("md5(data: ArrayBuffer) -> string"));

        let mut names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "能力名不应重复：{names:?}");
    }
}
