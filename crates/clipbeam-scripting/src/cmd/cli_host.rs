//! 命令行宿主：把 `ScriptHost` 落到终端上。
//!
//! `cargo run -p clipbeam -- script foo.js` 走这条路径：输出逐字打到 stdout，
//! 提问、文件授权与路径选择都读 `stdin`，进度打到 `stderr`。这样不开 GUI 也能调试脚本。
//!
//! 非交互场景（stdin 不是终端，例如重定向/管道）：`confirm` 按「否」、
//! 文件授权按**拒绝**处理、`pick_path` 按「没选到」返回 `None` —— 没人看着的时候
//! 既不阻塞，也不让脚本改磁盘。
//!
//! 没有原生选择框可用，`pick_path` 就退化成「在终端里输入一行路径」：规则与脚本里的
//! 路径完全一致（复用 [`crate::extensions::file::resolve_path`]），并额外要求它真的
//! 存在、且类型对得上 —— 原生选择框不可能给你一个不存在的路径，终端输入可以。

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use script_engine::CancelSignal;

use crate::extensions::file::resolve_path;
use crate::host::{ConfirmChoice, FileDecision, HostError, PickKind, ScriptHost};

/// 终端宿主。
///
/// * `type_str`：逐字符写 stdout，字符间按 `delay_ms` 等待（模拟打字机效果），
///   每字符前检查取消信号；
/// * `confirm`：在 stdout 提问、读一行 stdin；输入 `y`/`Y` 为「是」，`q`/`Q` 为「中止」，
///   其余（含 EOF）为「否」；
/// * `pick_path`：同样读一行 stdin（回车 = 取消）；输入必须是绝对路径且存在；
/// * `allow_file_change`：同样读一行 stdin；`y` 允许一次、`a` 本次运行内放行该目录、
///   其余拒绝；
/// * `progress`：把 `已输出/总数` 打到 stderr 的同一行（`\r` 覆盖），节流到 100ms 一次。
pub struct CliScriptHost {
    cancel: CancelSignal,
    typed: AtomicUsize,
    last_progress: Mutex<std::time::Instant>,
    interactive: bool,
    chunked: bool,
}

impl CliScriptHost {
    /// 建一个终端宿主。
    pub fn new(cancel: CancelSignal) -> Self {
        Self::with_chunked(cancel, false)
    }

    /// `chunked == true` 时按整段写文本（不逐字符 flush），适合把输出重定向到文件/管道。
    pub fn with_chunked(cancel: CancelSignal, chunked: bool) -> Self {
        Self {
            cancel,
            typed: AtomicUsize::new(0),
            last_progress: Mutex::new(std::time::Instant::now()),
            interactive: std::io::stdin().is_terminal(),
            chunked,
        }
    }

    /// 到目前为止已输出的字符数。
    pub fn typed_chars(&self) -> usize {
        self.typed.load(Ordering::Relaxed)
    }

    /// 在终端上问一个问题并读一行；非交互时返回 `None`。
    ///
    /// `label` 是行首方括号里的那两个字（「确认」/「授权」/「选择」），
    /// 让用户在滚动的终端里一眼看出脚本在问哪一类问题。
    fn ask(&self, label: &str, prompt: &str) -> Result<Option<String>, HostError> {
        if !self.interactive {
            return Ok(None);
        }

        print!("\n[{label}] {prompt} ");
        std::io::stdout()
            .flush()
            .map_err(|err| HostError::Failed(format!("刷新终端失败：{err}")))?;

        let mut input = String::new();
        let read = std::io::stdin()
            .lock()
            .read_line(&mut input)
            .map_err(|err| HostError::Failed(format!("读取输入失败：{err}")))?;
        if read == 0 {
            // EOF：当作「没有回答」
            return Ok(None);
        }
        Ok(Some(input.trim().to_string()))
    }
}

impl ScriptHost for CliScriptHost {
    fn type_str(&self, text: &str, delay_ms: u64) -> Result<(), HostError> {
        if self.chunked {
            // 一次写完整段：不 flush 每个字符，也不按字符等待
            if self.cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            let mut stdout = std::io::stdout();
            write!(stdout, "{text}")
                .map_err(|err| HostError::Failed(format!("写终端失败：{err}")))?;
            stdout.flush().ok();
            self.typed
                .fetch_add(text.chars().count(), Ordering::Relaxed);
            return Ok(());
        }

        let delay = Duration::from_millis(delay_ms);
        let mut stdout = std::io::stdout();

        for ch in text.chars() {
            if self.cancel.is_cancelled() {
                let _ = stdout.flush();
                return Err(HostError::Cancelled);
            }
            write!(stdout, "{ch}")
                .map_err(|err| HostError::Failed(format!("写终端失败：{err}")))?;
            stdout
                .flush()
                .map_err(|err| HostError::Failed(format!("刷新终端失败：{err}")))?;

            self.typed.fetch_add(1, Ordering::Relaxed);
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }
        stdout.flush().ok();
        Ok(())
    }

    fn confirm(&self, message: &str) -> Result<ConfirmChoice, HostError> {
        // 非交互（管道/重定向）：按「否」处理，绝不阻塞
        let Some(answer) = self.ask("确认", &format!("{message} (y=是 / 其它=否 / q=中止):"))?
        else {
            return Ok(ConfirmChoice::No);
        };

        if answer.eq_ignore_ascii_case("q") {
            return Ok(ConfirmChoice::Abort);
        }
        Ok(if answer.eq_ignore_ascii_case("y") {
            ConfirmChoice::Yes
        } else {
            ConfirmChoice::No
        })
    }

    /// 终端里手输路径（回车 = 取消）。
    ///
    /// 校验两条：路径必须能按 [`resolve_path`] 解析成绝对路径（`~`、盘符都认），
    /// 而且真的存在、类型与 `kind` 相符。原生选择框不会有这两类错误，手输会有 ——
    /// 与其让脚本拿着一个打错的路径往下跑，不如当场说清楚。
    fn pick_path(&self, prompt: &str, kind: PickKind) -> Result<Option<PathBuf>, HostError> {
        let label = match kind {
            PickKind::File => "文件",
            PickKind::Dir => "文件夹",
        };
        let question = if prompt.trim().is_empty() {
            format!("请输入{label}的绝对路径（回车=取消）:")
        } else {
            format!("{prompt}\n       {label}路径（回车=取消）:")
        };

        // 非交互：当作「没选到」，绝不阻塞（与 confirm 按「否」同一个思路）
        let Some(answer) = self.ask("选择", &question)? else {
            return Ok(None);
        };
        if answer.is_empty() {
            return Ok(None);
        }

        let path = resolve_path(&answer).map_err(HostError::Failed)?;

        let matched = match kind {
            PickKind::File => path.is_file(),
            PickKind::Dir => path.is_dir(),
        };
        if !matched {
            return Err(HostError::Failed(format!(
                "不是{label}（或不存在）：{}",
                path.display()
            )));
        }

        Ok(Some(path))
    }

    fn allow_file_change(
        &self,
        action: &str,
        path: &Path,
        scope_dir: &Path,
    ) -> Result<FileDecision, HostError> {
        let prompt = format!(
            "脚本请求{action}：{}\n       目录：{}\n       (y=允许一次 / a=本次运行内该目录都允许 / 其它=拒绝):",
            path.display(),
            scope_dir.display()
        );

        // 非交互：拒绝。没人看着的时候让脚本改磁盘不合适。
        let Some(answer) = self.ask("授权", &prompt)? else {
            return Ok(FileDecision::Deny);
        };

        Ok(match answer.to_ascii_lowercase().as_str() {
            "y" | "yes" => FileDecision::Allow,
            "a" | "all" => FileDecision::AllowDir,
            _ => FileDecision::Deny,
        })
    }

    fn progress(&self, typed: usize, total: usize) {
        // 节流：100ms 一次，避免逐字符刷屏
        {
            let mut last = self.last_progress.lock().unwrap();
            if last.elapsed() < Duration::from_millis(100) && typed != total {
                return;
            }
            *last = std::time::Instant::now();
        }
        eprint!("\r已输出 {typed}/{total} 字符");
        if typed == total {
            eprintln!();
        }
        let _ = std::io::stderr().flush();
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
