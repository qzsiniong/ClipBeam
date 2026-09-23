/* global $, console, sleep */
// ClipBeam 脚本示例：压缩 → base32_lower 编码 → 分片 → 逐片以 heredoc 敲进远程终端
//
// 只有键盘一条通道时，把一个文件送到远端的最小闭环是：压小 → 编码成纯 ASCII →
// 切片 → 每片敲一条 `cat <<'EOF' > xx.pNNNN` → 远端拼接、解码、解压、校验。
// 本示例还会把**接收端脚本 restore.sh 自己**先敲过去（只做 base32 编码，不压缩、
// 不分片），远端不必再手抄一遍脚本。
//
// 运行方式（二选一）：
//   * GUI：脚本页打开本文件，点「运行」，再点击远程终端窗口；
//   * 命令行：cargo run -p clipbeam -- script crates/clipbeam-scripting/seed/03-pack-and-shard.js
//     （只有 stdin 是终端时 `$.confirm` 才能回答 `y`；重定向/管道时按仓库约定一律当「否」）
//
// 用法：改下面「可调参数」几个常量，然后跑起来。远端会依次收到：
//
//   cat <<'EOF' > clipbeam-restore.b32
//   mfrggzdfmztwq2lk…（restore.sh 正文的 base32，按 1024 字符折行）
//   EOF
//   # 还原脚本：tr -d '[:space:]' < clipbeam-restore.b32 | tr a-z A-Z | base32 -d > restore.sh
//   cat <<'EOF' > clipbeam-payload.meta
//   parts=8
//   chars=8071
//   b32_md5=2b0d…（整段 base32 文本的 md5）
//   raw_bytes=5021
//   raw_md5=6f1c…（原文件字节的 md5）
//   EOF
//   cat <<'EOF' > clipbeam-payload.p0001
//   mfrggzdfmztwq2lk…（1024 个 base32 小写字符，单行）
//   EOF
//   …（p0002 … p0008）
//   # 收齐 8 片后：bash restore.sh clipbeam-payload
//
// 最后两步示例**故意不替你敲**（免得在远端误执行），由你手动做：
//   1. 跑上面那条「还原脚本」命令，得到 restore.sh（远端没有 base32 时，用 console
//      面板里给出的 python3 版本，效果一样）；
//   2. 跑 `bash restore.sh clipbeam-payload` —— 它自己拼接、校验、解码、解压。
//
// 两条容易被忽略的规则（与 01-quick-start.js 一致）：
//   1. 文件路径必须是**绝对路径**：`~` 展开成主目录，相对路径直接报错；
//   2. 只读操作不会询问；本示例不写本机文件，所以不会弹文件授权框。
//
// 为什么这样选（都不是随意定的）：
//   * 压缩用 gzip 而不是 zstd/xz：远端只要有 `gzip -dc` 就能解，zstd/xz 很多机器没装；
//   * 编码用 base32_lower_nopad：盘表只有 a-z2-7，**不可能出现大写 `EOF`**，heredoc
//     定界符天然不会被内容撞破；而且全是无需按 Shift 的键，打字最不容易出错；
//   * 数据负载按 1024 字符分片（8 的倍数：每片正好整数个 base32 量子），便于分批；
//     还原脚本**不分片** —— 它只有几 KB，一个 heredoc 更简单；正文按 1024 字符折行只是
//     躲开终端对单行长度的限制，拼接时空白会被去掉；
//   * 还原脚本正文用模板字面量装：正文里不写 `${}`、反引号、反斜杠，于是一个转义都不用，
//     并有测试钉住「远端解出来的 == 这里的正文」。
//
// 键盘通道的耗时正比于字符数：本示例先算给你看，再用 `$.confirm` 问一次。
// base32 约 1.6 倍膨胀、gzip 对文本常有 2~5 倍收益，数据负载大致打个平手或更小。

// ─── 可调参数 ────────────────────────────────────────────────────────────────

// 要发送的文件（绝对路径）。01-quick-start.js 会生成这个文件，也可以改成任意文件
const srcPath = '~/clipbeam-quick-start.txt'
// 远端落盘的文件名前缀：分片是 `<stem>.p0001` …，元信息是 `<stem>.meta`
const stem = 'clipbeam-payload'
// 每片多少 base32 字符（8 的倍数最整齐；片数上限 9999，超过请调大本值）
const chunkSize = 1024
// 每个字符之间的键盘延迟（毫秒）：3 约等于 330 字符/秒，调大更稳、更慢
const delay = 3
// 每片之间额外等一会儿，给远端 shell 留出处理 heredoc 的余量
const gapMs = 120
// 还原脚本在远端的两个文件名
const restoreB32Name = 'clipbeam-restore.b32'
const restorePath = 'restore.sh'

// ↓↓↓ restore.sh 正文（发送内容就是它的 base32；改这里就等于改远端拿到的脚本）
// 约定：正文里不写 `${}`、反引号、反斜杠 —— 这样它能原样装进下面的模板字面量，
// 一个转义都不需要（测试会同时钉住这三条约定与「远端解出来的 == 这段正文」）。
// `${}` 的缺席也不是问题：`"$stem.out"` 这类写法里引号本身就把变量名截断了，
// 顺带还躲开了「中文全角括号被当成变量名一部分」那个坑。
const restoreScript = `#!/usr/bin/env bash
# ClipBeam 接收端：拼接分片 → base32_lower(nopad) 解码 → gunzip → 逐层校验
# 用法： bash restore.sh <stem> [输出文件]        # 默认输出 <stem>.out
# 依赖： gzip +（base32 或 python3）+（md5sum / md5 / openssl 之一）
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "用法：bash restore.sh <stem> [输出文件]"
  exit 1
fi
stem=$1
out="$stem.out"
if [ $# -ge 2 ]; then out=$2; fi
meta="$stem.meta"

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

# GNU base32 -d 大小写敏感（小写会报 invalid input），必须先转大写；它接受无填充。
# python3 的 b32decode 则要求补齐 =，所以要自己补。两条路都通
b32_decode() {
  if command -v base32 >/dev/null 2>&1; then
    tr 'a-z' 'A-Z' | base32 -d
  else
    python3 -c 'import sys,base64;s="".join(sys.stdin.read().split()).upper();s+="="*(-len(s)%8);sys.stdout.buffer.write(base64.b32decode(s))'
  fi
}

# 1) 片号必须从 p0001 连续排到 pNNNN：缺一片、多一片、改名都拒绝
n=0
for f in "$stem".p[0-9][0-9][0-9][0-9]; do
  n=$((n + 1))
  want=$(printf '%s.p%04d' "$stem" "$n")
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
cat "$stem".p[0-9][0-9][0-9][0-9] | tr -d '[:space:]' > "$stem.b32"
got_chars=$(wc -c < "$stem.b32" | tr -d ' ')
if [ "$got_chars" -ne "$chars" ]; then
  echo "✗ base32 字符数不符：$got_chars ≠ $chars"
  exit 1
fi
if [ "$(md5_of "$stem.b32")" != "$b32_md5" ]; then
  echo "✗ base32 摘要不符（传输过程中丢字或串位）"
  exit 1
fi

# 3) 解码 + 解压（截断或损坏会在这两步非零退出）
b32_decode < "$stem.b32" > "$stem.gz"
gzip -dc "$stem.gz" > "$out"

# 4) 还原结果与源文件比对
if [ "$(wc -c < "$out" | tr -d ' ')" -ne "$raw_bytes" ]; then
  echo "✗ 还原大小不符"
  exit 1
fi
if [ "$(md5_of "$out")" != "$raw_md5" ]; then
  echo "✗ 还原摘要不符"
  exit 1
fi
echo "✓ 还原成功：$out"
`
// ↑↑↑ restore.sh 正文

// ─── 1. 读文件 → 压缩 ────────────────────────────────────────────────────────

const bytes = await $.read(srcPath)
const packed = $.gzip(bytes)

// ─── 2. base32_lower 编码（小写、无填充）+ 往返自检 ──────────────────────────

const text = $.base32_lower_nopad(packed)

// 自检：解回来必须与压缩后的字节一致。编码能力用错（比如换成带填充的变体、
// 或误用大写版解码器）在这里就会抛错，而不是等远端还原失败才发现
if ($.md5($.base32_lower_nopad_decode(text)) !== $.md5(packed))
  throw new Error('base32 往返自检失败：编码/解码结果与压缩字节不一致')

// ─── 3. 分片 ─────────────────────────────────────────────────────────────────

const parts = $.chunks(text, chunkSize)

if (parts.length > 9999)
  throw new Error(`分片数 ${parts.length} 超过 9999（编号只有 4 位）：请把 chunkSize 调大`)

const pad4 = n => String(n).padStart(4, '0')
const heredoc = (name, body) => `cat <<'EOF' > ${name}\n${body}\nEOF\n`

// 接收端脚本靠这份元信息做校验：片数、base32 字符数、两层 md5。
// 用 `\n` 而不是 `\r\n`：远端 shell 会把 `\r` 当成普通字符写进文件
const meta = [
  `parts=${parts.length}`,
  `chars=${text.length}`,
  `b32_md5=${$.md5(text)}`,
  `raw_bytes=${bytes.byteLength}`,
  `raw_md5=${$.md5(bytes)}`,
].join('\n')

// ─── 4. 还原脚本：只 base32 编码（不压缩、不分片），一个 heredoc 敲过去 ──────

const restoreB32 = $.base32_lower_nopad(restoreScript)
// 按 1024 字符折行：还是一个 heredoc、一个文件，不是分片；解码时空白会被去掉
const restoreHeredoc = heredoc(restoreB32Name, $.chunks(restoreB32, 1024).join('\n'))
// 敲两行注释，把解码命令与自检命令留在远端屏幕上（注释不会被执行）
const restoreHint = `# 还原脚本：tr -d '[:space:]' < ${restoreB32Name} | tr a-z A-Z | base32 -d > ${restorePath}\n# 自检：bash -n ${restorePath}\n`
// 远端没有 GNU base32 时用这条（本地 console 面板也会打印）
const restoreHintPython = `python3 -c 'import sys,base64;s="".join(open("${restoreB32Name}").read().split()).upper();sys.stdout.buffer.write(base64.b32decode(s+"="*(-len(s)%8)))' > ${restorePath}`

// ─── 5. 拼出要敲的每一段，并预估耗时（字符数 × 每键延迟）────────────────────

const steps = [
  { label: `还原脚本 ${restoreB32Name}`, text: restoreHeredoc },
  { label: '还原命令提示', text: restoreHint },
  { label: `元信息 ${stem}.meta`, text: heredoc(`${stem}.meta`, meta) },
]
for (let i = 0; i < parts.length; i++) {
  const name = `${stem}.p${pad4(i + 1)}`
  steps.push({ label: `${name}（第 ${i + 1}/${parts.length} 片）`, text: heredoc(name, parts[i]) })
}
steps.push({ label: '收尾提示', text: `# 收齐 ${parts.length} 片后：bash ${restorePath} ${stem}\n` })

const totalChars = steps.reduce((sum, step) => sum + step.text.length, 0)
const seconds = (totalChars * delay / 1000).toFixed(1)

console.log(`源文件 ${bytes.byteLength} 字节 → gzip ${packed.byteLength} 字节 → base32 ${text.length} 字符`)
console.log(`数据 ${parts.length} 片 + 还原脚本 ${restoreB32.length} 字符，共 ${totalChars} 个字符，按 ${delay}ms/键约 ${seconds} 秒`)
console.log('远端解码还原脚本（base32 优先）：')
console.log(`  tr -d '[:space:]' < ${restoreB32Name} | tr a-z A-Z | base32 -d > ${restorePath}`)
console.log('远端没有 base32 时用 python3：')
console.log(`  ${restoreHintPython}`)

// ─── 6. 逐段输出 ─────────────────────────────────────────────────────────────

const confirmed = await $.confirm(
  `将向当前焦点窗口敲入 ${steps.length} 段（${totalChars} 个字符，约 ${seconds} 秒），继续吗？`,
)

if (!confirmed) {
  console.log('已取消，未输出任何内容')
}
else {
  // 第一次 `$.type_str` 之前，宿主会弹待命窗口让你点好目标窗口；
  // 中途焦点被抢走会自动重新待命，并从断点继续（已输入的字符不会重复）
  for (let i = 0; i < steps.length; i++) {
    $.type_str(steps[i].text, delay)
    console.log(`已输出 ${i + 1}/${steps.length}：${steps[i].label}`)
    if (i + 1 < steps.length)
      await sleep(gapMs)
  }
}

console.log('脚本执行完成')
