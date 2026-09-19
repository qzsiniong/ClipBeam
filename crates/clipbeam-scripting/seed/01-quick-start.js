// ClipBeam 脚本示例：读取文件 → 摘要 → 压缩分片 → 确认 → 键盘输出
//
// 运行方式（二选一）：
//   * GUI：脚本页打开本文件，点「运行」，然后点击目标窗口；
//   * 命令行：cargo run -p clipbeam -- script scripts/01-quick-start.js --raw
//
// 用到的能力由 clipbeam-scripting 注入（见 crates/clipbeam-scripting/src/）。

// `Clipbeam` 与 `$` 指向同一个全局对象；这里取个短名字方便书写
const $ = Clipbeam

// 每个字符之间的键盘延迟（毫秒）
const delay = 10

// 读取文件（相对路径按进程工作目录解析）
const bytes = await $.file('README.md')

// 整文件摘要
const totalMd5 = $.md5(bytes)
$.typeStr(`MD5: ${totalMd5}\n\n`, delay)

// 压缩并按 1 KiB 分片
const chunks = $.zstd(bytes, 1024)
$.typeStr(`共 ${chunks.length} 片\n\n`, delay)

for (let i = 0; i < chunks.length; i++) {
  const chunk = chunks[i]
  const chunkMd5 = $.md5(chunk)
  const chunkB32 = $.base32(chunk)

  const confirmed = await $.confirm(`确认发送分片 ${i} 吗？`)
  if (!confirmed) {
    console.log(`跳过分片 ${i}`)
    continue
  }

  await $.sleep(1000)
  $.typeStr(`${i}\n${chunkMd5}\n${chunkB32}\n\n`, delay)
}

console.log('脚本执行完成')
