/* global $, console, sleep */
// ClipBeam 脚本示例：写文件 → 读回来 → 摘要 / 校验和 → 压缩 → 分片 → 确认 → 键盘输出
//
// 运行方式（二选一）：
//   * GUI：脚本页打开本文件，点「运行」，然后点击目标窗口；
//   * 命令行：cargo run -p clipbeam -- script crates/clipbeam-scripting/seed/01-quick-start.js --raw
//
// 用到的能力由 clipbeam-scripting 注入（见 crates/clipbeam-scripting/src/extensions/）。
//
// 两条容易被忽略的规则：
//   1. 文件路径必须是**绝对路径**：`~` 展开成主目录，`C:/x`、`/d/x`、`/cygdrive/d/x` 也认；
//   2. 修改文件（写 / 追加 / 删除 / 改名 / 复制 / 建目录）会先问你一次，
//      选「本次运行内该目录都允许」后，本次运行中该目录下不再重复询问。

// `$` 是能力命名空间的全局别名（正式名 `ClipBeam`），直接用即可 ——
// 它是引擎挂上的不可写全局，所以不能再 `const $ = ...`（会报重复声明）

// 每个字符之间的键盘延迟（毫秒）
const delay = 10

// 先写一个示例文件（第一次写会弹出授权确认）
const target = '~/clipbeam-quick-start.txt'
await $.write_text(target, 'ClipBeam 脚本引擎示例\n中文与 emoji 🎯\n')

// 读回来，并看一眼元信息
const bytes = await $.read(target)
const stat = await $.stat(target)

console.log(`文件：${stat.path}`)
console.log(`大小：${stat.size} 字节，修改于 ${new Date(stat.modifiedMs).toISOString()}`)

// 摘要与校验和
const totalMd5 = $.md5(bytes)
const totalCrc32 = $.crc32(bytes)
$.type_str(`MD5:   ${totalMd5}\nCRC32: ${totalCrc32}\n\n`, delay)

// 压缩后用 $.chunks 分片：压缩与切片是两件独立的事
const packed = $.zstd(bytes)
const parts = $.chunks(packed, 1024)
$.type_str(`压缩后 ${packed.byteLength} 字节，共 ${parts.length} 片\n\n`, delay)

for (let i = 0; i < parts.length; i++) {
  const part = parts[i]

  const confirmed = await $.confirm(`确认发送第 ${i + 1} 片吗？`)
  if (!confirmed) {
    console.log(`跳过分片 ${i + 1}`)
    continue
  }

  await sleep(200)
  $.type_str(`${i + 1}\n${$.md5(part)}\n${$.base32_nopad(part)}\n\n`, delay)
}

console.log('脚本执行完成')
