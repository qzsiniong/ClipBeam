//! 把脚本引擎接到 Tauri 上：GUI 侧的 `ScriptHost` 与 `ConsoleHook` 实现。
//!
//! 分工：
//!
//! * [`clipbeam_scripting::ScriptHost`] 是「业务宿主」接口（输出 / 询问 / 文件授权）；
//! * [`script_engine::ConsoleHook`] 是引擎的 `console.*` 落点 —— 本文件同时实现两者：
//!   `console.*` 进脚本窗口的 Console 面板，宿主交互走系统弹框与 [`Typer`]；
//! * `$.type_str` 走既有的 [`Typer`]（逐键打进当前焦点窗口），`$.confirm` 与文件授权
//!   用**系统原生**确认框（`tauri-plugin-dialog`），进度通过 `worker-progress` 事件推送；
//! * 「先点目标窗口再输出」由 [`crate::standby::StandbyGate`] 保证：**惰性**地在首次
//!   `type_str` 之前弹待命窗口，纯计算脚本不会被打扰。
//!
//! 命令行路径（`clipbeam script`）不走这里，它用 `clipbeam_scripting::cmd::CliScriptHost`。

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use script_engine::ConsoleHook;
use clipbeam_scripting::{ConfirmChoice, FileDecision, HostError, ScriptHost};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use crate::cancel::CancellationToken as WorkerCancel;
use crate::console_panel::ConsoleBuffer;
use crate::standby::StandbyGate;
use crate::typer::Typer;
use crate::worker::{Progress, TaskKind, WorkerState};

/// 进度节流间隔（与 worker 的进度事件保持一致：1 秒）。
const PROGRESS_THROTTLE_MS: u64 = 1000;

/// 进度片段的最大长度（字符）。
const SNIPPET_LIMIT: usize = 60;

/// 待命超时后的收尾动作（可被多次触发，因此用 `Fn` + `Arc` 而不是 `FnOnce`）。
pub type CancelHook = Arc<dyn Fn() + Send + Sync>;

/// 脚本窗口 label（系统确认框的父窗口；隐藏时临时显形）。
const SCRIPTING_WINDOW: &str = "scripting";

/// 普通确认框标题。
const CONFIRM_TITLE: &str = "ClipBeam 脚本确认";

/// 文件授权框标题。
const FILE_TITLE: &str = "ClipBeam 脚本要修改文件";

/// 文件授权框的三个按钮文案（结果按文案回传，所以三处必须一致）。
const FILE_ALLOW: &str = "允许一次";
const FILE_ALLOW_DIR: &str = "本次运行内该目录都允许";
const FILE_DENY: &str = "拒绝";

/// GUI 侧宿主：把脚本的输出、询问、进度接到 Tauri 窗口上。
pub struct TauriScriptHost {
    app: AppHandle,
    /// `Typer` 惰性构造：脚本可能一次都不输出，没必要提前申请键盘权限。
    typer: Mutex<Option<Typer>>,
    /// 与任务共享的取消令牌（Esc / 托盘 / 按钮）。
    cancel: WorkerCancel,
    /// 与任务共享的进度计数（`(已敲入, 总数)`）。
    progress: Arc<std::sync::RwLock<(usize, usize)>>,
    /// 待命门闩：首次输出前确保用户已把焦点切到目标窗口。
    standby: Arc<StandbyGate>,
    /// 兜底待命超时后的收尾（提示用户 + 取消任务），由 worker 注入。
    on_standby_cancel: CancelHook,
    /// 脚本窗口 Console 面板的输出缓冲。
    console: Arc<ConsoleBuffer>,
    typed: AtomicUsize,
    last_progress_ms: AtomicU64,
    started_at_ms: u64,
}

impl TauriScriptHost {
    /// 为一个脚本任务创建宿主。
    pub fn new(
        app: AppHandle,
        cancel: WorkerCancel,
        standby: Arc<StandbyGate>,
        progress: Arc<std::sync::RwLock<(usize, usize)>>,
        on_standby_cancel: CancelHook,
        console: Arc<ConsoleBuffer>,
    ) -> Self {
        Self {
            app,
            typer: Mutex::new(None),
            cancel,
            progress,
            standby,
            on_standby_cancel,
            console,
            typed: AtomicUsize::new(0),
            last_progress_ms: AtomicU64::new(0),
            started_at_ms: now_ms(),
        }
    }

    /// 到目前为止已交给键盘的字符数。
    pub fn typed_chars(&self) -> usize {
        self.typed.load(Ordering::Relaxed)
    }

    /// 输出进度：更新共享计数 + 节流 emit `worker-progress`。
    fn emit_progress(&self, typed: usize, total: usize, snippet: &str) {
        if let Ok(mut slot) = self.progress.write() {
            *slot = (typed, total);
        }

        let now = now_ms();
        let last = self.last_progress_ms.load(Ordering::Relaxed);
        let is_edge = typed == 1 || typed == total;
        if !is_edge && now.saturating_sub(last) < PROGRESS_THROTTLE_MS {
            return;
        }
        self.last_progress_ms.store(now, Ordering::Relaxed);

        let _ = self.app.emit(
            "worker-progress",
            Progress {
                kind: TaskKind::Script,
                got: typed,
                total,
                started_at: self.started_at_ms,
                ts: now,
                snippet: snippet.to_string(),
            },
        );
    }

    /// 弹一个**系统原生**确认框并把结果原样返回。
    ///
    /// 系统弹框要应用处于激活状态才可靠地出现在最前：脚本窗口在托盘模式可能是隐藏的，
    /// 先临时显形 + 聚焦，回答完再恢复原状态（不改变用户设定的窗口可见性）。
    ///
    /// 调用方是 Worker 的阻塞线程，正是 rfd blocking API 的适用场景；**绝不能在主线程调用**。
    fn ask_dialog(
        &self,
        title: &str,
        message: &str,
        buttons: MessageDialogButtons,
    ) -> MessageDialogResult {
        let scripting_win = self.app.get_webview_window(SCRIPTING_WINDOW);
        let was_visible = scripting_win
            .as_ref()
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        if let Some(window) = &scripting_win {
            if !was_visible {
                let _ = window.show();
                let _ = window.set_focus();
                // 脚本窗口临时显形期间也要有 Dock 图标，回答完恢复隐藏时再撤掉
                crate::sync_dock_icon(&self.app);
            }
        }

        let mut dialog = self
            .app
            .dialog()
            .message(message)
            .title(title)
            .kind(MessageDialogKind::Warning)
            .buttons(buttons);
        if let Some(window) = &scripting_win {
            dialog = dialog.parent(window);
        }

        let result = dialog.blocking_show_with_result();

        if let Some(window) = &scripting_win {
            if !was_visible {
                let _ = window.hide();
                crate::sync_dock_icon(&self.app);
            }
        }

        result
    }
}

impl ScriptHost for TauriScriptHost {
    fn type_str(&self, text: &str, delay_ms: u64) -> Result<(), HostError> {
        // 首次输出前先确保焦点在目标窗口（见 StandbyGate 的说明）。
        // 空串不注入任何按键，没必要为此弹待命窗口。
        if !text.is_empty() {
            self.standby
                .ensure_ready(&self.app, &self.cancel, self.on_standby_cancel.clone())?;
        }

        let mut guard = self.typer.lock().unwrap();
        if guard.is_none() {
            let cfg = self
                .app
                .state::<WorkerState>()
                .config
                .read()
                .unwrap()
                .clone();
            // 与发送/部署共用同一个 Typer 实现：Return/Tab/Shift 特判、非 ASCII 回退 Unicode 输入
            *guard = Some(Typer::new(&cfg, self.cancel.clone()).map_err(HostError::Failed)?);
        }
        let typer = guard.as_mut().expect("刚刚已初始化");

        let total = text.chars().count();
        let mut snippet = String::new();

        for ch in text.chars() {
            if self.cancel.is_cancelled() {
                return Err(HostError::Cancelled);
            }
            typer
                .send_char(ch, !typer.send_real_keys())
                .map_err(HostError::Failed)?;

            let typed = self.typed.fetch_add(1, Ordering::Relaxed) + 1;
            if snippet.chars().count() < SNIPPET_LIMIT {
                snippet.push(ch);
            }
            // 换成可见字符，避免片段里的换行把日志/界面撑乱
            let display_snippet: String = snippet
                .chars()
                .map(|c| if c.is_control() { '·' } else { c })
                .collect();
            self.emit_progress(typed, total, &display_snippet);

            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        Ok(())
    }

    /// `$.confirm`：弹**系统原生**三按钮对话框（是 / 否 / 取消）。
    ///
    /// 语义：是 → `true`；否 → `false`；取消 → 中止脚本；**不设超时**，一直等用户回答。
    fn confirm(&self, message: &str) -> Result<ConfirmChoice, HostError> {
        match self.ask_dialog(CONFIRM_TITLE, message, MessageDialogButtons::YesNoCancel) {
            MessageDialogResult::Yes => Ok(ConfirmChoice::Yes),
            MessageDialogResult::No => Ok(ConfirmChoice::No),
            // Cancel = 用户要求停掉脚本；Custom 只会在换用 *Custom 按钮时出现，一并按中止处理
            _ => Err(HostError::Cancelled),
        }
    }

    /// 文件修改授权：三个按钮分别是「允许一次 / 本次运行内该目录都允许 / 拒绝」。
    ///
    /// 「本次运行内都允许」由能力实现记住（`FileState`），不在这里维护状态 ——
    /// 宿主只回答这一次问题。
    fn allow_file_change(
        &self,
        action: &str,
        path: &Path,
        scope_dir: &Path,
    ) -> Result<FileDecision, HostError> {
        let message = format!(
            "脚本请求{action}\n\n文件：{}\n目录：{}\n\n选「{FILE_ALLOW_DIR}」后，本次运行中该目录下的同类操作不再询问。",
            path.display(),
            scope_dir.display()
        );

        let result = self.ask_dialog(
            FILE_TITLE,
            &message,
            MessageDialogButtons::YesNoCancelCustom(
                FILE_ALLOW.to_string(),
                FILE_ALLOW_DIR.to_string(),
                FILE_DENY.to_string(),
            ),
        );

        // 自定义按钮的结果按文案回传，用文案区分三个分支
        Ok(match result {
            MessageDialogResult::Custom(choice) if choice == FILE_ALLOW => FileDecision::Allow,
            MessageDialogResult::Custom(choice) if choice == FILE_ALLOW_DIR => {
                FileDecision::AllowDir
            }
            // 其它（含直接关掉窗口）一律按拒绝处理
            _ => FileDecision::Deny,
        })
    }

    fn progress(&self, typed: usize, total: usize) {
        self.emit_progress(typed, total, "");
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl ConsoleHook for TauriScriptHost {
    /// `console.*` 送到脚本窗口的 Console 面板。
    ///
    /// GUI 应用的标准流用户看不到，所以这里**不写** stdout/stderr（仅留 `log::debug!`
    /// 供排查）；CLI 走 [`script_engine::StdoutConsole`]，仍然打到终端。
    fn write(&self, level: &str, text: &str) {
        log::debug!("[script:{level}] {text}");
        self.console.push(level, text);
    }
}

/// 当前能力清单 + 命名空间名字（喂给前端的 CodeMirror 补全与文档面板）。
///
/// 名字随清单一并下发，前端就不必再硬编码 `ClipBeam` / `$`。
pub fn capability_list() -> clipbeam_scripting::CapabilityList {
    clipbeam_scripting::capability_list()
}

/// 脚本源码是否可能注入键盘事件（只用于 Console 面板的运行行提示）。
///
/// 待命窗口**不再**依赖这个判定：它只由 [`crate::standby::StandbyGate`] 在首次
/// `$.type_str` 之前触发，所以纯计算脚本永远不会看到待命窗口。
/// 这里保守一点（源码里出现 `.type_str` 就算「会输出」）只是为了让 Console 里
/// `▶ 运行 xxx` 这行能提示「(含键盘输出)」，判错没有任何副作用。
pub fn may_type(source: &str) -> bool {
    source.contains(".type_str")
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn may_type_detects_typical_calls() {
        assert!(may_type(r#"$.type_str("你好", 10)"#));
        assert!(may_type(r#"const $ = ClipBeam; $.type_str("x")"#));
        assert!(may_type(r#"ClipBeam.type_str("x")"#));
        // 变量持有函数也命中（保守判定）
        assert!(may_type(r#"const out = ClipBeam.type_str"#));
    }

    #[test]
    fn may_type_ignores_pure_computation() {
        assert!(!may_type(r#"const b = await $.read("a.bin"); $.md5(b)"#));
        assert!(!may_type(r#"console.log("只算不输出")"#));
        assert!(!may_type(""));
    }
}
