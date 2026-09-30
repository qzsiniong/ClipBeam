//! 插件 id 的规则与校验。
//!
//! id 同时是**目录名**与**清单里的身份字段**，两者必须一致（见 [`crate::catalog`]）：
//! 这样「界面上看到的插件」与「磁盘上的哪个目录」永远是对得上的，排查时不用猜。
//!
//! 规则刻意保守：只允许 ASCII 字母数字与 `-` `_` `.`，不得以 `.` 开头（隐藏目录/`.`/`..`），
//! 不得含路径分隔符，长度上限 [`MAX_ID_LEN`]。理由与脚本文件名的校验一致（见
//! `clipbeam-scripting::scripts::is_valid_script_name`）：
//! **不做「清洗后改名」的容错**，非法就给明确错误 —— 悄悄改名会让用户找不到自己的插件。

/// id 的长度上限（字符数）。
pub const MAX_ID_LEN: usize = 64;

/// id 是否合法。
pub fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.chars().count() > MAX_ID_LEN {
        return false;
    }
    if id.starts_with('.') {
        return false;
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

/// 校验 id，非法时返回中文说明（文案同时用于界面与日志，所以写清「哪里不合法」）。
pub fn check_id(id: &str) -> Result<(), String> {
    if is_valid_id(id) {
        return Ok(());
    }

    let reason = if id.is_empty() {
        "不能为空".to_string()
    } else if id.chars().count() > MAX_ID_LEN {
        format!("过长（上限 {MAX_ID_LEN} 个字符）")
    } else if id.starts_with('.') {
        "不能以 . 开头".to_string()
    } else if id.contains('/') || id.contains('\\') {
        "不能包含路径分隔符".to_string()
    } else if id.contains("..") {
        "不能包含 ..".to_string()
    } else {
        "只能使用 ASCII 字母、数字、-、_、.".to_string()
    };

    Err(format!("插件 id {id:?} 不合法：{reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reasonable_ids() {
        for ok in [
            "hello",
            "hello-plugin",
            "my_plugin",
            "com.example.plugin",
            "p1",
        ] {
            assert!(is_valid_id(ok), "{ok} 应当合法");
        }
    }

    #[test]
    fn rejects_paths_and_hidden_names() {
        for bad in [
            "", ".", "..", ".hidden", "a/b", "a\\b", "../evil", "a..b", "插件", "a b", "a:b",
        ] {
            assert!(!is_valid_id(bad), "{bad} 应当被拒绝");
        }
    }

    #[test]
    fn rejects_overlong_ids() {
        let long = "a".repeat(MAX_ID_LEN + 1);
        assert!(!is_valid_id(&long), "超长 id 应当被拒绝");
        assert!(is_valid_id(&"a".repeat(MAX_ID_LEN)), "正好到上限应当合法");
    }

    #[test]
    fn error_message_explains_the_reason() {
        assert!(check_id("").unwrap_err().contains("不能为空"));
        assert!(check_id("../x").unwrap_err().contains(".."));
        assert!(check_id("a/b").unwrap_err().contains("路径分隔符"));
        assert!(check_id("插件").unwrap_err().contains("ASCII"));
        assert!(check_id("hello-plugin").is_ok());
    }
}
