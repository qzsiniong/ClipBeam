# ClipBeam 双向剪贴板书桥 实现计划

## 目标与背景

宿主机（macOS，**架构同时兼容 Windows**）与远程浏览器（无网络通路的 VDI/远程桌面页面）之间做剪贴板桥接，利用两条非对称物理通道：

* **宿主机 → 远程：键盘通道**。Rust 模拟键盘逐字输入，浏览器页面 JS 监听按键还原文本。

* **远程 → 宿主机：光通道（二维码）**。JS 循环播放二维码（可多帧），Rust 连续截屏解码组包。

形态约定（按用户反馈）：

* Rust 端是**带系统托盘的常驻普通程序**（双击启动、无终端窗口），不做倒计时：触发时用户已把焦点切到远程页面，仅保留约 200ms 的"热键修饰键释放"等待；托盘菜单可发起/取消全部操作并显示状态。

* Web 端是**尽量小的单文件 HTML**（内联全部 CSS/JS/二维码库），方便传输到远程。

* Rust 端提供**键盘部署接收页**功能：把单文件接收页 base32 编码后包进一个极小的"自解压 HTML"，用键盘打进远程的记事本，用户另存为 .html 双击打开即可，无需任何文件通道。

* **全部参数 UI 可配置并持久化**：发送/接收/中止三组热键、键延迟、settle 延迟、二维码帧长、帧间隔、接收超时、文本大小上限；托盘"设置…"打开配置窗口，改完即时生效（热键重新注册）并写入配置文件。

* **紧急中止优先**：发送链路一旦焦点不对会向错误窗口狂敲键，危害最大。设置独立的"中止热键"（任务运行期间才注册），typer 每敲一个字符前检查取消标志，触发后几十毫秒内停止；再按一次触发热键、点托盘"取消"同样中止。中止后通知明确提示"可能已有 N 个字符落入错误窗口，不会自动清理（避免误删）"。

仅支持文本（UTF-8），图片/富文本 v1 明确提示不支持。

## Repository Research

* 仓库目录为空，无既有代码与约束。

* 已核实关键 crate（2026-09）：

  * `enigo` 0.6.1（键盘模拟；`Enigo::new(&Settings)` + `Keyboard::text/key`；mac/Windows 均支持）

  * `global-hotkey` 0.8.0（全局热键，mac/Windows/Linux）

  * `tray-icon` 0.24.2（系统托盘 + 内置 muda 菜单，mac/Windows/Linux；macOS 要求主线程 RunLoop）

  * `winit` 0.30（事件循环：主线程统一驱动托盘菜单事件与全局热键事件）

  * `xcap` 0.9.8（跨平台截屏，带 `image` feature：`Monitor::all()` → `capture_image()` 返回 `RgbaImage`；Windows 走 WGC/GDI）

  * `rqrr` 0.11.0（屏幕二维码识别：`PreparedImage::prepare(luma)` → `detect_grids()` → `grid.decode()`）

  * `arboard` 3（跨平台剪贴板）、`data-encoding` 2（BASE32\_NOPAD）、`clap` 4、`image`

  * 设置窗口：`egui` + `egui-winit` + `egui_glow`（复用已有 winit 事件循环按需开窗，跨平台 OpenGL，不引入第二事件循环）

  * 配置持久化：`serde` + `serde_json` + `dirs`（JSON 存于 `~/Library/Application Support/ClipBeam/config.json` 与 `%APPDATA%/ClipBeam/config.json`）

  * 通知按平台 cfg 分发：macOS `mac-notification-sys` 0.6；Windows `wintoast`（Toast，底层 windows crate）。不选 notify-rust（不支持 Windows）。

  * **无摘要类 crate**：CRC32 在 Rust/JS 两端各自表驱动实现（约 20 行）。

* 远程环境很可能无外网：单文件 HTML 内联全部依赖，不引用任何 CDN。

## 协议设计

### 协议 A：宿主机 → 远程（键盘通道，用于传剪贴板文本）

字符集：RFC 4648 base32 **小写无填充**（`abcdefghijklmnopqrstuvwxyz234567`），全部免 Shift。

打字流帧格式（实际无空格，`0`/`1` 为数字行物理按键）：

```
ClipBeam（标题）
─────────────
发送到远程（宿主机剪贴板）      <发送热键>
从远程接收                      <接收热键>
键盘部署接收页到远程…
复制自解压接收页（到宿主机剪贴板）
─────────────
取消当前任务                    <中止热键，默认 Esc>
状态：空闲 / 发送中… / 接收中 x/N
─────────────
设置…（热键 / 延迟 / 阈值 / 超时）
在 Finder/资源管理器中显示接收页
退出
```

> 两端约定：**CRC32 的输入统一是"拼包后的 base32 字符串字节"**，不解码先校验，两条通道完全对称。

* `0`：起始/字段分隔符；`1`：整帧结束符。只用数字行 0/1 做控制符——免 Shift 且在各键盘布局上物理位置最稳定。

* `clipbeam1`：固定魔术头（含协议版本 v1），防用户正常输入误触发接收状态机。

* CRC32 摘要仅 7 个 base32 字符，避免 hex 中的 0/1/8/9 与控制符冲突。

* CRC32 定位为**传输差错检测**（丢键、重复键、布局错配导致的系统性乱码必然校验失败，偶然漏检率 1/2³²），不防恶意篡改——匹配本场景威胁模型。

* JS 状态机：IDLE 见 `0` → 校验魔术头 → 收 PAYLOAD（仅 `a-z2-7`）→ 收 DIGEST → 见 `1` 成帧；CRC32 比对通过后再 base32 解码、`TextDecoder` 还原 UTF-8、写剪贴板；接收期间 `preventDefault()` 吞键；10 秒空闲或非法控制符自动复位 IDLE；页面有"暂停接收"开关。

* 兜底：`writeText` 被拒时展示文本框 + "手动复制"按钮。

* Rust 发送：触发 → arboard 读文本 → **等待 settle 200ms 让热键修饰键抬起（可配）** → enigo 逐字符发送，默认每键 3ms（可配）→ 完成通知。无倒计时。

* **中止保障（最高优先级）**：取消令牌 `Arc<AtomicBool>` 全链路共享；中止途径有三条——① 中止热键（默认 Esc，可在设置中改为 Ctrl+Alt+X 等 VDI 客户端不转发的组合，仅任务运行时注册）；② 再按一次发送热键；③ 托盘"取消当前任务"。typer 在**每个字符发送前**检查令牌（最坏停止延迟 ≈ 一个键间隔，默认 ≤10ms 级）；截屏接收循环每轮检查。中止后统计已发送字符数并通知"已中止：已发出约 N 字符，可能落在错误窗口，请人工检查；不会自动按 Backspace/Ctrl+A 以免误删远程内容"。

* 文本超过大小上限（默认 256KB，可配）直接拒绝并通知（键盘通道耗时与字符数成正比，256KB 已约 20 分钟）。

### 协议 B：远程 → 宿主机（二维码光通道）

字符集：base32 **大写无填充**（`A-Z2-7`）——QR alphanumeric 模式原生支持大写+数字，比 byte 模式容量高约 40%、屏摄更可靠；Rust 解码前统一转小写。

每帧二维码文本（`.` 也属于 QR alphanumeric 允许字符集）：

```
CB1.<总帧数>.<序号>.<整包CRC32的base32大写(7字符)>.<本分块base32数据>
```

* 不设每帧 CRC：QR Reed-Solomon 已保证帧内位级正确；整包 CRC32 同时承担批次标识与组包校验。

* JS：按钮 → `readText()`（权限拒则 textarea 手动粘贴兜底）→ base32 大写 → 每 **800 字符**一块（页面"高级"里可调帧长与帧间隔，默认 300ms）→ 逐帧二维码（EC=M，版本自动，画布约 520px、留 quiet zone）→ 循环播放，显示 `第 i/N 帧` 与停止按钮。

* Rust：热键/菜单触发 → 循环截屏（主显示器）→ 灰度、降采样到宽约 1600px → rqrr 解码；同一字符串连续重复只计一次；以 `(CRC32, 总帧数)` 为批次 key 收集 `HashMap<序号, 数据>`；遇到不同批次的帧自动清空重来；收齐全部序号或超时（默认 120 秒，可配；中止热键/菜单随时取消）→ 按序拼接 → CRC32 校验 → base32 解码 → 写剪贴板 → 系统通知（成功或缺失帧数）。

### 协议 C：键盘部署接收页（一次性引导通道）

* 编译期 `include_str!("../web/clipbeam.html")` 把接收页嵌进二进制。

* 部署时生成**自解压 HTML** 文本：极小引导头（含约 15 行 base32 解码 + `TextDecoder` + `document.write`，全 ASCII 标点/大小写）+ 模板字符串内嵌 `BASE32(接收页全文)`（主体，全小写免 Shift）+ 引导尾。

* 操作流程（托盘通知/菜单可见提示）：

  1. 远程打开记事本（Win+R → `notepad`）并点击编辑区；
  2. 宿主机托盘选"键盘部署接收页到远程"；
  3. Rust 等待 200ms 后逐键打完自解压 HTML（同 3ms 节奏、可取消）；
  4. 用户 Ctrl+S 另存为 `clipbeam.html`（自解压内容全 ASCII，记事本 ANSI 编码也不会损坏），双击打开即得完整接收页。

* 同时提供"复制自解压 HTML 到宿主机剪贴板"菜单项：若远程恰好有其他粘贴/文件通道，可一键走捷径。

* 未来可选：对接收页 gzip+base32（引导头改用 `DecompressionStream` 异步解压）进一步缩短；v1 不做。

## Files and Modules

```
clipbeam/
├── Cargo.toml
├── assets/
│   └── icon.png          # 托盘图标（自绘极简小图，include_bytes! 内嵌）
├── src/
│   ├── main.rs           # winit 事件循环（主线程）：托盘菜单/热键/设置窗事件分发、worker 线程与状态回传；
│   │                     # 顶部 windows_subsystem="windows"（release 无控制台）；
│   │                     # clap：默认托盘模式，另留 send-once / recv-once 子命令供联调测试
│   ├── config.rs         # Config（serde JSON 持久化）：发送/接收/中止三组热键、键延迟 3ms、
│   │                     # settle 200ms、接收超时 120s、大小上限 256KB；含默认值、范围校验、热键解析/格式化
│   ├── cancel.rs         # CancellationToken（Arc<AtomicBool> + 计数），typer 逐键/截屏逐轮检查
│   ├── settings.rs       # egui 设置窗口（嵌入同一 winit 循环按需创建）：热键捕获控件、数字输入、恢复默认；
│   │                     # 保存即写 JSON 并触发热键重注册
│   ├── protocol.rs       # base32 无填充编解码、表驱动 CRC32、键盘帧/QR 帧构造与解析；含单元测试
│   ├── typer.rs          # enigo 封装：逐字符输入，每字符前查取消令牌 + 可配延迟；send 与 deploy 共用
│   ├── send.rs           # 读剪贴板 → 组键盘帧 → typer 发送；中止时返回已发送字符数
│   ├── deploy.rs         # include_str! 接收页 → 自解压 HTML → typer 输出；另有"复制到剪贴板"变体
│   ├── receive.rs        # xcap 截屏循环、灰度降采样、rqrr 解码、(CRC32,总帧数) 批次组包、写剪贴板
│   ├── tray.rs           # tray-icon 图标/菜单构建、状态文案与 tooltip 更新
│   └── notify.rs         # #[cfg] 分发：macOS mac-notification-sys / Windows wintoast
└── web/
    └── clipbeam.html      # 唯一交付的接收页：内联 CSS+应用JS+CRC32+base32+qrcode 库（单文件目标 ≤ ~25KB）
```

托盘菜单：

```
ClipBeam（标题）
─────────────
发送到远程（宿主机剪贴板）      <发送热键>
从远程接收                      <接收热键>
键盘部署接收页到远程…
复制自解压接收页（到宿主机剪贴板）
─────────────
取消当前任务                    <中止热键，默认 Esc>
状态：空闲 / 发送中… / 接收中 x/N
─────────────
设置…（热键 / 延迟 / 阈值 / 超时）
在 Finder/资源管理器中显示接收页
退出
```

热键跨平台默认值：发送/接收 macOS 用 Cmd+Shift+K/J、Windows/Linux 用 Ctrl+Shift+K/J（`#[cfg]` 选默认修饰符）；中止热键默认 Esc。三组热键均可在设置窗口捕获重绑（含 F 键等 VDI 客户端通常不转发的键），发送/接收热键常驻注册，**中止热键仅任务运行期间注册、结束即注销**，避免全局占用 Esc。配置变更即时重注册并持久化。

## Implementation Steps

1. **脚手架、配置与协议层**：`cargo init`；加依赖（enigo 0.6.1、arboard 3、global-hotkey 0.8、tray-icon 0.24、winit 0.30、egui/egui-winit/egui\_glow、xcap 0.9 `image` feature、rqrr 0.11、image、data-encoding 2、serde/serde\_json、dirs、clap 4；cfg 依赖 mac-notification-sys / wintoast）。实现 `protocol.rs`（base32 无填充 + CRC32 表驱动 + 键盘帧/QR 帧构造解析）、`config.rs`（默认值/范围校验/JSON 读写/热键字符串解析与格式化）、`cancel.rs`（取消令牌）；先写单元测试（CRC32 标准向量、中英文/emoji/空串/非对齐长度往返、坏帧忽略、配置往返）。
2. **发送核心（无 UI 先行）**：`typer.rs` + `send.rs`；`send-once` 子命令验证：读剪贴板 → settle → 组帧逐键发送（参数全部取自 Config）→ **每字符前查取消令牌**；非文本/超限错误返回；中止返回已发送字符数。
3. **单文件接收页·入站**：创建 `web/clipbeam.html`，内联极简 CSS 与自实现 base32/CRC32；键盘状态机（魔术头、10 秒复位、暂停开关、吞键、进度）；writeText + 手动复制兜底。与步骤 2 做本地 E2E（ASCII/中文/emoji/50KB/Esc 取消/误触）。
4. **单文件接收页·出站**：把 qrcode-generator 压缩版以内联 `<script>` 放入同一 HTML（头部注释标注来源与 MIT 许可）；readText + 粘贴兜底、base32 大写、CRC32、800 字符分帧、300ms 循环播放、停止。确认单文件体积 ≤ \~25KB 且断网打开可用。
5. **接收核心（无 UI 先行）**：`receive.rs` + `recv-once` 子命令：xcap 截屏 → 灰度降采样 → rqrr → QR 帧解析 → 去重/批次重置 → 超时/取消 → CRC32 校验 → arboard 写回。与步骤 4 E2E：单帧/多帧/缺帧超时/播放中换批次自动重置。
6. **键盘部署功能**：`deploy.rs`：自解压 HTML 生成器（引导头 + base32 主体 + 引导尾），复用 typer 输出；另提供复制到剪贴板变体；手工演练记事本 → 另存 .html → 打开后页面完整可用。
7. **托盘常驻 + 设置窗口整合**：`main.rs` 用 winit 0.30 在主线程跑事件循环（Poll 模式下每轮 try\_recv 托盘/热键通道，并在设置窗打开时泵 egui）；构建 `tray.rs` 菜单与图标；任务在 std::thread worker 执行，状态经 mpsc 回传更新菜单/tooltip；任务开始注册中止热键、结束注销；`settings.rs` 用 egui-winit + egui\_glow 在同一循环内按需开窗（三组热键捕获按钮：按下组合即录入；键延迟/settle/超时/上限数字输入带范围校验；恢复默认；保存即写 JSON、即时重注册热键）；`notify.rs` 接 macOS/Windows 通知；release 配置 `windows_subsystem = "windows"`；生成 `assets/icon.png`。
8. **跨平台收口与验证**：mac 全量验证；Windows 侧至少 `cargo check --target x86_64-pc-windows-msvc`（缺工具链则装后检查），重点 cfg 边界、enigo SendInput、xcap Windows 截屏、wintoast 通知、egui 窗口、无终端窗口启动。

## Dependencies and Considerations

* **平台权限**：macOS 首次运行需授予"辅助功能 + 输入监控"（enigo/global-hotkey）与"屏幕录制"（xcap），未授权时把各 crate 的错误转成中文通知与菜单状态提示；Windows 一般无需特殊授权。

* **键盘布局假设**：远程会话需 US/QWERTY（小写字母与数字行主客一致）；非 QWERTY 会 CRC 校验失败（安全失败，不写错内容），界面与通知明示该假设。

* enigo 0.6 为较新 API，逐字符输入方式（`text` 单字符 vs `Key::Layout`）以 docs.rs 实测为准；键间 sleep 防事件合并/丢键。

* tray-icon/global-hotkey 在 macOS 必须挂在主线程 RunLoop —— 由 winit 事件循环统一承载；Windows 对线程模型宽松，同一套 winit 循环即可。

* xcap 在 Windows 依赖 WGC（feature `wgc`，按需要启用），实现时确认 0.9.8 默认 feature 是否足够；截屏失败给出"检查屏幕录制/显示器权限"提示。

* 单文件接收页走 `file://`：Chrome/Edge 视为安全上下文，剪贴板 API 可用；浏览器策略完全禁用时两端都有 textarea 兜底。

* 自解压部署内容刻意保持全 ASCII：主体 base32 免 Shift；引导头虽含标点但仅几百字符；记事本 ANSI 另存不会损坏。

* worker 与事件循环之间只传简单状态枚举（Idle/Sending{sent}/Receiving{got,total}/Done/Cancelled{sent}/Error(String)），菜单重建保持轻量。

* 设置窗口复用主 winit 循环：窗口只在打开期间存在，事件按需转发给 egui；不引 eframe 以避免"一进程两个事件循环"在 macOS 上的限制。配置热更新边界：热键即时重注册，延迟 settle/超时/上限在**下次任务启动时**读取（运行中任务不改参数，避免行为跳变）。

* 中止热键是安全底线：必须保证它在宿主机层面被接收（OS 级全局注册，不依赖远程焦点）；但部分 VDI 客户端会独占转发 Esc/F 键——设置窗口里给出提示文案，建议改绑 Ctrl+Alt 组合或功能键，并实测哪个不被客户端吞掉。

## Validation

* `cargo test`：CRC32 对照标准向量（空串 `00000000`、`"123456789"` → `cbf43926`）并与 JS 实现交叉比对；base32 往返（中英文/emoji/空串/长度不对齐）；键盘帧/QR 帧构造解析；批次切换、坏帧忽略。

* `cargo clippy` / `cargo build` 无警告；Windows target `cargo check` 通过。

* 托盘程序：双击启动无终端窗口；菜单各项可用；任务中状态实时刷新；退出干净（注销热键、结束 worker）。

* 设置与持久化：设置窗口修改三组热键/延迟/超时/上限后即时生效（新热键立即触发、旧热键失效）；重启程序配置仍在；越界值被拒绝；"恢复默认"可用；中止热键在任务结束后确认已注销（Esc 恢复系统原义）。

* 紧急中止实测：发送 50KB 文本途中分别用中止热键、再按发送热键、托盘取消三种方式中止，实测从按下到最后一个键落下的延迟在个位数键间隔内（3ms 延迟下 ≤ \~30ms）；通知包含已发送字符数；接收循环中止即时退出。

* 键盘通道 E2E：远程打开接收页（含步骤 6 部署出来的自解压页），托盘/热键发送 ASCII、中文、emoji、约 50KB 文本，页面进度与成功提示正常、剪贴板逐字节一致；正常打字不误触状态机。

* 二维码通道 E2E：单帧/多帧、缺帧超时通知、播放中重新生成（换批次 CRC32）接收端自动重置、写回剪贴板内容一致。

* 部署通道 E2E：记事本接收自解压 HTML → 另存 `clipbeam.html` → 打开后入站/出站功能与原单文件一致。

* 权限拒绝路径：writeText/readText 兜底文本框可用；macOS 未授权时通知可读。

## Risks

* **键盘布局不一致**：CRC32 必然校验失败（安全失败）；缓解：界面与通知明示 QWERTY 假设，失败提示附可能原因。

* **焦点错误导致向错误窗口狂敲键**（最高危）：缓解：触发前由用户确认焦点；200ms settle 仅等修饰键；typer 逐键检查取消令牌（默认 ≤30ms 可停）；三条中止途径（中止热键/重复触发键/托盘菜单）；中止通知已发送字符数且不做自动清理；默认 3ms 键延迟也可在设置中调大以便新手观察。

* **VDI 客户端吞掉中止热键**：部分远程桌面客户端独占 Esc/功能键，OS 全局热键虽不依赖焦点仍可能与客户端冲突；缓解：中止热键可重绑（设置窗口内附选择建议），任务运行时才注册，并保留托盘菜单这条不依赖键盘的中止途径。

* **Windows 无法在本机充分验证**：用 cfg 隔离平台差异 + Windows target check；运行期问题（Toast/截屏/输入）留待 Windows 实测，代码面收敛在 notify.rs/receive.rs/typer.rs 三处。

* **超大文本**：键盘通道天然慢；256KB 硬上限 + 通知拒绝；二维码帧数过多时接收耗时线性增长，120s 超时可配。

* **截屏识别率**（二维码过小/缩放/分辨率）：固定 520px 画布 + quiet zone、灰度降采样、EC=M、发送端 300ms 保证每帧至少截屏一次。

* **误触接收**：魔术头 + 页面暂停开关 + 状态机 10 秒空闲复位。

* **部署引导头标点输入受远程键盘加速键干扰**：引导头仅数百字符、全 ASCII；如个别远程桌面把 `<` 等映射异常，可改走"复制自解压 HTML 到剪贴板"菜单项配合其他通道。

