/* global $, console, sleep */
// ClipBeam 脚本示例（TypeScript）：压缩 → base32_lower_nopad 编码 → 分片 → 逐片以 heredoc
// 敲进远程终端，并把接收端脚本 restore.sh 自己先敲过去。
//
// 只有键盘一条通道时，把一个文件送到远端的最小闭环是：压小 → 编码成纯 ASCII →
// 切片 → 每片敲一条 `cat <<'EOF' > xx.pNNNN` → 远端拼接、解码、解压、逐层校验。
// 本示例还会把**接收端脚本自己**先敲过去（只 base32 编码，不压缩、不分片），
// 远端不必再手抄一遍脚本。
//
// 运行方式（二选一）：
//   * GUI：脚本页打开本文件，点「运行」，再点击远程终端窗口；
//   * 命令行：cargo run -p clipbeam -- script crates/clipbeam-scripting/seed/03-pack-and-shard.ts --raw
//
// 用法：改下面「可调参数」几个常量，然后跑起来。远端会依次收到：
//
//   cat <<'EOF' > restore.sh.b32
//   mfrggzdfmztwq2lk…（restore.sh 正文的 base32，按 1024 字符折行）
//   EOF
//   cat <<'EOF' > clipbeam-quick-start.txt.meta
//   parts=8
//   chars=8071
//   b32_md5=2b0d…（整段 base32 文本的 md5）
//   raw_bytes=5021
//   raw_md5=6f1c…（原文件字节的 md5）
//   EOF
//   cat <<'EOF' > clipbeam-quick-start.txt.gz.b32.p0001
//   mfrggzdfmztwq2lk…（1024 个 base32 小写字符，单行）
//   EOF
//   …（p0002 … p0008）
//   # 收尾：解出 restore.sh、语法自检、跑还原
//   tr -d '[:space:]' < restore.sh.b32 | tr a-z A-Z | base32 -d > restore.sh
//   bash -n restore.sh
//   bash restore.sh clipbeam-quick-start.txt
//
// 三条容易被忽略的规则（与 01-quick-start.js 一致）：
//   1. 文件路径必须是**绝对路径**：`~` 展开成主目录，相对路径直接报错；
//   2. 只读操作不会询问；本示例只在示例文件不存在时写一次，所以最多弹一次文件授权框；
//   3. `$.type_str` 之前宿主会弹待命窗口让你点好目标窗口 —— 本示例不自己 `$.confirm`，
//      点下「运行」就会开始往当前焦点窗口敲字。
//
// 为什么这样选（都不是随意定的）：
//   * 压缩用 gzip 而不是 zstd/xz：远端只要有 `gzip -dc` 就能解，zstd/xz 很多机器没装；
//   * 编码用 base32 小写无填充：盘表只有 a-z2-7，连 `=` 都不会出现 —— 密文里
//     **不可能出现大写 `EOF`**，heredoc 定界符天然不会被内容撞破；而且全是无需按
//     Shift 的键，打字最不容易出错。代价是末尾几个比特靠补零凑整，接收端两种解码器
//     都认（GNU `base32 -d` 直接吃无填充，python3 那条兜底自己补 `=`）；
//     想换成带填充的变体，把这两个名字换成不带 `_nopad` 的一对即可；
//   * 数据负载按 1024 字符分片（8 的倍数：每片正好整数个 base32 量子），便于分批；
//     还原脚本**不分片** —— 它只有几 KB，一个 heredoc 更简单；正文按 1024 字符折行只是
//     躲开终端对单行长度的限制，拼接时空白会被去掉；
//   * 还原脚本正文由 `getRestoreScript()` 返回，正文里不写 `${}`、反引号、反斜杠，
//     于是一个转义都不用，并有测试钉住「远端解出来的 == 这里的正文」。
//
// 键盘通道的耗时正比于字符数：本示例先算给你看再开敲。
// base32 约 1.6 倍膨胀、gzip 对文本常有 2~5 倍收益，数据负载大致打个平手或更小。

// ─── 可调参数 ────────────────────────────────────────────────────────────────

// 要发送的文件（绝对路径）。不存在时会先写一个示例文件，已存在的文件**不会被覆盖**；
// 也可以直接指向任意已有文件 —— 本示例只读它
const srcPath: string = '~/clipbeam-quick-start.txt';
// 每片多少 base32 字符（8 的倍数最整齐；片数上限 9999，超过请调大本值）
const chunkSize: number = 1024;
// 每个字符之间的键盘延迟（毫秒）：6 约等于 166 字符/秒，调大更稳、更慢
const delay: number = 6;
// 每片之间额外等一会儿，给远端 shell 留出处理 heredoc 的余量
const gapMs: number = 120;

// ─── 远端文件名 ──────────────────────────────────────────────────────────────

// 远端文件名一律从**源文件名**派生，所以跑完一眼就能看出这批分片是哪个文件的。
//
// 用 `$.basename`：它**同时按 `/` 与 `\` 切**，两端行为一致。
// 不要写成 `srcPath.split("/").pop()` —— Windows 路径里一个 `/` 都没有，
// 那会把**整条路径**当成文件名（`C:\Users\…\sample.bin`），于是敲给远端的是
// `cat <<'EOF' > C:\Users\…\sample.bin.meta`：在远端的 bash 里毫无意义，
// 整个还原流程都是坏的。这个坑在 Unix 上不会发作（路径自带 `/`），只会在 Windows 上炸。
const base: string = $.basename(srcPath) || "payload";
// 还原脚本在远端的三个名字：base32 传输体、解码后的脚本本体、元信息
const restoreB32Name: string = `restore.sh.b32`;
const restorePath: string = "restore.sh";
const metaName: string = `${base}.meta`;
// 负载经过 gzip → base32，文件名跟着编码步骤一路加后缀，出问题时不用猜是哪一步的产物
const packedName: string = `${base}.gz.b32`;

// ─── 1. 示例文件（只在不存在时写）────────────────────────────────────────────

if (!(await $.exists(srcPath)))
	await $.write_text(srcPath, "ClipBeam 脚本引擎示例\n中文与 emoji 🎯\n");

// ─── 2. 读文件 → 压缩 ────────────────────────────────────────────────────────

const bytes: ArrayBuffer = await $.read(srcPath);
const packed: ArrayBuffer = $.gzip(bytes);

// ─── 3. base32 编码（小写、无填充）+ 往返自检 ────────────────────────────────

const text: string = $.base32_lower_nopad(packed);

// 自检：解回来必须与压缩后的字节一致。编码能力用错（比如换成带填充的变体、
// 或误用大写版解码器）在这里就会抛错，而不是等远端还原失败才发现
if ($.md5($.base32_lower_nopad_decode(text)) !== $.md5(packed))
	throw new Error("base32 往返自检失败：编码/解码结果与压缩字节不一致");

// ─── 4. 分片 ─────────────────────────────────────────────────────────────────

// 每片正好落在 base32 的量子边界上（8 字符 = 5 字节），末尾也不会有 `=` 需要对齐
const parts: string[] = $.chunks(text, chunkSize);
if (parts.length > 9999)
	throw new Error(
		`分片数 ${parts.length} 超过 9999（编号只有 4 位）：请把 chunkSize 调大`,
	);

const pad4 = (n: number): string => String(n).padStart(4, "0");
const heredoc = (name: string, body: string): string =>
	`cat <<'EOF' > ${name}\n${body}\nEOF\n`;

// 接收端脚本靠这份元信息做校验：片数、base32 字符数、两层 md5。
// 用 `\n` 而不是 `\r\n`：远端 shell 会把 `\r` 当成普通字符写进文件
const meta: string = [
	`parts=${parts.length}`,
	`chars=${text.length}`,
	`b32_md5=${$.md5(text)}`,
	`raw_bytes=${bytes.byteLength}`,
	`raw_md5=${$.md5(bytes)}`,
].join("\n");

// ─── 5. 还原脚本：只 base32 编码（不压缩、不分片），一个 heredoc 敲过去 ──────

const restoreScript: string = getRestoreScript();
const restoreB32: string = $.base32_lower(restoreScript);
// 按 1024 字符折行：还是一个 heredoc、一个文件，不是分片；解码时空白会被去掉
const restoreHeredoc: string = heredoc(
	restoreB32Name,
	$.chunks(restoreB32, 1024).join("\n"),
);

// ─── 6. 拼出要敲的每一段，并预估耗时（字符数 × 每键延迟）────────────────────

interface TypeStep {
	/** 给 console 看的说明，例如「第 3/8 片」。 */
	label: string;
	/** 真正交给 `$.type_str` 的文本。 */
	text: string;
}

const steps: TypeStep[] = [
	{ label: `还原脚本 ${restoreB32Name}`, text: restoreHeredoc },
	{ label: `元信息 ${metaName}`, text: heredoc(metaName, meta) },
];
for (let i = 0; i < parts.length; i++) {
	const name: string = `${packedName}.p${pad4(i + 1)}`;
	steps.push({
		label: `${name}（第 ${i + 1}/${parts.length} 片）`,
		text: heredoc(name, parts[i]),
	});
}

// 收尾：解出 restore.sh、语法自检、跑还原。远端没有 GNU base32 时，把第一行换成
// console.log 里给出的那条 python3 命令（效果一样）
steps.push({
	label: "收尾：还原并执行",
	text: `tr -d '[:space:]' < ${restoreB32Name} | tr a-z A-Z | base32 -d > ${restorePath}
bash -n ${restorePath}
bash ${restorePath} ${base}\n`,
});

const totalChars: number = steps.reduce((sum, step) => sum + step.text.length, 0);
const seconds: number = Math.floor((totalChars * delay) / 1000);
const formatTime = (totalSeconds: number): string => {
	const h: number = Math.floor(totalSeconds / 3600);
	const m: number = Math.floor(totalSeconds / 60) % 60;
	const s: number = totalSeconds % 60;
	if (h === 0 && m === 0) return `${s}s`;
	if (h === 0) return `${m}m${s}s`;
	return `${h}h${m}m${s}s`;
};

console.log(
	`源文件 ${bytes.byteLength} 字节 → gzip ${packed.byteLength} 字节 → base32 ${text.length} 字符`,
);
console.log(
	`数据 ${parts.length} 片 + 还原脚本 ${restoreB32.length} 字符，共 ${totalChars} 个字符，按 ${delay}ms/键约 ${formatTime(seconds)}`,
);
// 远端没有 GNU base32 时用这条解出 restore.sh（与上面收尾第一行等价）
console.log("远端没有 base32 时用 python3：");
console.log(
	`  python3 -c 'import sys,base64;s="".join(open("${restoreB32Name}").read().split()).upper();s+="="*(-len(s)%8);sys.stdout.buffer.write(base64.b32decode(s))' > ${restorePath}`,
);

// ─── 7. 逐段输出 ─────────────────────────────────────────────────────────────
// 第一次 `$.type_str` 之前，宿主会弹待命窗口让你点好目标窗口；
// 中途焦点被抢走会自动重新待命，并从断点继续（已输入的字符不会重复）
for (let i = 0; i < steps.length; i++) {
	$.type_str(steps[i].text, delay);
	console.log(`已输出 ${i + 1}/${steps.length}：${steps[i].label}`);
	if (i + 1 < steps.length) await sleep(gapMs);
}

console.log("脚本执行完成");

/**
 * 接收端 bash 脚本的正文（发送内容就是它的 base32）。
 *
 * 约定：正文里不写 `${}`、反引号、反斜杠 —— 这样它能原样装进模板字面量，一个转义都
 * 不需要（测试会同时钉住这三条约定与「远端解出来的 == 这段正文」）。
 *
 * 定义成函数而不是顶层 `const`：模板字面量必须在**运行时**求值。若写成顶层常量，
 * 它的初始化时机就在 `$.type_str` 之前还是之后变得依赖语句顺序，容易改错。
 */
function getRestoreScript(): string {
	return `#!/usr/bin/env bash
# ClipBeam 接收端：拼接分片 → base32 解码（小写无填充）→ gunzip → 逐层校验
# 用法： bash restore.sh <base>          # base 是原文件名，例如 clipbeam-quick-start.txt
# 产物： <base>（还原出来的原文件），中间文件 <base>.gz.b32 / <base>.gz
# 依赖： gzip +（base32 或 python3）+（md5sum / md5 / openssl 之一）
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "用法：bash restore.sh <base>"
  exit 1
fi
base=$1
meta="$base.meta"
b32="$base.gz.b32"

meta_get() { sed -n "s/^$1=//p" "$meta" | head -1; }
parts=$(meta_get parts)
chars=$(meta_get chars)
b32_md5=$(meta_get b32_md5)
raw_md5=$(meta_get raw_md5)
raw_bytes=$(meta_get raw_bytes)

# md5 工具各平台不一样：GNU 是 md5sum，macOS/BSD 是 md5，兜底用 openssl
md5_of() {
  if command -v md5sum >/dev/null 2>&1; then md5sum "$1" | cut -d' ' -f1
  elif command -v md5 >/dev/null 2>&1; then md5 -q "$1"
  else openssl md5 "$1" | awk '{print $NF}'; fi
}

# 发送端用的是 base32 小写无填充（base32_lower_nopad），盘表里不会出现 =。
# 解码侧要自己把 = 补齐：base32 -d 的实现不止一种，busybox / BSD 系会按 RFC4648
# 校验「长度必须是 8 的倍数」，喂无填充就会报 invalid input（GNU 那种宽容的只是
# 运气好）；python3 的 b32decode 同样要求补齐。补的是**本地解码用**的，不是传输内容
# 无填充时末尾那组只编码 3 / 4 / 2 / 1 字节，所以长度余数只可能是 5 / 7 / 2 / 4，
# 依次补 3 / 1 / 6 / 4 个 =
# 长度用 wc -c 数：正文里既不能用长度展开（美元加大括号加井号那个写法），
# 也不能让算术展开紧跟着它 —— 两种写法都会被 TS 当成模板插值，把后面的行当代码
# 空白（heredoc 折行引入的换行）统一先删掉，长度也要按删完之后算
b32_decode() {
  if command -v base32 >/dev/null 2>&1; then
    _t=$(tr -d '[:space:]' | tr 'a-z' 'A-Z')
    _len=$(printf '%s' "$_t" | wc -c | tr -d ' ')
    _n=0
    case $(expr "$_len" % 8) in
      2) _n=6 ;;
      4) _n=4 ;;
      5) _n=3 ;;
      7) _n=1 ;;
    esac
    while [ "$_n" -gt 0 ]; do
      _t="$_t="
      _n=$(( _n - 1 ))
    done
    printf '%s' "$_t" | base32 -d
  else
    python3 -c 'import sys,base64;s="".join(sys.stdin.read().split()).upper();s+="="*(-len(s)%8);sys.stdout.buffer.write(base64.b32decode(s))'
  fi
}

# 1) 片号必须从 p0001 连续排到 pNNNN：缺一片、多一片、改名都拒绝
n=0
for f in "$b32".p[0-9][0-9][0-9][0-9]; do
  n=$((n + 1))
  want=$(printf '%s.p%04d' "$b32" "$n")
  if [ "$f" != "$want" ]; then
    echo "✗ 缺分片：$want"
    exit 1
  fi
done
if [ "$n" -ne "$parts" ]; then
  echo "✗ 分片数不符：$n ≠ $parts"
  exit 1
fi

# 2) 拼接后去掉所有空白（含换行），还原成发送端那一段完整 base32 文本
cat "$b32".p[0-9][0-9][0-9][0-9] | tr -d '[:space:]' > "$b32"
got_chars=$(wc -c < "$b32" | tr -d ' ')
if [ "$got_chars" -ne "$chars" ]; then
  echo "✗ base32 字符数不符：$got_chars ≠ $chars"
  exit 1
fi
if [ "$(md5_of "$b32")" != "$b32_md5" ]; then
  echo "✗ base32 摘要不符（传输过程中丢字或串位）"
  exit 1
fi

# 3) 解码 + 解压（截断或损坏会在这两步非零退出）
b32_decode < "$b32" > "$base.gz"
gzip -dc "$base.gz" > "$base"

# 4) 还原结果与源文件比对
if [ "$(wc -c < "$base" | tr -d ' ')" -ne "$raw_bytes" ]; then
  echo "✗ 还原大小不符"
  exit 1
fi
if [ "$(md5_of "$base")" != "$raw_md5" ]; then
  echo "✗ 还原摘要不符"
  exit 1
fi
echo "✓ 还原成功：$base"
`;
}
