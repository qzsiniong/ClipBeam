//! 用户配置：三组热键 + 延迟/超时/阈值，JSON 持久化于系统 config 目录。
//!
//! 热键字符串使用 global-hotkey 的解析格式（如 "Cmd+Shift+K" / "Ctrl+Shift+J" / "Esc"）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// 发送（宿主机→远程）热键。
    pub send_hotkey: String,
    /// 接收（远程→宿主机）热键。
    pub recv_hotkey: String,
    /// 任务中止热键（仅任务运行期间注册）。
    pub stop_hotkey: String,
    /// 逐键发送间隔（毫秒）。
    pub key_delay_ms: u64,
    /// 触发热键后、开始打字前的修饰键释放等待（毫秒）。
    pub settle_ms: u64,
    /// 二维码接收超时（秒）。
    pub receive_timeout_s: u64,
    /// 键盘发送文本大小上限（KB，1KB=1024 字节）。
    pub max_text_kb: usize,
}

#[cfg(target_os = "macos")]
fn default_send_hotkey() -> String {
    "Cmd+Shift+K".into()
}
#[cfg(target_os = "macos")]
fn default_recv_hotkey() -> String {
    "Cmd+Shift+J".into()
}
#[cfg(not(target_os = "macos"))]
fn default_send_hotkey() -> String {
    "Ctrl+Shift+K".into()
}
#[cfg(not(target_os = "macos"))]
fn default_recv_hotkey() -> String {
    "Ctrl+Shift+J".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            send_hotkey: default_send_hotkey(),
            recv_hotkey: default_recv_hotkey(),
            stop_hotkey: "Esc".into(),
            key_delay_ms: 3,
            settle_ms: 200,
            receive_timeout_s: 120,
            max_text_kb: 256,
        }
    }
}

impl Config {
    pub fn max_text_bytes(&self) -> usize {
        self.max_text_kb * 1024
    }

    /// 返回非法字段的中文错误描述；空 Vec 表示通过。
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.send_hotkey.trim().is_empty() {
            errs.push("发送热键不能为空".into());
        }
        if self.recv_hotkey.trim().is_empty() {
            errs.push("接收热键不能为空".into());
        }
        if self.stop_hotkey.trim().is_empty() {
            errs.push("中止热键不能为空".into());
        }
        if self.send_hotkey == self.recv_hotkey
            || self.send_hotkey == self.stop_hotkey
            || self.recv_hotkey == self.stop_hotkey
        {
            errs.push("三个热键不能重复".into());
        }
        if self.key_delay_ms > 100 {
            errs.push("键延迟需在 0~100ms 之间".into());
        }
        if self.settle_ms > 5000 {
            errs.push("settle 等待需在 0~5000ms 之间".into());
        }
        if !(5..=3600).contains(&self.receive_timeout_s) {
            errs.push("接收超时需在 5~3600 秒之间".into());
        }
        if !(1..=10_240).contains(&self.max_text_kb) {
            errs.push("文本上限需在 1~10240 KB 之间".into());
        }
        errs
    }

    pub fn key_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.key_delay_ms)
    }
    pub fn settle(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.settle_ms)
    }
    pub fn receive_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.receive_timeout_s)
    }

    /// 配置文件路径：config_dir/KeyBeam/config.json。
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("KeyBeam").join("config.json"))
    }

    /// 读取配置；文件缺失或损坏时静默回退默认值。
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 保存配置（自动创建目录）。
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "找不到系统配置目录"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).expect("config 序列化");
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(Config::default().validate().is_empty());
    }

    #[test]
    fn json_roundtrip_and_missing_fields() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        // 旧版本/缺字段 JSON 应回退默认
        let partial: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(partial, Config::default());
    }

    #[test]
    fn validation_catches_bad_values() {
        let mut cfg = Config::default();
        cfg.key_delay_ms = 999;
        cfg.receive_timeout_s = 1;
        cfg.max_text_kb = 0;
        cfg.stop_hotkey = cfg.send_hotkey.clone();
        let errs = cfg.validate();
        assert!(errs.len() >= 4, "应报出多项错误: {errs:?}");
    }
}
