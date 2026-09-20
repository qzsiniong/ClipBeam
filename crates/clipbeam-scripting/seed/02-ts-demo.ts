/* global $, console, sleep */
// ClipBeam 脚本示例（TypeScript）：类型注解 + 编码 / 压缩往返 + 逐字输出
//
// 运行方式：
//   * GUI：脚本页打开本文件，点「运行」，然后点击目标窗口；
//   * 命令行：cargo run -p clipbeam -- script crates/clipbeam-scripting/seed/02-ts-demo.ts --raw
//
// .ts 在进程内用 oxc 转译（不需要装 Node / tsc）；类型注解会被剥掉。
// 注意：运行期报错的行号指向**转译后**的 JS，不做 source map。
//
// 这个示例是纯计算（不碰文件系统），因此不会弹任何授权确认 —— 只有真的
// `$.type_str` 输出前会弹待命窗口，让你先点好目标窗口。

/** 演示用的预览数据。 */
interface Preview {
  title: string
  body: string
}

/** 预览的展示方式（顺带演示 TS 的枚举也能被转译）。 */
enum PreviewMode {
  Plain,
  Compressed,
}

// `$` 是能力命名空间的全局别名（正式名 `ClipBeam`），不建议再自己声明
const delay: number = 20

const preview: Preview = {
  title: '编码与压缩往返',
  body: 'ClipBeam 的脚本引擎支持顶层 await、类型注解，以及中文与 emoji 🎯。\n',
}

// TextEncoder 是引擎补齐的标准全局：字符串 → UTF-8 字节
const encoded: Uint8Array = new TextEncoder().encode(preview.body)
// $.bytes 把任意二进制形态统一成 ArrayBuffer（视图 / 字符串也能直接传）
const bytes: ArrayBuffer = $.bytes(encoded)

// 编码：Base32（大写带填充）/ Base64 URL-safe（无填充）
console.log('base32 前缀：', $.base32(bytes).slice(0, 24), '…')
console.log('base64url 前缀：', $.base64_url_nopad(bytes).slice(0, 24), '…')

// 压缩往返：gzip 压完再解，应当与原文逐字节一致
const gzipped: ArrayBuffer = $.gzip(bytes)
const restored: string = new TextDecoder('utf-8').decode($.gunzip(gzipped))
console.log('gzip 往返一致：', restored === preview.body)

const mode: PreviewMode = PreviewMode.Compressed

$.type_str(`======= ${preview.title} =======\n`, delay)
$.type_str(`模式：${mode === PreviewMode.Compressed ? '压缩' : '原样'}\n`, delay)
$.type_str(`大小：${bytes.byteLength} → ${gzipped.byteLength} 字节\n`, delay)
$.type_str(`${preview.body}\n`, delay)
$.type_str('=======\n', delay)

// 已输出字符数可以这样估算（与键盘延迟一起决定总耗时）
const estimatedMs = (preview.body.length + preview.title.length) * delay
await sleep(200)
console.log(`大致耗时 ${estimatedMs}ms`)
