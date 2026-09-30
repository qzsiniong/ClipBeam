//! 类型声明的**内嵌副本**：把引擎的 `engine.d.ts` 作为字符串带进二进制。
//!
//! # 为什么需要它
//!
//! 插件的入口可以是 `.ts`，但用户是在**自己的插件目录**里写它 —— 那里不会有我们的仓库。
//! 所以应用得把「引擎提供哪些标准全局（`sleep` / `console` / `TextDecoder` / 定时器…）」
//! 的声明**写进用户的插件目录**，编辑器才认得出这些名字。
//! 声明的真源仍是同目录的 `engine.d.ts`（人读的那份），这里只是它的可编程入口。
//!
//! 走 `include_str!` 而不是运行时读文件：声明必须跟着二进制走
//! （与示例脚本、示例插件同一个理由 —— 否则「装完就少一份声明」）。
//!
//! 使用方：`clipbeam-scripting/src/declarations.rs`（脚本目录）与
//! `clipbeam-plugins/src/declarations.rs`（插件目录）。

/// 引擎标准全局的声明原文（`src/spec/engine.d.ts` 的逐字节副本）。
///
/// 使用方可以按需改写（例如去掉只对仓库目录结构成立的 `reference` 行），
/// 但**不该改动声明本体** —— 类型是对外契约。
pub const ENGINE_DTS: &str = include_str!("spec/engine.d.ts");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_real_declaration_file() {
        // 抽两处内容确认拿到的确实是那份文件，而不是空串/占位
        assert!(ENGINE_DTS.contains("declare function sleep("));
        assert!(ENGINE_DTS.contains("declare const console: ScriptConsole"));
        assert!(ENGINE_DTS.len() > 500, "声明内容不该这么短");
    }
}
