//! 协议 C（部署通道）：把单文件接收页打包成「自解压 HTML」引导包——
//! 全 ASCII、单行、无外部依赖——通过键盘逐字符打进远程记事本，
//! 另存为 `.html` 打开后，引导脚本解码并替换文档，得到完整接收页。

use std::time::Duration;

use arboard::Clipboard;

use crate::cancel::CancellationToken;
use crate::config::Config;
use crate::protocol::{b32_encode_lower};
use crate::typer::{TypeResult, Typer};

/// 编译期内嵌接收页（单文件）。
const PAGE: &str = include_str!("../../web/clipbeam.html");

/// 自解压模板。约束：
/// - 全 ASCII（enigo Unicode 逐字符输入，避免任何多字节字符）；
/// - 单行，不含 `\n`/`\r`（防止记事本里触发快捷键/换行问题）；
/// - 载荷仅 `[a-z2-7]`，放在双引号 JS 字符串中无需转义；
/// - 除结尾的 `</script>` 外不得出现该序列（载荷 base32 不可能包含）。
const TEMPLATE: &str = concat!(
    r#"<!doctype html><meta charset="utf-8"><title>ClipBeam</title><body style="font:14px sans-serif;padding:24px">ClipBeam bootstrap. If this page does not reload automatically, save this text as a .html file (UTF-8) and open it again.<script>var d=""#,
    "__PAYLOAD__",
    r#"";var a="abcdefghijklmnopqrstuvwxyz234567";var v=0,b=0,o=[];for(var i=0;i<d.length;i++){v=(v<<5)|a.indexOf(d.charAt(i));b+=5;if(b>=8){b-=8;o.push((v>>>b)&255);}}var t=new TextDecoder().decode(new Uint8Array(o));document.open();document.write(t);document.close();</script>"#
);

/// 生成自解压引导 HTML。
pub fn bootstrap_html() -> String {
    // 大写 base32：与模板内解码表一致，且全为 JS 字符串安全字符。
    let payload = b32_encode_lower(PAGE.as_bytes());
    TEMPLATE.replace("__PAYLOAD__", &payload)
}

/// 引导包大小信息：(待敲入字符数, 内嵌接收页字节数)。
pub fn sizes() -> (usize, usize) {
    (bootstrap_html().chars().count(), PAGE.len())
}

/// 逐字符把引导包打进当前焦点窗口（远程记事本）。
pub fn type_bootstrap(cfg: &Config, cancel: &CancellationToken, settle: bool) -> TypeResult {
    if settle && !Typer::wait_settle(cfg, cancel) {
        return TypeResult::Cancelled(0);
    }
    let html = bootstrap_html();
    let mut typer = match Typer::new(cfg, cancel.clone()) {
        Ok(t) => t,
        Err(e) => return TypeResult::Failed(0, e),
    };
    std::thread::sleep(Duration::from_secs(3));
    typer.type_str(&html)
}

/// 把引导包放到宿主机剪贴板（远程桌面自带剪贴板同步时可直接粘贴）。
pub fn copy_bootstrap() -> Result<(), String> {
    let html = bootstrap_html();
    let mut cb = Clipboard::new().map_err(|e| format!("无法访问本机剪贴板: {e}"))?;
    cb.set_text(html)
        .map_err(|e| format!("写本机剪贴板失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_single_line_ascii() {
        let s = bootstrap_html();
        assert!(s.is_ascii(), "引导包必须全 ASCII");
        assert!(!s.contains('\n') && !s.contains('\r'), "引导包必须单行");
        assert!(!s.contains("__PAYLOAD__"), "占位符必须被替换");
        // 只有结尾一个 </script>
        assert_eq!(s.matches("</script>").count(), 1);
    }

    #[test]
    fn bootstrap_payload_decodes_to_exact_page() {
        let s = bootstrap_html();
        // 取出 var d="..." 的载荷
        let start = s.find("var d=\"").unwrap() + "var d=\"".len();
        let rest = &s[start..];
        let end = rest.find('"').unwrap();
        let payload = &rest[..end];
        assert!(
            payload
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
            "载荷只能含大写字母与数字"
        );
        // 与模板中解码表等价的 Rust 解码应还原出原页面字节
        let decoded = crate::protocol::b32_decode(payload).unwrap();
        assert_eq!(decoded, PAGE.as_bytes());
    }
}
