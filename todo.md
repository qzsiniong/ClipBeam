# 待实现功能

- [ ] 整合文件发送和接收功能
- [x] 自定义 DSL (类 js 语法) —— 已由内置 JS/TS 脚本引擎实现
  - 原计划里的每一项现在都直接可用（不需要另造 DSL）：

    | 原计划写法 | 现在怎么写 |
    |---|---|
    | `var bytes = file("file/path")` | `const bytes = await $.file('file/path')` |
    | `var compressed = zstd(bytes)` | `const chunks = $.zstd(bytes, 1024)`（压缩 + 切片） |
    | `var str = base32(bytes)` | `const str = $.base32(bytes)` |
    | `typeStr(str)` | `$.typeStr(str, 10)` |
    | `var chunks = chunk(str, 100)` | `$.zstd(bytes, 1024)` 直接返回分片数组 |
    | `for (var chunk of chunks) { … }` | 原样支持 `for` / `for…of` / `if` / 函数 |
    | `sleep(1000)` | `await $.sleep(1000)` |
    | `var confirmed = confirm(...)` | `const confirmed = await $.confirm('确认发送吗？')` |
    | `set_delay(10)` | 尚无便捷包装，用 `$.typeStr(str, 10)` 的第二个参数 |

  - 完整说明见 README 的「脚本引擎（JS/TS）」；脚本页（侧边栏「脚本」）提供语法高亮、
    `$` 补全与 TypeScript 类型诊断，命令行用 `clipbeam script <文件>` 直接跑

```js
// 原计划里的示例，现在可以原样在脚本页里运行（`Clipbeam` 与 `$` 指向同一个全局对象）
const $ = Clipbeam
const delay = 10

const bytes = await $.file('file/path')       // 读取文件
const totalMd5 = $.md5(bytes)                 // 整文件 md5
const chunks = $.zstd(bytes, 1024)            // 压缩 + 按 1 KiB 切片

$.typeStr(`${totalMd5}\n\n`, delay)

for (let i = 0; i < chunks.length; i++) {
  const chunk = chunks[i]
  const chunkMd5 = $.md5(chunk)
  const chunkB32 = $.base32(chunk)

  const confirmed = await $.confirm('确认发送吗？')
  if (confirmed) {
    await $.sleep(1000)
    $.typeStr(`${i}\n${chunkMd5}\n${chunkB32}\n\n`, delay)
  }
}
```
