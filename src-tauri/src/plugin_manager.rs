//! 插件管理器：发现、启停、动作派发、插件日志。
//!
//! # 线程模型（错误隔离的关键）
//!
//! **一个插件一条独立的 OS 线程**（该线程上再建一个 current-thread tokio 运行时来驱动引擎）。
//! 于是：
//!
//! * 插件死循环只卡住它自己的线程 —— 应用与其它插件照常；
//! * 插件线程 panic 只影响那一个插件（[`PluginManager`] 把它标成「异常」）；
//! * 停用插件 = 给它的线程发一条 `Stop`，取消信号置位，然后等它退出。
//!
//! 为什么不像脚本那样复用 `spawn_blocking`：脚本是「跑完即释放」，而插件是常驻的，
//! 需要一个**长期存活、可被反复唤醒**的执行体。用 `spawn_blocking` 会长期占用线程池的工作线程。
//!
//! # 与 Tauri 的边界
//!
//! 本模块只管「插件的生命周期」；具体 UI（toast / 通知 / 对话框 / 托盘）在
//! [`crate::plugin_host`]。这样管理器可以在没有窗口的情况下被测试（见文件末尾的单测）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clipbeam_plugins::catalog::{self, PluginRecord};
use clipbeam_plugins::{runtime, PermissionSet, PluginHost, PluginMeta, PluginRuntimeOptions};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::console_panel::{ConsoleBuffer, ConsoleLine};
use crate::plugin_host::TauriPluginHost;

/// 插件在界面上的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    /// 清单或目录有问题，不能启用。
    Invalid,
    /// 可用但已关闭（默认状态）。
    Disabled,
    /// 正在运行。
    Active,
    /// 启用过程中出错（错误原文见 `error`）。
    Error,
    /// 被要求停用但线程没在预期时间内退出。
    StopTimeout,
}

/// 一个插件对界面的完整描述（`list_plugins` 的返回项）。
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    /// 插件 id（等于目录名）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 版本号。
    pub version: String,
    /// 说明。
    pub description: Option<String>,
    /// 作者。
    pub author: Option<String>,
    /// 插件目录。
    pub dir: String,
    /// 入口文件名。
    pub entry: String,
    /// 状态。
    pub state: PluginState,
    /// 声明的权限（`Permission::name()` 的名字列表）。
    pub permissions: Vec<String>,
    /// 清单里声明的托盘菜单（`(id, label)`）。
    pub menus: Vec<(String, String)>,
    /// 最近一次错误 / 清单问题的原文。
    pub error: Option<String>,
    /// 当前是否可用（清单正常）。
    pub usable: bool,
}

/// 正在运行的插件。
struct RunningPlugin {
    /// 发送给插件线程的队列（`Send` 失败说明线程已经退了）。
    tx: std::sync::mpsc::Sender<PluginCommand>,
    /// 插件线程的处理句柄（停用时用来等待退出；`Err` 表示 panic）。
    join: Option<std::thread::JoinHandle<()>>,
    /// 停用请求置位后，能力调用与动作派发都会尽早收手。
    cancel: script_engine::CancelSignal,
    /// 动作回调正在跑（用于「卡住」预检）。
    busy: Arc<AtomicBool>,
    /// 插件身份（诊断日志与「按插件目录解析路径」时用）。
    meta: PluginMeta,
}

/// 发给插件线程的命令。
enum PluginCommand {
    /// 派发一个动作（载荷是 JSON）。
    Action {
        /// 动作 id（对应 `menus[].id`）。
        id: String,
        /// 传给回调的载荷。
        payload: serde_json::Value,
    },
    /// 停用：置取消信号并让线程退出。
    Stop,
}

/// 单个动作的等待上限：超过就认为这个插件卡住了。
///
/// 引擎的中断是**协作式**的（只在 `await` / 定时器处让出），所以一个纯计算的死循环
/// 无法被强行打断 —— 这里只能做到「不再给它排新活」，避免事件越堆越多。
const ACTION_WATCHDOG_MS: u64 = 5_000;

/// 停用插件时等线程退出的上限。
const STOP_WAIT_MS: u64 = 3_000;

/// 插件管理器（作为 Tauri State 存在）。
pub struct PluginManager {
    app: AppHandle,
    /// 发现结果与运行状态的合并视图。
    records: Mutex<Vec<PluginInfo>>,
    /// 正在运行的插件。
    running: Mutex<HashMap<String, RunningPlugin>>,
    /// 每个插件一个日志缓冲（按 id）。
    consoles: Mutex<HashMap<String, Arc<ConsoleBuffer>>>,
}

impl PluginManager {
    /// 建管理器并做一次发现（不启用任何插件）。
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            records: Mutex::new(Vec::new()),
            running: Mutex::new(HashMap::new()),
            consoles: Mutex::new(HashMap::new()),
        }
    }

    /// 重新扫描插件目录，合并运行状态后返回列表。
    ///
    /// 发现失败（读目录出错）不改变已有列表：宁可显示上一次的结果，也不要让界面空掉。
    pub fn refresh(&self) -> Vec<PluginInfo> {
        let discovered = match catalog::scan() {
            Ok(records) => records,
            Err(err) => {
                log::warn!("扫描插件目录失败：{err}");
                return self.list();
            }
        };

        let previous: HashMap<String, PluginState> = self
            .list()
            .into_iter()
            .map(|info| (info.id, info.state))
            .collect();

        let infos: Vec<PluginInfo> = discovered
            .iter()
            .map(|record| to_info(record, previous.get(&record.id).copied()))
            .collect();

        *self.records.lock().unwrap() = infos.clone();
        infos
    }

    /// 当前列表（不重新扫描）。
    ///
    /// 运行时状态由 [`PluginManager::running`] 决定，不信任 `records` 里的旧状态 ——
    /// 插件线程可能已经 panic 退出，而 `records` 还停在 `Active`。
    pub fn list(&self) -> Vec<PluginInfo> {
        let running = self.running.lock().unwrap();
        let mut infos = self.records.lock().unwrap().clone();

        for info in &mut infos {
            if let Some(plugin) = running.get(&info.id) {
                debug_assert_eq!(plugin.meta.id, info.id, "运行表与列表的 id 必须一致");
                // 线程退了（panic 或提前返回）→ 标异常，别让界面显示「运行中」
                if plugin
                    .join
                    .as_ref()
                    .is_some_and(std::thread::JoinHandle::is_finished)
                {
                    info.state = PluginState::Error;
                    if info.error.is_none() {
                        info.error = Some("插件线程已退出（可能是内部 panic）".to_string());
                    }
                } else {
                    info.state = PluginState::Active;
                }
            }
        }

        infos
    }

    /// 启用一个插件。
    ///
    /// 失败原因会写进列表的 `error`（界面直接显示），并返回同样的错误。
    pub fn enable(&self, id: &str) -> Result<(), String> {
        // 先做一次「能不能启用」的全部检查（清单、权限、入口），
        // 这样用户看到的是「你的清单缺权限」而不是一个 JS 语法错误。
        self.refresh();
        let Some(info) = self.list().into_iter().find(|info| info.id == id) else {
            return Err(format!("找不到插件 {id:?}（试试刷新）"));
        };
        if !info.usable {
            return Err(info
                .error
                .unwrap_or_else(|| format!("插件 {id:?} 的清单不可用")));
        }
        if self.running.lock().unwrap().contains_key(id) {
            return Err(format!("插件 {id:?} 已经在运行"));
        }

        // 重新读一次清单（refresh 后 records 里的 manifest 已经解析好了）
        let record = self
            .record(id)
            .ok_or_else(|| format!("找不到插件 {id:?}"))?;
        let manifest = record
            .manifest
            .clone()
            .ok_or_else(|| format!("插件 {id:?} 的清单不可用"))?;

        let meta = PluginMeta {
            id: manifest.id.clone(),
            name: manifest.display_name().to_string(),
            version: manifest.version_or_default().to_string(),
            description: manifest.description.clone(),
            author: manifest.author.clone(),
            dir: std::path::PathBuf::from(&record.dir),
            entry: manifest.entry_name().to_string(),
        };

        // 入口先读 + 先转译：语法错误要在这里就报，不要等线程起来才失败
        // 入口读不了（文件缺失 / 语法错误）时把原因记进列表，再把错误原样抛给界面
        let source = runtime::load_entry(&meta).inspect_err(|err| {
            self.set_error(id, err);
        })?;

        let console = self.console_for(id);
        console.push(
            "info",
            &format!("▶ 启用 {}{}", meta.name, version_suffix(&meta)),
        );

        let cancel = script_engine::CancelSignal::new();
        let (tx, rx) = std::sync::mpsc::channel::<PluginCommand>();
        let busy = Arc::new(AtomicBool::new(false));

        // `cancel` 的克隆给宿主（能力用它判断「是否已被要求停止」），
        // 本体留给运行时与运行表
        let host: Arc<dyn PluginHost> = Arc::new(TauriPluginHost::new(
            self.app.clone(),
            meta.clone(),
            cancel.clone(),
        ));

        let thread_meta = meta.clone();
        let thread_console = console.clone();
        let thread_permissions = manifest.permissions;
        let thread_busy = busy.clone();
        // 线程里也要一份取消信号（引擎的 sleep 与能力检查靠它提前收手）
        let thread_cancel = cancel.clone();

        let join = std::thread::Builder::new()
            // 线程名带插件 id：排查时 `sample` 一眼看出是哪个插件
            .name(format!("clipbeam-plugin-{}", meta.id))
            .spawn(move || {
                run_plugin_thread(
                    thread_meta,
                    thread_permissions,
                    host,
                    thread_console,
                    thread_cancel,
                    source,
                    rx,
                    thread_busy,
                )
            })
            .map_err(|err| {
                let message = format!("创建插件线程失败：{err}");
                self.set_error(id, &message);
                message
            })?;

        self.running.lock().unwrap().insert(
            id.to_string(),
            RunningPlugin {
                tx,
                join: Some(join),
                cancel,
                busy,
                meta: meta.clone(),
            },
        );

        self.mark(id, PluginState::Active, None);
        self.notify_changed();
        log::info!("插件 {}（{}）已启用", meta.name, meta.id);
        Ok(())
    }

    /// 停用一个插件：置取消信号 → 请线程退出 → 等它收尾。
    pub fn disable(&self, id: &str) -> Result<(), String> {
        let Some(plugin) = self.running.lock().unwrap().remove(id) else {
            // 没在跑就是「已是关闭状态」，不算错误（界面上的开关可能重复点）
            self.mark(id, PluginState::Disabled, None);
            self.notify_changed();
            return Ok(());
        };

        // 先取消：插件正在跑的长动作（对话框、sleep）会尽早收手
        plugin.cancel.cancel();
        let _ = plugin.tx.send(PluginCommand::Stop);

        let stopped = wait_for_exit(plugin.join, STOP_WAIT_MS);
        match stopped {
            true => {
                self.mark(id, PluginState::Disabled, None);
                self.console_for(id).push("info", "■ 已停用");
            }
            false => {
                // 线程没退：如实告诉用户，而不是假装停掉了
                let message = format!(
                    "插件 {id:?} 没有在 {STOP_WAIT_MS}ms 内退出（可能卡在某个循环里）；\
                     应用不会再向它派发动作"
                );
                log::warn!("{message}");
                self.mark(id, PluginState::StopTimeout, Some(message.clone()));
                self.console_for(id).push("error", &message);
            }
        }

        self.notify_changed();
        Ok(())
    }

    /// 重新加载（禁用 + 启用）。
    pub fn reload(&self, id: &str) -> Result<(), String> {
        self.disable(id)?;
        self.enable(id)
    }

    /// 派发一个动作给插件线程。
    ///
    /// 返回值只表示「有没有排进队列」；真正的失败（回调抛异常）由插件线程写进插件日志。
    pub fn dispatch_action(
        &self,
        id: &str,
        action_id: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        let running = self.running.lock().unwrap();
        let Some(plugin) = running.get(id) else {
            return Err(format!("插件 {id:?} 没有在运行"));
        };

        if plugin
            .join
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return Err(format!("插件 {id:?} 的线程已退出"));
        }

        // 「卡住」预检：上一个动作还没回来就不再排队，避免事件越堆越多
        if plugin.busy.load(Ordering::SeqCst) {
            let message = format!(
                "插件 {id:?} 上一个动作超过 {}s 还没返回（疑似卡住），本次动作已忽略",
                ACTION_WATCHDOG_MS / 1000
            );
            log::warn!("{message}");
            self.console_for(id).push("warn", &message);
            return Err(message);
        }

        plugin
            .tx
            .send(PluginCommand::Action {
                id: action_id.to_string(),
                payload,
            })
            .map_err(|_| format!("插件 {id:?} 的线程已退出"))
    }

    /// 插件日志缓冲（没有就建一个）。
    pub fn console_for(&self, id: &str) -> Arc<ConsoleBuffer> {
        let mut consoles = self.consoles.lock().unwrap();
        consoles
            .entry(id.to_string())
            .or_insert_with(|| {
                let app = self.app.clone();
                let plugin_id = id.to_string();
                // 事件名沿用脚本那套（前端 Console 面板逻辑可以复用），
                // 但带上 plugin_id 让前端能区分是哪个插件的日志。
                ConsoleBuffer::with_emitter(Arc::new(move |event, line: &ConsoleLine| {
                    let payload = PluginConsoleLine {
                        plugin_id: plugin_id.clone(),
                        line: line.clone(),
                    };
                    let _ = app.emit(event, payload);
                }))
            })
            .clone()
    }

    /// 取一个插件的日志快照。
    pub fn console_snapshot(&self, id: &str) -> Vec<ConsoleLine> {
        self.console_for(id).snapshot()
    }

    /// 清空一个插件的日志。
    pub fn clear_console(&self, id: &str) {
        self.console_for(id).clear();
    }

    /// 停掉全部插件（应用退出时调用）。
    ///
    /// 退出路径上**不能等太久**：用户点了退出就该退出，卡在这里的插件留给进程结束去清理，
    /// 但要先把取消信号置位（让正在跑的动作尽早收手）并尝试短暂等待。
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.running.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.disable(&id);
        }
    }

    /// 正在运行的插件 id（诊断与测试用）。
    pub fn running_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.running.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// 一个插件的目录（托盘图标等「相对插件目录」的路径解析要用）。
    pub fn plugin_dir(&self, id: &str) -> Option<std::path::PathBuf> {
        self.list()
            .into_iter()
            .find(|info| info.id == id)
            .map(|info| std::path::PathBuf::from(info.dir))
    }

    /// 当前**清单声明**的插件动作菜单（用于同步托盘）。
    ///
    /// 只包含「可用且已启用意图」的插件吗？不 —— 这里是**全部可用插件**的菜单项。
    /// 理由：菜单项属于「插件的入口」，点它才决定要不要唤醒插件；插件没运行时
    /// `dispatch_action` 会给出明确提示（比菜单项时有时无更好理解）。
    /// 界面上的启停开关控制的是「插件线程跑不跑」，不是「菜单项在不在」。
    pub fn plugin_tray_items(&self) -> Vec<crate::tray::PluginTrayItem> {
        self.list()
            .into_iter()
            .filter(|info| info.usable)
            .flat_map(|info| {
                info.menus
                    .into_iter()
                    .map(move |(action_id, label)| crate::tray::PluginTrayItem {
                        plugin_id: info.id.clone(),
                        action_id,
                        label,
                        enabled: true,
                    })
            })
            .collect()
    }

    /// 取一条发现记录（内部用）。
    fn record(&self, id: &str) -> Option<PluginRecord> {
        catalog::scan()
            .ok()?
            .into_iter()
            .find(|record| record.id == id)
    }

    /// 更新列表里的状态与错误。
    fn mark(&self, id: &str, state: PluginState, error: Option<String>) {
        let mut records = self.records.lock().unwrap();
        if let Some(info) = records.iter_mut().find(|info| info.id == id) {
            info.state = state;
            info.error = error;
        }
    }

    /// 记一条错误（状态置为 `Error`）。
    fn set_error(&self, id: &str, message: &str) {
        self.mark(id, PluginState::Error, Some(message.to_string()));
        self.console_for(id).push("error", message);
        self.notify_changed();
    }

    /// 广播「插件列表变了」。
    fn notify_changed(&self) {
        let _ = self.app.emit("plugins-changed", self.list());
    }
}

/// 插件日志行（带插件 id 的事件载荷）。
#[derive(Debug, Clone, Serialize)]
pub struct PluginConsoleLine {
    /// 哪个插件的日志。
    pub plugin_id: String,
    /// 行内容（结构与脚本 Console 一致）。
    pub line: ConsoleLine,
}

/// 把发现记录转成界面用的信息。
fn to_info(record: &PluginRecord, previous: Option<PluginState>) -> PluginInfo {
    let state = if !record.is_usable() {
        PluginState::Invalid
    } else {
        match previous {
            // 已经不在运行的插件，之前显示的 Active 状态要回落到 Disabled
            Some(PluginState::Active) | Some(PluginState::StopTimeout) => PluginState::Disabled,
            Some(state) => state,
            None => PluginState::Disabled,
        }
    };

    PluginInfo {
        id: record.id.clone(),
        name: record.display_name().to_string(),
        version: record.version().to_string(),
        description: record
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.description.clone()),
        author: record
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.author.clone()),
        dir: record.dir.clone(),
        entry: record
            .manifest
            .as_ref()
            .map(|manifest| manifest.entry_name().to_string())
            .unwrap_or_else(|| "-".to_string()),
        state,
        permissions: record
            .permissions()
            .granted()
            .into_iter()
            .map(|permission| permission.name().to_string())
            .collect(),
        menus: record
            .manifest
            .as_ref()
            .map(|manifest| {
                manifest
                    .menus
                    .iter()
                    .map(|item| (item.id.clone(), item.label.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        error: record.problem.clone(),
        usable: record.is_usable(),
    }
}

/// `（vX.Y.Z）` 形式的版本后缀（界面/日志里用；版本是默认值时省略）。
fn version_suffix(meta: &PluginMeta) -> String {
    if meta.version == "0.0.0" {
        String::new()
    } else {
        format!("（v{}）", meta.version)
    }
}

/// 等插件线程退出；`true` 表示已退出。
fn wait_for_exit(join: Option<std::thread::JoinHandle<()>>, timeout_ms: u64) -> bool {
    let Some(join) = join else {
        return true;
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if join.is_finished() {
            // `join()` 只为回收句柄；线程里的 panic 已经由日志反映
            let _ = join.join();
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// 插件线程主体：建运行时 → 跑入口 → 循环处理动作 → 退出。
///
/// 每个插件自己的 tokio current-thread 运行时在这里建：引擎的 `sleep` / 异步能力
/// 需要一个 tokio 上下文，而这条线程是插件独占的，阻塞它不会影响别人。
#[allow(clippy::too_many_arguments)]
fn run_plugin_thread(
    meta: PluginMeta,
    permissions: PermissionSet,
    host: Arc<dyn PluginHost>,
    console: Arc<ConsoleBuffer>,
    cancel: script_engine::CancelSignal,
    source: String,
    rx: std::sync::mpsc::Receiver<PluginCommand>,
    busy: Arc<AtomicBool>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            console.push("error", &format!("创建插件运行时失败：{err}"));
            return;
        }
    };

    let entry_name = meta.entry.clone();
    let plugin_id = meta.id.clone();
    let console_for_engine = console.clone();

    let runtime = match rt.block_on(clipbeam_plugins::create_runtime(
        host.clone(),
        PluginRuntimeOptions::new(meta.clone())
            .permissions(permissions)
            .cancel(cancel.clone())
            .console(console_for_engine as Arc<dyn script_engine::ConsoleHook>),
    )) {
        Ok(runtime) => runtime,
        Err(err) => {
            console.push("error", &format!("初始化插件运行时失败：{err:#}"));
            return;
        }
    };

    // 跑入口：插件在这里登记动作回调
    if let Err(err) = rt.block_on(runtime.run_entry(&entry_name, &source)) {
        console.push("error", &format!("插件入口执行失败：{err}"));
        return;
    }

    let actions = rt.block_on(runtime.registered_action_keys());
    if actions.is_empty() {
        console.push(
            "warn",
            "入口没有登记任何动作：清单里的 menus 需要 $plugin.tray.onAction 实现",
        );
    } else {
        console.push("info", &format!("已登记动作：{}", actions.join(", ")));
    }

    // 事件循环：一次一个动作（插件的 JS 是单线程的，并发没有意义）
    while let Ok(command) = rx.recv() {
        match command {
            PluginCommand::Stop => break,
            PluginCommand::Action { id, payload } => {
                if cancel.is_cancelled() {
                    break;
                }
                busy.store(true, Ordering::SeqCst);
                let outcome = rt.block_on(runtime.eval_action(&id, payload));
                busy.store(false, Ordering::SeqCst);

                match outcome {
                    Ok(()) => log::debug!("插件 {plugin_id} 动作 {id:?} 执行完成"),
                    Err(err) => {
                        let message = format!("动作 {id:?} 执行失败：{err}");
                        console.push("error", &message);
                    }
                }
            }
        }
    }

    log::debug!("插件 {plugin_id} 线程退出");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipbeam_plugins::PluginManifest;

    fn record_usable(id: &str) -> PluginRecord {
        let json = format!(
            r#"{{ "id": {id:?}, "name": "显示名", "version": "1.2.3",
                 "permissions": {{ "feedback": true, "tray": true }},
                 "menus": [{{ "id": "go", "label": "开始" }}] }}"#
        );
        PluginRecord {
            id: id.to_string(),
            dir: format!("/tmp/{id}"),
            manifest: Some(PluginManifest::from_json(&json).expect("清单应当合法")),
            problem: None,
            skipped: None,
        }
    }

    fn record_broken(id: &str) -> PluginRecord {
        PluginRecord {
            id: id.to_string(),
            dir: format!("/tmp/{id}"),
            manifest: None,
            problem: Some("找不到入口文件 index.js".to_string()),
            skipped: None,
        }
    }

    #[test]
    fn usable_record_becomes_a_disabled_entry() {
        let info = to_info(&record_usable("demo"), None);
        assert_eq!(info.id, "demo");
        assert_eq!(info.name, "显示名");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.state, PluginState::Disabled, "发现不等于启用");
        assert!(info.usable);
        assert!(info.error.is_none());
        assert_eq!(info.entry, "index.js");
        assert_eq!(
            info.permissions,
            vec!["feedback".to_string(), "tray".to_string()],
            "权限按固定顺序展示"
        );
        assert_eq!(info.menus, vec![("go".to_string(), "开始".to_string())]);
    }

    #[test]
    fn broken_record_is_invalid_and_keeps_the_reason() {
        let info = to_info(&record_broken("bad"), None);
        assert_eq!(info.state, PluginState::Invalid);
        assert!(!info.usable);
        assert_eq!(info.error.as_deref(), Some("找不到入口文件 index.js"));
        assert_eq!(info.version, "-", "坏插件没有版本");
        assert!(info.permissions.is_empty());
    }

    /// 上一轮显示的 `Active` 在重新扫描后必须回落成 `Disabled`：
    /// 否则刷新一下界面就会显示「运行中」，而插件其实没跑。
    #[test]
    fn stale_active_state_falls_back_to_disabled() {
        for stale in [PluginState::Active, PluginState::StopTimeout] {
            let info = to_info(&record_usable("demo"), Some(stale));
            assert_eq!(info.state, PluginState::Disabled, "{stale:?} 应当回落");
        }

        // 其它状态原样保留（错误信息不该被刷新抹掉）
        let info = to_info(&record_usable("demo"), Some(PluginState::Error));
        assert_eq!(info.state, PluginState::Error);
    }

    #[test]
    fn state_serializes_lowercase_for_the_frontend() {
        let json = serde_json::to_string(&PluginState::StopTimeout).unwrap();
        assert_eq!(json, "\"stoptimeout\"");
    }

    #[test]
    fn version_suffix_is_omitted_for_the_default_version() {
        let mut meta = PluginMeta {
            id: "demo".into(),
            name: "demo".into(),
            version: "0.0.0".into(),
            description: None,
            author: None,
            dir: std::path::PathBuf::from("/tmp/demo"),
            entry: "index.js".into(),
        };
        assert_eq!(version_suffix(&meta), "");
        meta.version = "1.2.3".into();
        assert_eq!(version_suffix(&meta), "（v1.2.3）");
    }
}
