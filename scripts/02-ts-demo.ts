// ClipBeam 脚本示例（TypeScript）：解码非 UTF-8 文本并逐字输出
//
// 运行方式：
//   * GUI：脚本页打开本文件，点「运行」，然后点击目标窗口；
//   * 命令行：cargo run -p clipbeam -- script scripts/02-ts-demo.ts --raw
//
// .ts 在进程内用 oxc 转译（不需要装 Node / tsc）；类型注解会被剥掉。
// 注意：运行期报错的行号指向**转译后**的 JS，不做 source map。

/** 示例：只用来演示类型注解会被正确剥掉。 */
interface PreviewOptions {
  path: string
  previewChars: number
}

enum PreviewKind {
  Text,
  Binary,
}

const options: PreviewOptions = {
  path: 'README.md',
  previewChars: 300,
}

const $ = Clipbeam
const delay: number = 20

// 读文件（顶层 await 可用，不需要包 async IIFE）
const bytes: ArrayBuffer = await $.file(options.path)

// TextDecoder 支持全部 WHATWG 编码标签：utf-8 / gbk / gb18030 / big5 / shift_jis …
const text: string = new TextDecoder('utf-8').decode(bytes)

const kind: PreviewKind = PreviewKind.Text
console.log('预览类型：', kind === PreviewKind.Text ? 'text' : 'binary')

$.typeStr(`======= ${options.path} =======\n`, delay)
$.typeStr(`${text.slice(0, options.previewChars)}\n`, delay)
$.typeStr('=======\n', delay)

// 已输出字符数可以这样估算（与键盘延迟一起决定总耗时）
const estimatedMs = (text.length + options.path.length) * delay
await $.sleep(200)
console.log(`大致耗时 ${estimatedMs}ms`)
