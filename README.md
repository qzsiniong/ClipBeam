# ClipBeam

宿主机 ↔ 无网络远程环境之间的**剪贴板桥接工具**。

典型场景：通过 VDI / 远程桌面 / 云桌面浏览器访问一台与外网隔离的机器，
无法使用剪贴板同步、无法传输文件。ClipBeam 利用两条"天然存在"的通道：

- **键盘通道**（宿主机 → 远程）：本机模拟逐字符按键，远程网页接收；
- **光通道**（远程 → 宿主机）：远程网页循环播放二维码，本机截屏解码。

远程侧只需要一个**单文件、零依赖、可断网 `file://` 打开**的网页
（`packages/client-vanilla/dist/index.html`，由 `packages/client-vanilla`
构建生成，`pnpm build:web` 即可产出），无需安装任何软件。

---

## 目录

- [工作原理](#工作原理)
- [快速开始](#快速开始)
- [使用方法](#使用方法)
- [脚本引擎（JS/TS）](#脚本引擎jsts)
- [配置与设置](#配置与设置)
- [命令行子命令](#命令行子命令)
- [构建](#构建)
- [测试](#测试)
- [协议规范](#协议规范)
- [限制与注意事项](#限制与注意事项)

---

## 工作原理

```mermaid
flowchart LR
    subgraph HOST["宿主机（Rust 托盘程序）"]
        CB1["本机剪贴板"]
        KB["键盘模拟<br/>macOS: CGEvent<br/>Windows: enigo"]
        CAP["截屏 + 二维码解码<br/>xcap + rqrr"]
        CB2["写入本机剪贴板"]
    end

    subgraph REMOTE["远程（单文件网页，浏览器打开）"]
        PAGE_IN["键盘接收状态机<br/>base32 + CRC32 校验"]
        RCB1["navigator.clipboard<br/>.writeText()"]
        RCB2["navigator.clipboard<br/>.readText()"]
        QR["分帧 + 循环播放二维码<br/>（内联 qrcode 库）"]
    end

    CB1 -->|"① 读文本"| KB
    KB -->|"② 逐键输入帧（协议 A）"| PAGE_IN
    PAGE_IN -->|"③ 校验通过"| RCB1

    RCB2 -->|"④ 读文本"| QR
    QR -->|"⑤ 循环显示二维码（协议 B）"| CAP
    CAP -->|"⑥ 轮询截屏、按批次组包、校验"| CB2
```

三条协议：

| 通道 | 方向 | 介质 | 帧格式 |
|---|---|---|---|
| **A** | 宿主机 → 远程 | 键盘逐字符 | `0clipbeam{ver}0<小写base32>0<7字符CRC摘要>1`（`ver=1` 未压缩 / `ver=2` zstd 压缩） |
| **B** | 远程 → 宿主机 | 二维码截屏 | `CB1.总帧数.序号.7字符CRC摘要.大写base32块` |
| **C** | 宿主机 → 远程 | 键盘（一次性引导） | 全 ASCII 单行「自解压 HTML」，打开后还原完整接收页 |

两条数据通道统一使用：

- **base32（RFC4648 无填充）**：通道 A 用小写，帧分隔符只用数字行的 `0`/`1`，
  全程不产生 Shift 组合键，任何键盘布局都能敲出；通道 B 用大写（属于 QR
  alphanumeric 字符集，单码容量更大）；
- **CRC32（IEEE，多项式 `0xEDB88320`）**：对**拼包后的 base32 字符串**计算，
  摘要取 4 字节大端再 base32（7 字符）。Rust 与 JS 各有一份表驱动实现，互为校验。

### 宿主机 → 远程（协议 A）时序

```mermaid
sequenceDiagram
    participant U as 用户
    participant App as ClipBeam（宿主机）
    participant Page as 接收页（远程浏览器）

    U->>App: 复制文本后，焦点切到远程页面，按发送热键
    App->>App: arboard 读剪贴板文本
    App->>App: UTF-8 → 小写 base32 → CRC32 摘要 → 组帧
    Note over App: 等待 settle（默认 200ms）<br/>让热键修饰键抬起
    loop 帧中每个字符（默认间隔 3ms，每键前查中止令牌）
        App->>Page: 模拟按键（CGEvent / enigo Unicode）
    end
    Page->>Page: 状态机收帧 → base32 解码 → CRC 校验
    Page->>Page: navigator.clipboard.writeText(原文)
    Page-->>U: 显示接收成功
    App-->>U: 系统通知"发送完成"
```

### 远程 → 宿主机（协议 B）时序

```mermaid
sequenceDiagram
    participant U as 用户
    participant Page as 接收页（远程浏览器）
    participant App as ClipBeam（宿主机）

    U->>Page: 点"读取远程剪贴板并生成二维码"
    Page->>Page: readText → UTF-8 → 大写 base32 → 分块
    Note over Page: 每帧含 总帧数/序号/CRC；多帧循环播放
    U->>App: 按接收热键（页面保持二维码可见）
    loop 每 250ms 逐屏截屏所有显示器（可中止/超时）
        App->>App: 各屏降采样 → rqrr 解码
        App->>App: 按 (CRC, 总帧数) 归组批次，序号去重
        Note over App: 播放端换了数据（CRC 变化）→ 整批重新收集
    end
    App->>App: 帧收齐 → 按序拼接 → CRC 校验 → base32 解码
    App->>App: 写入本机剪贴板
    App-->>U: 系统通知"接收完成，已写入剪贴板"
```

### 任务忙闲与热键状态

```mermaid
stateDiagram-v2
    [*] --> 空闲
    空闲 --> 发送中: 发送热键 / 托盘"发送"
    空闲 --> 接收中: 接收热键 / 托盘"截屏接收"
    空闲 --> 部署中: 托盘"部署接收页"
    发送中 --> 空闲: Esc、再按发送热键、托盘取消、完成或失败
    接收中 --> 空闲: Esc、再按接收热键、托盘取消、完成或超时
    部署中 --> 空闲: Esc、托盘取消、完成或失败
    note right of 发送中
        空闲仅注册发送/接收热键,
        Esc 不拦截、透传其他应用;
        忙时自动切换为 Esc + 当前
        任务热键(再按即停)
    end note
```

---

## 快速开始

### 1. 构建并启动

```bash
# macOS（推荐）：打包成可双击的 ClipBeam.app
scripts/package-macos.sh --install     # 安装到 /Applications，Spotlight 搜 ClipBeam 启动

# 或直接运行裸二进制（联调用，会带一个终端窗口）
cargo build --release
./target/release/clipbeam          # 无参数 = 常驻系统托盘（macOS 平时不显示 Dock 图标）
```

### 2. 授予权限（仅 macOS）

首次使用时系统会弹窗授权，在 **系统设置 → 隐私与安全性** 中确认：

| 权限 | 用途 |
|---|---|
| 辅助功能 | 模拟键盘输入（发送、部署） |
| 屏幕录制 | 截屏解码二维码（接收） |
| 通知 | 任务完成/失败提示 |

授权后需重启程序生效。

### 3. 把接收页弄到远程（三选一）

1. **键盘部署（无任何通道时）**：远程打开记事本并聚焦 → 托盘菜单选
   **「部署接收页到远程（键盘输入，约 3 分钟）」** → 等待逐字符敲完 →
   把记事本内容另存为 `clipbeam.html`（UTF-8 编码）→ 浏览器打开；
2. **剪贴板部署（远程桌面自带剪贴板同步时）**：托盘选
   **「部署接收页（复制到宿主机剪贴板）」** → 在远程粘贴保存为 `.html`；
3. 执行 `pnpm build:web` 构建接收页，把产物
   `packages/client-vanilla/dist/index.html` 通过任意可用方式传过去。

### 4. 双向使用

接收页在远程浏览器打开后（断网亦可，`file://` 直接打开）：

- **宿主机 → 远程**：本机复制文本 → 鼠标点一下远程页面使其获得焦点 →
  按 `Cmd+Shift+K`（Windows 为 `Ctrl+Shift+K`）；
- **远程 → 宿主机**：远程页面点 **「读取远程剪贴板并生成二维码」** →
  二维码开始循环播放 → 宿主机按 `Cmd+Shift+J`。

---

## 使用方法

### 发送（宿主机 → 远程）

1. 在宿主机复制任意文本（限纯文本，默认上限 256 KB）；
2. 把焦点切到远程浏览器的 ClipBeam 页面（点击页面任意位置）；
3. 按发送热键，或托盘菜单选「发送本机剪贴板 → 远程」；
4. 程序等待 200ms 让修饰键抬起，然后逐键敲入协议帧，页面收到后自动：
   校验 CRC → base32 解码 → 写入远程剪贴板 → 页面提示成功。

> 页面上的「暂停接收」复选框：当你需要在页面输入框里打字时勾选，
> 避免键盘状态机误收普通击键。

**紧急中止**：发送是逐键模拟，若发现焦点不对、按键触发了异常操作，立即：

- 按 **Esc**（中止热键，默认）；
- 或**再按一次发送热键**；
- 或托盘菜单选「取消」。

每敲一个字符前都会检查中止令牌，最坏停止延迟约等于一个键间隔（默认 3ms）。

### 接收（远程 → 宿主机）

1. 在远程页面「② 发送出站」区点 **「读取远程剪贴板并生成二维码」**；
   - 若浏览器拒绝 `navigator.clipboard.readText()`（权限策略限制），
     把文本粘贴到出现的文本框，点「用上方文本生成二维码」；
   - 「高级」可调整每帧字符数（默认 800）与每帧停留时长（默认 300ms）；
2. 二维码以全屏遮罩**固定居中循环播放**（页面滚动/缩放都不影响截屏区域），
   浮层上实时显示当前帧/总帧数；**点击浮层任意处即关闭并停止播放**
   （也可使用页面上的「■ 停止播放」按钮）；
3. 宿主机按接收热键（`Cmd+Shift+J`）或托盘选「截屏接收远程二维码」；
4. 程序每 250ms 对**所有显示器**逐屏截屏解码（多显示器无需指定屏幕，
   运行中热插拔也能自动适配），按 `(CRC, 总帧数)` 批次自动去重组包；
   播放中途换了一批数据会自动整批重来；
5. 收齐且 CRC 校验通过后写入本机剪贴板并弹通知；默认 120 秒超时。

中止方式同样有三种：**Esc**、再按一次接收热键、托盘取消。

> **进度窗口**（420×96 的悬浮面板）：任务启动时自动弹出，四行显示
> 「任务 + 状态 + 百分比 + ✕」「进度条」「已输出/速度/剩余/已用 或 结果标题」「脚本输出片段 或 结果说明」。
> 默认出现在**鼠标所在显示器**的右上角（避开菜单栏 / 任务栏），可以按住面板拖动，
> 拖过的位置在本次运行内会被记住；点右上角 **✕** 随时收起。
> 任务结束后 **5 秒**自动隐藏 —— 但只要你**碰过**它（鼠标移到上面、点一下、或拖一下，
> 都说明你在关心这次结果），它就**不会自动消失**，留着让你看清楚，由你手动点 ✕ 关闭。

### 托盘菜单

| 菜单项 | 说明 |
|---|---|
| 状态 | 当前空闲/忙（不可点击；进度显示在独立进度窗口） |
| 发送本机剪贴板 → 远程 | 触发一次协议 A 发送 |
| 截屏接收远程二维码 | 触发一次协议 B 接收 |
| 部署接收页到远程（键盘输入，约 3 分钟） | 协议 C，逐键敲自解压引导页 |
| 部署接收页（复制到宿主机剪贴板） | 协议 C，走远程桌面自带剪贴板 |
| 脚本编辑器… | 打开独立的脚本大窗口（编辑 + 运行 + Console 面板） |
| 设置… | 打开设置窗口 |
| 退出 ClipBeam | 退出常驻程序 |

任务运行期间，三个执行类菜单项自动禁用。

---

## 脚本引擎（JS/TS）

除了「发送本机剪贴板」这类固定动作，ClipBeam 还内置了一个**嵌入式脚本引擎**：
用 JavaScript / TypeScript 编排整个流程（读文件 → 计算摘要 → 压缩分片 → 逐键输出），
适合「把某个文件的多个分片依次敲进远程窗口」这类固定脚本化场景。

```js
// `$`（正式名 `ClipBeam`）是引擎挂上的能力命名空间；`sleep` 是引擎提供的全局（要 await）
const bytes = await $.read('~/data.bin')   // 读真实文件（只接受**绝对路径**）
const parts = $.chunks($.zstd(bytes), 1024) // 先压缩，再按 1 KiB 分片（两件事分开）
$.type_str(`共 ${parts.length} 片\n`, 10)  // 逐键打进当前焦点窗口
for (let i = 0; i < parts.length; i++) {
  if (await $.confirm(`发送第 ${i + 1} 片？`)) {
    await sleep(1000)                       // 全局 sleep，必须 await
    $.type_str(`${i}\n${$.base32_nopad(parts[i])}\n`, 10)
  }
}
```

### 三层结构

| 层 | 位置 | 职责 |
|---|---|---|
| 引擎（core） | `crates/script-engine/` | 跑 JS/TS（QuickJS + oxc 进程内转译）、`sleep` / `console` / `TextDecoder` / `TextEncoder` / 定时器 / `atob`·`btoa` / `performance` / `structuredClone`、**能力扩展机制**、**命名空间配置**（名字由使用方给） |
| 使用方能力集 | `crates/clipbeam-scripting/` | **决定命名空间叫什么**（`NAMESPACE` / `NAMESPACE_ALIAS`）；用扩展机制注入 ClipBeam 需要的能力：编解码（hex / base32 / base64）、摘要（md5 / crc32）、压缩（zstd / gzip / brotli / xz）、文件系统（`$.read` / `$.write` …）、宿主交互（`$.type_str` / `$.confirm`）；`ScriptHost` 定义；脚本目录与内置示例；终端宿主 |
| 应用 | `src-tauri/`（`scripting.rs` / `standby.rs` / `console_panel.rs` / `script_runner.rs`） | 接 Tauri 命令、Worker 任务、`Typer` 键盘输出、**系统原生确认框与文件授权框**、**惰性待命窗口**、**Console 面板缓冲** |

引擎本身**不认识**剪贴板 / 键盘 / Zstandard，**也不给能力命名空间起名字**：它建一个匿名对象，
名字（`ClipBeam` / `$`）由使用方在 `RuntimeOptions::namespace` 里配置 —— 引擎里没有一处业务名字。
能力再由使用方通过 `ScriptExtension` 注入（新增能力只要实现 `register` + `spec` 两个方法）。

命名空间挂上是**不可写、不可配置**的，所有扩展注册完后对象会被 `Object.freeze`：
脚本改不了能力（`$.md5 = null` 会直接报错）。`$` 由引擎提供，所以脚本里**不要**再写
`const $ = ClipBeam`（会报 `redeclaration of '$'`），直接用 `$` 即可。

### 能力一览

凡是接收二进制的能力都接受三种入参：`string`（按 UTF-8 编码）、`ArrayBuffer`、任何
`ArrayBufferView`（`Uint8Array` / `DataView` …，按 `byteOffset` + `byteLength` 取）。

| 能力 | 说明 |
|---|---|
| `$.bytes(data)` | 统一成 `ArrayBuffer`（字符串按 UTF-8） |
| `$.str(data, encoding?)` | 字节 → 文本（默认 `utf-8`，支持全部 WHATWG 标签） |
| `$.chunks(data, chunkSize?)` | 按 `chunkSize` 分片（默认 1024）：字符串按**码点**切返回 `string[]`，二进制按**字节**切返回 `ArrayBuffer[]` |
| `$.hex` / `$.hex_upper` / `$.hex_decode` | 十六进制编解码（解码大小写均可） |
| `$.base32` / `_nopad` / `_lower` / `_lower_nopad` | RFC4648 Base32：大写+填充 / 大写无填充 / 小写+填充 / 小写无填充 |
| `$.base32_decode` / `_nopad_decode` / `_lower_decode` / `_lower_nopad_decode` | 与上面一一对应；**严格**校验大小写与填充 |
| `$.base64` / `_nopad` / `_url` / `_url_nopad` + 对应 `*_decode` | 标准盘表与 URL-safe 盘表，各自带/不带填充 |
| `$.md5(data)` | 32 位小写十六进制 MD5 |
| `$.crc32(data, format?)` | CRC-32（大端）：`hex`（默认，8 位大写）/ `hex_lower` / `base32`（7 位）/ `base32_lower` / `base64`（6 位） |
| `$.zstd(data, level?)` | Zstandard 压缩（默认级别 3）；**只压缩不分片** |
| `$.gzip` / `$.gunzip` | gzip 压缩（默认 6）与解压 |
| `$.brotli` / `$.unbrotli` | Brotli 压缩（默认质量 5）与解压 |
| `$.lzma` / `$.unlzma` | xz（LZMA2）压缩与解压，产物兼容 `xz` / `7z` / `tar -J` |
| `$.read` / `$.read_text` | 读文件（字节 / 带编码的文本），只读不询问 |
| `$.exists` / `$.stat` / `$.list` | 存在性、元信息（大小 / 类型 / 修改时间）、目录条目 |
| `$.write` / `$.write_text` / `$.append` / `$.append_text` | 写 / 覆盖、追加（**会先询问**；父目录需已存在） |
| `$.mkdir` / `$.remove` / `$.rename` / `$.copy` | 建目录（递归）/ 删除（目录递归）/ 改名移动 / 复制（目录递归），**都会先询问** |
| `$.type_str(text, delayMs?)` | 把文本交给宿主输出：GUI 下逐个字符打进**当前焦点窗口**，命令行下打印到终端 |
| `$.confirm(message)` | 向用户提问：GUI 下弹**系统原生**确认框（是 / 否 / 取消），命令行下读 stdin；回答「是」为 `true`、「否」为 `false`、取消/中止时抛异常，**不设超时**（一直等用户回答） |
| `$.request_focus(hint?)` | 请求用户把焦点切到目标窗口并等待确认：GUI 下弹出待命窗口（`hint` 是显示给用户的提示，如「请点击远程记事本」），命令行下直接返回。脚本要在**中途**换一个输出目标时调用它 |
| `console.log/info/debug/warn/error` | 写到脚本窗口底部的 **Console 面板**（按等级着色）；命令行运行时打到终端 |
| `sleep(ms)`（**标准全局**） | 异步等待；等待期间被中止会立即返回。返回 Promise，**必须 `await`** |
| `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` | 定时器；**只属于本次运行**，脚本结束时统一清理 |
| `performance.now()` / `structuredClone(v)` | 单调时钟 / JSON 语义深拷贝 |
| `atob` / `btoa` | WHATWG 语义的 base64（只处理 Latin-1 字符串） |
| `TextDecoder` / `TextEncoder` | 支持全部 WHATWG 编码标签（`utf-8` / `gbk` / `gb18030` / `big5` / `shift_jis` …） |

### 文件系统的两条规则

1. **只接受绝对路径**。`~`（主目录）、`C:/x`、`C:\x`、git-bash 的 `/d/x`、Cygwin 的
   `/cygdrive/d/x` 都算绝对路径；相对路径直接报错 —— 脚本的工作目录取决于宿主怎么启动，
   放行相对路径等于埋雷。
2. **修改要问一次**。只读操作（`read` / `read_text` / `exists` / `stat` / `list`）不问；
   写、追加、建目录、删除、改名、复制每次都会弹一个三按钮系统框：
   `允许一次` / `本次运行内该目录都允许` / `拒绝`。「本次运行内都允许」记在**本次运行**里，
   运行结束即失效（下次运行重新问）。命令行下非交互（管道 / 重定向）时一律按**拒绝**处理。

脚本里可以直接使用**顶层 `await`**，不需要包 `async` IIFE。

### 运行方式

**GUI**：托盘菜单「脚本编辑器…」，或主窗口侧边栏 / 仪表盘的「脚本」按钮
→ 打开**独立的脚本大窗口**（1100×760，可缩放；左侧是脚本列表，右侧是编辑器 + Console）
→ 选/新建脚本 → 点「运行」。

脚本是一个独立窗口而不是主窗口里的一个页面，原因有三：编辑器需要足够的空间；
它可以和主窗口/进度窗口并排摆放；关闭主窗口（托盘模式）后脚本窗口仍可独立使用。

> **Dock 图标**：平时不占 Dock（托盘常驻），但**脚本窗口可见期间会显示 Dock 图标**，
> 这样你可以把编辑器丢到另一块屏幕、用 Cmd+Tab 切回它；脚本窗口一关，Dock 图标随之消失。
> 实现见 `src-tauri/src/lib.rs` 的 `sync_dock_icon`（用 Tauri 的 `set_dock_visibility`，
> 它内部对 macOS 的进程类型切换做了防抖）。

**待命窗口只在首次 `$.type_str` 之前弹**：脚本不一定注入键盘事件（例如只读文件、算摘要的脚本），
所以待命窗口是惰性的 ——

* 脚本执行到**第一次** `$.type_str` 时，才弹出待命窗口并等用户点击目标窗口；
* 纯计算脚本（从不输出）**不会有任何待命提示**，直接跑完；
* 同一次运行里只提示一次，后续 `$.type_str` 不再重复打扰；
* 脚本在那之前可能已经读文件、算摘要、打印 `console` 日志 —— 这些都照常执行。

待命窗口有 10 秒倒计时，超时即取消任务。进度窗口会显示「已敲入 N/M 字符 + 当前输出片段」，
按 **Esc**（或点「中止」）随时停下。

#### 焦点锁定与重新确认

一次待命成功 = **锁定**一个目标窗口。之后每次输出前都会校验这个锁（打字过程中每 250ms 一次）：

* **焦点没变** → 直接继续打，不再打扰；
* **焦点换了窗口** → 立刻停手，稍等 50ms 复查一次（吸收系统通知、确认框这类瞬时抖动），
  确认真的换了就**自动重新弹出待命窗口**；用户重新点好目标窗口后，输出**从断点继续**
  （已输入的字符不会重复）；
* 脚本要用**中途**换一个输出目标（例如先打进浏览器、再打进记事本），显式调用
  `$.request_focus("请点击远程记事本")`：会立刻重开一轮待命并显示这句提示（焦点没变也会弹）；
* 待命窗口上的**「暂停检测」**用于「要点好几次才能把焦点切到位」的场景：暂停期间点击任何窗口
  都不会开始输出，倒计时与 10 秒超时**一起挂起**（想切多久都行，不设上限；「取消」与 Esc 始终可用）；
  点**「恢复检测」**回到原来的逻辑（倒计时重新给满），因为按钮把焦点交回了待命窗口，
  所以还需要再点一次目标窗口才会确认。

焦点识别是**窗口级**的：macOS 走系统级 AX API（复用键盘注入本来就需要的那项「辅助功能」权限；拿不到
AX 时回退成「前台应用」级别），Windows 走 `GetForegroundWindow`；两者都不看窗口标题，所以
浏览器切标签、文档改名**不会**被误判成换窗口。其它平台（Linux）不检测，行为与以前一致。

**命令行**：`clipbeam script <文件>`，`$.type_str` 写终端、`$.confirm` 与文件授权读 stdin
（不需要待命窗口：终端的焦点就是当前窗口）；加 `--raw` 时不走逐字打字节奏，
适合把输出重定向到文件或管道。

### 脚本目录

```
macOS   ~/Library/Application Support/ClipBeam/scripts/
Windows %APPDATA%\ClipBeam\scripts\
Linux   ~/.config/ClipBeam/scripts/
```

首次启动会写入两个内置示例（`01-quick-start.js`、`02-ts-demo.ts`），**已存在的文件不会被覆盖** ——
你可以放心修改示例，下次启动不会还原。GUI 里可以新建 / 编辑 / 保存 / 删除。
文件能力不受脚本目录限制，可以访问任意**绝对路径**（第一次修改某个目录里的东西时会向你确认）。

### Console 面板

脚本窗口底部是 Console 面板，脚本里的 `console.log/info/debug/warn/error` 会出现在这里
（GUI 不写标准输出——用户看不到；只有命令行运行时才打到终端）：

* 按等级着色（error 红、warn 琥珀、debug 灰），每行带时间；
* 面板里还有运行状态行：`▶ 运行 xxx`、`✓ 完成(输出 N 个字符,用时 Xms)`，
  脚本报错时把引擎的错误原文（含文件名与行列）写成 error 行，中止时写 warn 行；
* 每次运行前清空 —— 面板永远只属于**这一次**运行，排查时不会被上一轮输出干扰；
* 上限 1000 行（狂打印也只保留最后 1000 行），可按等级过滤、可手动清空；
* **高度可调**：拖住面板上方的分隔条改高度（方向键也能微调，`Tab` 聚焦分隔条后按 ↑/↓）；
  高度记在 localStorage，关窗重开还是这个高度；
* **可折叠 / 可最大化**：标题栏右侧的 ⌄ 折叠成一条标题栏，⤢ 让 Console 占满整个编辑区
  （编辑器只是被压到 0 高、不会卸载，所以撤销历史和光标位置都还在；再点一次还原）；
* **自动跟随最新输出**：默认贴底；手动往上翻看历史时暂停跟随，滚回底部自动恢复
  （清空、切换等级过滤也会重新贴底）；
* 日志存在后端，关闭再打开脚本窗口时面板会自动恢复。

### 脚本窗口里的确认框

脚本窗口有三处需要用户确认的危险操作（切换脚本 / 新建 / 删除会丢改动），以及「有未保存修改时
是否先保存再运行」。它们统一走 `@tauri-apps/plugin-dialog` 的 `ask()`，也就是**系统原生 Yes/No 弹框**
—— 与脚本里 `$.confirm` 的观感一致。

> 注意：这里不能用 `window.confirm()`。Tauri 的 webview 会把它接管成插件调用，
> 而权限里只放开了消息类弹框，直接调用会抛 `dialog.confirm not allowed. Command not found`。
> 另外 `dialog:default` 只包含 open/save/message，**不含**消息弹框，必须在
> `src-tauri/capabilities/default.json` 里显式写 `dialog:allow-ask`（或 `allow-message`）。

### 编辑器能力

脚本窗口里的编辑器用 CodeMirror 6（高亮/缩进来自 lezer），**语义能力全部来自一个进程内的
TypeScript 语言服务** —— 补全、悬停、参数信息、诊断问的是同一个服务、同一份虚拟文件表，
所以不会出现「诊断说没问题、悬停说旧类型」这种不一致：

| 能力 | 说明 |
|---|---|
| 补全 | 按上下文给候选：`ClipBeam.` / `$.` 列出全部能力（带签名与 JSDoc）、变量后列出它类型的成员、标识符位置列出作用域内的变量；候选项的详情面板显示签名 + 说明 |
| 悬停 | 悬停任何标识符/表达式显示类型签名 + JSDoc，例如 `(method) ClipBeam.md5(data: BinaryInput): string`；说明会被清理 Markdown 并截断（最长 260 字），tooltip 限宽 460px、限高 240px，避免盖住代码 |
| 参数信息 | 在调用括号里显示签名并高亮当前参数（编辑器顶部一条，例如 `zstd(data: ArrayBuffer, chunkSize?: number)` + `参数 1/2`） |
| 诊断 | 类型不匹配、拼错方法名、用了运行时不存在的 API 都会标红/标黄（`TS2345:` 这类错误码也一并显示） |
| 编辑体验 | 括号匹配/自动闭合、自动缩进、搜索、`Cmd/Ctrl+Enter` 运行、`Cmd/Ctrl+S` 保存（完整快捷键见脚本窗口右上角的 ⌨ 面板） |

#### 快捷键

脚本窗口右上角有一个 ⌨ 按钮，点开就是完整清单（内容由 `src/lib/clipbeam-script/shortcuts.ts`
提供，与编辑器真实绑定共用同一份键位串，不会漂移）；运行/保存按钮的 tooltip 也会带上快捷键。
macOS 显示 ⌘/⇧/⌥/⌃，Windows/Linux 显示 Ctrl/Shift/Alt：

| 分组 | 动作 | macOS | Windows / Linux |
|---|---|---|---|
| 运行与保存 | 运行脚本 | `⌘⏎` | `Ctrl+Enter` |
| 运行与保存 | 保存脚本 | `⌘S` | `Ctrl+S` |
| 编辑 | 撤销 / 重做 | `⌘Z` / `⌘⇧Z` | `Ctrl+Z` / `Ctrl+Y` |
| 编辑 | 注释 / 取消注释 | `⌘/` | `Ctrl+/` |
| 编辑 | 增加 / 减少缩进 | `⌘]` / `⌘[` | `Ctrl+]` / `Ctrl+[` |
| 编辑 | 整行上移 / 下移 | `⌥↑` / `⌥↓` | `Alt+↑` / `Alt+↓` |
| 编辑 | 删除整行 | `⇧⌘K` | `Shift+Ctrl+K` |
| 编辑 | 全选 | `⌘A` | `Ctrl+A` |
| 查找 | 查找 | `⌘F` | `Ctrl+F` |
| 查找 | 下一个 / 上一个匹配 | `⌘G` / `⌘⇧G` | `Ctrl+G` / `Ctrl+Shift+G` |
| 查找 | 选中下一个相同词 | `⌘D` | `Ctrl+D` |
| 查找 | 跳到行 | `⌥⌘G` | `Ctrl+Alt+G` |
| 补全与提示 | 触发补全 | `` ⌥` ``（Ctrl-Space 在 mac 常被输入法占用） | `Ctrl+Space` |
| 补全与提示 | 接受候选 / 关闭面板 | `⏎` / `Esc` | `Enter` / `Esc` |

> 面板有意只列「常用」部分：方向键、Home/End、多光标（`⌘⌥↑/↓`）、复制行（`⇧⌥↑/↓`）
> 等 CodeMirror 默认键位没有一一列出。

编辑器里的类型来自两个声明文件（`crates/script-engine/src/spec/engine.d.ts` 与
`crates/clipbeam-scripting/src/spec/clipbeam.d.ts`），它们同时被
`pnpm typecheck:scripts` 使用，因此**编辑器里的断言与命令行检查一致**：

```bash
pnpm typecheck:scripts    # 用 tsc 检查内置示例（crates/clipbeam-scripting/seed/*.ts）与两份声明
pnpm test:editor          # 编辑器语义功能与快捷键格式化的单测
```

> TypeScript 语言服务是**懒加载**的（体积不小）：首次打开脚本页时后台预热，
> 就绪前补全退回「能力清单」版本、悬停不弹框；加载失败也只是退化，不影响编辑与运行。

### 窗口置顶

脚本窗口工具栏有一个「置顶」按钮：打开后窗口始终压在其他窗口之上（`alwaysOnTop`），
方便把编辑器和目标窗口并排摆放。状态不持久化 —— 每次打开脚本窗口回到默认（不置顶）。

### TypeScript 支持范围

`.ts` / `.mts` / `.cts` 会被自动转译（oxc，进程内，**不需要 Node/tsc**）：

| 支持 | 说明 |
|---|---|
| 类型注解 / `interface` / `type` / `as` / 泛型 | 直接剥掉 |
| `enum` / `namespace` | 转成运行时对象 |
| 顶层 `await` | 脚本按 module 解析，因此允许 |
| 现代 JS 语法 | 原样保留，不做 ES 降级 |

| 不支持 | 说明 |
|---|---|
| JSX / TSX | 没有 React 运行时，`.tsx` 会明确报错 |
| 装饰器等需要 helper 的语法 | 明确报错，而不是生成跑不起来的代码 |
| ESM 的 `import` / `export` 值导入 | 运行时是「全局 + async eval」，`import type` 会被移除，值导入暂不支持 |
| source map | 运行期报错的**行列号指向转译后的 JS**；语法错误不受影响（诊断直接给 `.ts` 的位置） |

### 安全说明

脚本拥有**与 ClipBeam 同等的本机权限**：`$.read` 可以读任意**绝对路径**、`$.type_str` 会把内容
敲进当前焦点窗口。读操作没有沙箱也没有提示；写 / 删 / 改名 / 复制 / 建目录每次都会弹系统框
确认（可以放行「本次运行内该目录」）。请只运行你信任的脚本。

---

## 配置与设置

托盘菜单「设置…」打开图形设置窗口，所有项实时校验，非法时「保存」禁用：

| 设置项 | 默认值（macOS / Windows） | 合法范围 |
|---|---|---|
| 发送热键 | `Cmd+Shift+K` / `Ctrl+Shift+K` | global-hotkey 语法，如 `Cmd+Shift+K`、`Esc` |
| 接收热键 | `Cmd+Shift+J` / `Ctrl+Shift+J` | 同上 |
| 中止热键 | `Esc` | 同上，三个热键不得重复 |
| 键延迟 | 3 ms/键 | 0–100 ms |
| settle 等待 | 200 ms | 0–5000 ms（热键后等修饰键抬起再开始打字） |
| 接收超时 | 120 秒 | 5–3600 秒 |
| 文本上限 | 256 KB | 1–10240 KB |
| zstd 压缩 | 开 | 开/关（仅协议 A，关闭后用未压缩帧便于对比耗时） |

热键支持点击「捕获」后直接按下组合键录入。保存后立即重注册全局热键并通知。

配置文件位置（JSON，可手工编辑；缺字段自动回退默认值）：

- macOS：`~/Library/Application Support/ClipBeam/config.json`
- Windows：`%APPDATA%\ClipBeam\config.json`
- Linux：`~/.config/ClipBeam/config.json`

---

## 命令行子命令

不带参数启动为托盘常驻模式；以下子命令主要用于联调/脚本化：

```bash
clipbeam send-once      # 立即发送一次本机剪贴板（0.2s 后开始，Ctrl-C 强杀）
clipbeam recv-once      # 立即截屏接收，stderr 打印 got/total 进度
clipbeam deploy-type   # 逐键敲入自解压接收页到当前焦点窗口（先切到远程记事本）
clipbeam deploy-copy   # 把自解压接收页复制到宿主机剪贴板
clipbeam script <文件>          # 在终端里运行脚本（.js / .ts，TS 在进程内转译）
clipbeam script <文件> --raw    # 同上，但按整段写出（不逐字模拟打字节奏）
```

---

## 构建

要求 Rust stable（edition 2021）+ Node.js 18+ + pnpm。

pnpm workspace + cargo workspace 单仓多包布局：

- 仓库根：Tauri 主界面（Vue 3 + Vite + TS + shadcn-vue）；`Cargo.toml` 定义 cargo workspace
- `src-tauri/`：Rust crate + Tauri 配置（`clipbeam_lib` + `clipbeam` 二进制）
- `crates/script-engine/`：可嵌入的脚本引擎（QuickJS + oxc），只提供标准全局、扩展机制与命名空间配置
- `crates/clipbeam-scripting/`：使用方能力集（编解码 / 摘要 / 压缩 / 文件系统 / 宿主交互）+ `ScriptHost` + 脚本目录 + 终端宿主
- `scripts/`：示例脚本（供命令行运行；内置示例的真源在 `crates/clipbeam-scripting/seed/`）
- `packages/shared/`：两个 client 共享的协议逻辑（CB1 分帧、base32/CRC、zstd、剪贴板、接收状态机）
- `packages/client/`：浏览器端 Vue 3 收发页（shadcn-vue + Tailwind v4，开发预览用）
- `packages/client-vanilla/`：零依赖单文件接收页，构建产物被 Rust `include_str!` 内嵌用于部署

```bash
# 安装全部 workspace 依赖（仓库根执行一次即可）
pnpm install

# 单独构建远程接收页（产物 packages/client-vanilla/dist/index.html）
pnpm build:web

# Debug 开发模式（Tauri 自动启动 Vite dev server + Rust 编译 + 托盘应用；
# cargo 构建时会按需自动构建 client-vanilla，可用 CLIPBEAM_SKIP_WEB_BUILD=1 跳过）
cd src-tauri && cargo tauri dev

# Release 打包（Tauri 自动：前端构建 + Rust release + bundle）
cd src-tauri && cargo tauri build
```

### macOS：打包成可双击的 .app（推荐）

直接双击裸二进制会打开一个终端窗口，关掉终端程序即退出。用打包脚本生成
标准应用包，启动无终端、平时不占 Dock（仅菜单栏图标；脚本窗口打开期间会临时出现在 Dock），
与普通 Mac 应用一致：

```bash
scripts/package-macos.sh              # 生成 src-tauri/target/release/bundle/macos/ClipBeam.app
scripts/package-macos.sh --install    # 额外安装到 /Applications（Spotlight/Launchpad 可启动）
```

脚本自动完成：`pnpm install`（如需）→ `cargo tauri build`（Tauri 2 接管前端
构建、Rust release 编译、`Contents/` 与 `Info.plist` 组装、`.icns` 生成）→
ad-hoc 代码签名 → LaunchServices 注册。之后在 Finder 双击或 `open ClipBeam.app`
即可启动，程序由 launchd 托管，关闭终端/SSH 断开都不影响运行。

> 首次启动需在「系统设置 → 隐私与安全性」授予辅助功能、屏幕录制、通知权限；
> ad-hoc 签名的应用与之前裸二进制是不同身份，权限需要重新授权一次。

平台依赖：

- **macOS**：Xcode Command Line Tools；键盘模拟走 CoreGraphics
  （`CGEventKeyboardSetUnicodeString`，任意线程可用），通知走 Notification Center；
- **Windows**：MSVC 工具链；键盘模拟走 enigo，通知走 Toast（winrt-notification）。

已验证 Windows 目标交叉编译检查：

```bash
rustup target add x86_64-pc-windows-msvc
cd src-tauri && cargo check --target x86_64-pc-windows-msvc
```

托盘图标由 [src-tauri/build.rs](src-tauri/build.rs) 在编译期代码生成（Bresenham K 字
光束 + 青色光点），无需额外二进制资源入库。

---

## 测试

```bash
# 整个 workspace（引擎、能力集、Tauri 应用）
cargo test --workspace

# 新增的两个 crate 是 clippy 干净的；src-tauri / build.rs 里有一批历史告警，
# 所以这里只对它们跑 clippy（要做全量门禁需先清掉历史告警）
cargo clippy -p script-engine -p clipbeam-scripting --all-targets

# 只跑脚本引擎与能力集（第一次编译 QuickJS 的 C 代码较慢，之后很快）
cargo test -p script-engine
cargo test -p clipbeam-scripting

# Tauri 侧的协议层单元测试与跨实现 E2E
cd src-tauri
cargo test
cargo run --example qr_e2e   # JS(qrcode-generator) 产出帧 → Rust(rqrr) 解码的跨实现 E2E

# 前端：构建 + 脚本类型检查
pnpm build
pnpm typecheck:scripts
pnpm lint
```

> `cargo test -p script-engine` 覆盖：`sleep` / 取消 / 定时器 / `atob`·`btoa` /
> `structuredClone`、`TextDecoder` 的 GBK/BOM/fatal、扩展机制的注册/重名/声明一致性、
> TS 转译与端到端；
> `cargo test -p clipbeam-scripting` 覆盖：入参多态、编码往返与严格解码、md5/crc32 与
> Rust 侧独立实现对齐、压缩往返（gzip 产物再由 Rust 解一遍）、文件读写与**授权策略**
> （拒绝 / 本次运行放行 / 跨运行失效）、`type_str`/`confirm` 的宿主语义、脚本目录与内置示例。
> 命名空间的配置与校验（默认不挂、不可写绑定、冻结对象、非法名字/冲突名报错）由
> `cargo test -p script-engine --test namespace` 覆盖。

### 在浏览器里调试前端（Chrome）

Tauri 的 webview 才会注入 `window.__TAURI_INTERNALS__`，所以直接开 Vite 会白屏 ——
`src/App.vue` 在 setup 里读窗口 label，拿不到就 `TypeError`。现在 dev 模式下会自动装一层
**Tauri mock**（`src/dev/tauri-mock.ts`，用官方 `@tauri-apps/api/mocks`）：

```bash
pnpm dev
# 然后 Chrome 打开其中之一（路由是 hash 模式，与 Tauri 窗口 URL 一致）
http://localhost:5173/            # 主窗口（总览）
http://localhost:5173/#/scripting # 脚本窗口
http://localhost:5173/#/settings  # 设置
http://localhost:5173/#/progress  # 进度（裸布局）
http://localhost:5173/#/standby   # 待命（裸布局）
```

* 窗口 label 默认按路由推断，也可以用 `?window=<main|scripting|progress|standby>` 显式指定；
* 25 条自定义命令走内存夹具（脚本清单/读写、配置、能力清单、Console 缓冲…），
  `start_script` 还会模拟一次运行（推 `worker-*` 事件 + Console 行）；
* `plugin:window|*` 返回合理默认，`plugin:dialog|ask` 一律按"用户取消"返回 `false`；
* 后端事件不会自己发生，用 `window.__CLIPBEAM_DEV__` 手动造：

```js
__CLIPBEAM_DEV__.emit('worker-started', 'send')       // 状态徽标变成"发送"
__CLIPBEAM_DEV__.consoleLine('error', '手造一行错误')  // Console 面板立刻出现红行
await __CLIPBEAM_DEV__.runScript('01-quick-start.js') // 演练一次完整运行
```

> **不覆盖**：真实键盘注入、剪贴板、文件系统、Rust worker 与二维码识别、系统原生弹框 ——
> 这些仍然要真机验证。mock 有 `import.meta.env.DEV` + `__TAURI_INTERNALS__` 存在性双重守卫：
> `pnpm tauri dev` 与生产构建都不受影响（已验证生产产物里不含 mock 代码）。
>
> VS Code 里可以直接用 `Chrome: 前端调试（Vite + Tauri mock）` 这条 launch 配置（F5），
> 断点能命中 `src/**`。

---

## 协议规范

### 协议 A：键盘帧

```
0 clipbeam{ver} 0 <payload> 0 <digest> 1
```

- `clipbeam` 为 8 字符公共前缀，第 9 字符 `ver` 为协议版本：
  - `1` = v1，未压缩；
  - `2` = v2，payload 为 zstd 压缩后的 base32（压缩等级 3；压缩后反而更大时自动回退 v1）；
- `0` 为字段分隔、末尾 `1` 为整帧结束，均为数字行键，无需 Shift；
- `payload`：v1 = 文本 UTF-8 字节的小写无填充 base32；v2 = zstd 压缩流的小写 base32；
- `digest`：`CRC32(payload 字节)` 的 4 字节大端再小写 base32（7 字符）；
- 接收页先匹配 `clipbeam` 8 字符前缀再取版本号，同时兼容 v1/v2；
  v2 帧在 base32 解码后调用内联 fzstd 库解压；半截帧 10 秒无后续自动复位。

### 协议 B：二维码帧

```
CB1.<total>.<index>.<digest>.<data>
```

- `total`：本批总帧数（≥1）；`index`：当前帧序号（0 起，`< total`）；
- `digest`：**全部帧 data 按序拼接后**的 CRC32，4 字节大端再大写 base32（7 字符）；
- `data`：文本 UTF-8 → 大写无填充 base32 → 等长分块（默认每块 800 字符）；
- 接收端以 `(digest, total)` 标识批次，序号去重；批次键变化即清空重来；
  收齐后拼接 → 先校验 CRC（不解码先校验）→ base32 解码 → 写剪贴板。

### 协议 C：自解压引导页

- 模板为全 ASCII、单行 HTML/JS，载荷仅 `[A-Z2-7]`，可安全放入双引号 JS 字符串；
- 载荷 = 完整接收页 UTF-8 的大写无填充 base32（编译期 `include_str!` 内嵌
  `packages/client-vanilla/dist/index.html`）；
- 远程浏览器打开引导页后，内联脚本做 base32 解码并 `document.write` 替换自身，
  得到与 client-vanilla 构建产物完全一致的接收页。

---

## 限制与注意事项

- **仅支持文本剪贴板**：图片、富文本等非文本内容会被拒绝，请先转成纯文本；
- **键盘通道耗时与长度成正比**：base32 约 1.6 倍膨胀，按默认 3ms/键估算，
  256 KB 文本约需 20 分钟；开启 zstd 压缩（默认）可显著缩短重复/可压缩文本的耗时，
  但对随机短文本无收益（会自动回退未压缩帧）；大数据量优先考虑能否走协议 C 之外的文件通道；
- **焦点必须在远程页面**：发送前程序不切换任何焦点，请先点击远程页面；
- **键盘布局无关性**：协议 A 只使用小写字母与数字行 `0`/`1`，
  不依赖远程当前输入法（中文输入法下仍可正常接收）；
- **浏览器剪贴板权限**：`navigator.clipboard.readText()` 需要用户手势触发
  且受页面权限策略限制，被拒绝时使用页面上的粘贴备选框；
- **二维码可见性**：多屏环境下会截取所有显示器，二维码出现在任意一块屏幕上
  均可被识别，但该区域需完整可见、未被遮挡/最小化，每屏会降采样到 1600px 宽再解码；
- **安全**：两条通道都没有也不需要网络连接；CRC32 只防传输错误，不提供机密性，
  屏幕/键盘内容在本地处理，不上传任何数据；
- **脚本与本机同权限**：`$.read` 能读任意绝对路径、`$.type_str` 会把内容敲进当前焦点窗口；
  读操作没有沙箱，修改操作会弹框确认（但确认框本身不是安全边界），只运行可信脚本；
- **`$.confirm` 会一直等**：系统确认框不设超时，弹框期间 Worker 保持忙（其他任务无法启动）。
  此时按 Esc 只会**终止脚本**，系统弹框仍留在屏幕上，需要手动点掉；
- **弹框时主窗口会临时显形**：托盘模式下主窗口是隐藏的，而系统弹框需要应用处于激活状态，
  因此确认期间主窗口会短暂显示并获得焦点，回答后恢复隐藏。

## 技术栈

| 层 | 依赖 |
|---|---|
| 应用框架 | Tauri 2（tray-icon feature）、tauri-plugin-global-shortcut、tauri-plugin-clipboard-manager、tauri-plugin-notification |
| 前端 | Vue 3 + Vite + TypeScript + Tailwind CSS v4 + shadcn-vue（reka-ui）+ Pinia + vue-router |
| 异步运行时 | tokio（worker 任务用 spawn_blocking 执行同步业务调用） |
| 键盘模拟 | CoreGraphics（macOS）、enigo（Windows/Linux） |
| 剪贴板 | arboard |
| 截屏 / 二维码 | xcap、rqrr、image |
| 编码 / 压缩 / 配置 / CLI | data-encoding、zstd（协议 A 压缩）、serde/serde_json、dirs、clap |
| 通知 | mac-notification-sys（macOS）、winrt-notification（Windows） |
| 脚本引擎 | QuickJS（rquickjs 0.14）、TypeScript → JavaScript 转译（oxc 0.150，进程内，不需要 Node） |
| 脚本编辑器 | CodeMirror 6（高亮 + `$` 补全 + TypeScript 编译器 API 做类型诊断） |
| 远程页 | 原生 JS 单文件，内联 qrcode-generator + fzstd（zstd 解压），零运行时依赖（不参与构建） |
