/* global $, console, sleep */
// ClipBeam 脚本示例（TypeScript）：把任意文件经**键盘通道（K）**送到远程网页，
// 由页面收片、校验、落盘，并通过**二维码通道（Q）**回报进度。
//
// 与 03-pack-and-shard 的区别是「目标」：那个往远程**终端**里敲 heredoc，让远端 shell
// 自己拼文件；这个往远程**浏览器页面**里敲协议帧，页面能回报状态，因此多了
// **断点续传**、**局部重传**与**整文件校验**。
//
// 流程（协议 v2）：
//
//   1. 敲探测帧 kbb（总片数 / 原始大小 / 分片大小 / 文件名 / 整文件 MD5 / 是否压缩）
//   2. 页面据此定位**会话目录**（会话键 = fileMd5-chunkSize），算出**缺失区间**并回报：
//        unauthorized  还没选保存目录，等用户授权
//        ready         从头发
//        partial       只缺 missing 里的那些片
//        complete      远端已有同一文件，直接结束
//        error         页面出错（原因在 reason 里）
//   3. 只把**缺失区间里**的分片敲过去（窗口 16 片一批，批间读一次反馈）
//   4. 页面收齐 → 拼接 → 整体解压 → 校验长度与 MD5 → 落盘 → 回报 complete
//
// 运行方式（二选一）：
//   * GUI：托盘「发送文件到远程」自动带上你要发的文件；也可以在本页改 srcPath 后点「运行」；
//   * 命令行：cargo run -p clipbeam -- script crates/clipbeam-scripting/seed/04-file-transfer.ts --raw
//
// 前置条件：远程页面已经点过「选择保存目录」（否则反馈恒为 unauthorized）。
//
// 用到的能力：read / md5 / zstd / chunks / base32_lower_nopad / base32_nopad_decode /
// crc32 / basename / format_duration / type_str / pick_path / scan_qr / exists。

// ─── 可调参数 ────────────────────────────────────────────────────────────────

// 要发送的文件（绝对路径）。`null` 表示运行时弹选择框让你挑 ——
// 托盘的「发送文件到远程」会把这个常量替换成你选的文件路径。
const srcPath: string | null = null
// 每片原始字节数。调大 = 片数少、每片字符多；4096 字节约 6.6K 字符/片。
// 它同时是**会话身份**的一部分（见下面的协议说明），改它会让旧分片不被复用。
const chunkBytes: number = 4096
// 每键延迟（毫秒）。越小越快，但云桌面丢键率会上升
const delay: number = 3
// 是否尝试压缩（整文件压缩后 < 95% 才真的启用）
const compress: boolean = true
// 缺失分片最多补发多少轮
const maxRounds: number = 10
// 每发多少片读一次反馈（把二维码往返摊薄，同时把单轮丢失范围限制在这个窗口内）
const windowSize: number = 16
// 等页面「可以开始」的上限（毫秒）
const readyTimeoutMs: number = 180_000
// 发完等「整文件校验通过」的上限（毫秒）
const finishTimeoutMs: number = 600_000
// 读一次反馈二维码的间隔（毫秒）
const pollMs: number = 400
// 是否输出调试信息（console.debug → 脚本 Console 面板 / 终端 stdout）。
// 默认开：脚本的控制台本来就是给它自己看的，排障时需要细节。
// 只关心结果、不想看细节就改成 false —— 注意正常进度仍会走 console.log。
const DEBUG: boolean = true

// ─── K 协议（宿主 → 远程）────────────────────────────────────────────────────
//
// 帧：0 <magic> 0 <flags> 0 <crc> 0 <tail…> 1
//
// `magic` **就是消息类型**（没有单独的 type/版本字段）：
//   kba 剪贴板文本   kbb 文件探测   kbc 文件分片
// 新增消息 = 新增 magic；旧接收页遇到不认识的 magic 会整帧忽略。
//
// 字符集纪律：**帧内任何字段都不含 `0`/`1`**，凡是自然表示里可能出现它们的字段，
// 一律先 `base32 小写无填充`。于是 `0` 只可能是字段分隔、`1` 只可能是整帧终结。
// base32 小写字母表是 `[a-z2-7]`，它是 `[a-z0-9]` 的子集 —— 整条流免 Shift。
//
//   flags  单个 base32 数字（5 bit → a..v）：bit0 = zstd，bit1..4 保留（必须为 0）
//   crc    7 字符 = base32_lower(crc32("magic 0 flags 0 tail"))，
//          也就是**除引导符与 crc 字段本身之外的全部内容** —— 所以 flags 与
//          seq/total/size/chunkSize 被改坏都会当场发现
//
//   kbb tail：<total> 0 <size> 0 <chunkSize> 0 <name> 0 <fileMd5>
//   kbc tail：<seq> 0 <total> 0 <size> 0 <chunkSize> 0 <payload>

/**
 * 调试输出（`console.debug`）。关掉时是空操作，调用点不必写 `if`。
 *
 * 与 `console.log` 的分工：`log` 给用户看进度（已发多少片、用时多少），
 * `debug` 给排障看细节（帧长、窗口边界、反馈原文、每片的字节数）。
 */
function dbg(...args: unknown[]): void {
	if (DEBUG)
		console.debug('[transfer]', ...args)
}

/** base32 小写字母表；`flags` 是它的一个"数字"，不是字符本身。 */
const B32_ALPHABET = 'abcdefghijklmnopqrstuvwxyz234567'
/** flags 的 bit0。 */
const FLAG_ZSTD = 1

/** 数值 → 十进制串 → base32 小写无填充（十进制里的 0/1 只出现在输入侧）。 */
function num(n: number): string {
	return $.base32_lower_nopad(String(n))
}

/** `flags` 值（0..=31）→ 单个 base32 字符。 */
function flagsChar(flags: number): string {
	return B32_ALPHABET.charAt(flags)
}

/**
 * 组装一帧：`0 <magic> 0 <flags> 0 <crc> 0 <tail> 1`。
 *
 * CRC 覆盖 `magic 0 flags 0 tail` —— 除引导符与 `crc` 字段本身之外的全部内容。
 * `flags` 必须在覆盖范围内：它的 bit0 决定载荷要不要解压，漏在校验之外的话，
 * 一个字符被改坏就会静默走进错误的解码分支。
 */
function kFrame(magic: string, flags: number, tail: string): string {
	const covered = magic + '0' + flagsChar(flags) + '0' + tail
	const crc = $.crc32(covered, 'base32_lower')
	const frame = '0' + magic + '0' + flagsChar(flags) + '0' + crc + '0' + tail + '1'
	dbg(`组帧 ${magic}：flags=${flagsChar(flags)} crc=${crc} tail=${tail.length} 字符 → 整帧 ${frame.length} 字符`)
	return frame
}

/** 组文件探测帧。`fileMd5` 是 base32(原始 16 字节) 的 26 字符串。 */
function probeFrame(
	total: number,
	rawSize: number,
	chunkSize: number,
	name: string,
	fileMd5: string,
	zstd: boolean,
): string {
	const tail = [
		num(total),
		num(rawSize),
		num(chunkSize),
		$.base32_lower_nopad(name),
		fileMd5,
	].join('0')
	return kFrame('kbb', zstd ? FLAG_ZSTD : 0, tail)
}

/** 组文件分片帧。`payload` 是该片**线上字节**的 base32。 */
function chunkFrame(
	seq: number,
	total: number,
	rawSize: number,
	chunkSize: number,
	payload: string,
	zstd: boolean,
): string {
	const tail = [
		num(seq),
		num(total),
		num(rawSize),
		num(chunkSize),
		payload,
	].join('0')
	return kFrame('kbc', zstd ? FLAG_ZSTD : 0, tail)
}

// ─── Q 协议（远程 → 宿主）────────────────────────────────────────────────────
//
// 帧：<magic>.<total>.<index>.<crc>.<payload>      magic：QBA 剪贴板 / QBB 控制消息
// `crc` 是**整条消息** payload 的摘要（同批各帧一致）：既是批量身份，也是整体校验。
// 这与 K 通道"每帧各算各的"是刻意不同的 —— Q 通道的几帧是一组轮播的二维码，
// 不是独立投递，校验自然该落在整条消息上。
//
// 控制消息是 JSON（字段用完整名称）：
//   {"type":"feedback","status":"partial","missing":"0-2,7"}
//   {"type":"feedback","status":"complete","saved":"报告 (1).pdf"}
//   {"type":"feedback","status":"error","reason":"整文件 MD5 不符"}

interface Feedback {
	status: string
	/** 缺失分片区间清单（0-based，`a-b` 闭区间，逗号分隔）；无缺失时为空串。 */
	missing: string
	/** complete 时页面实际落盘的文件名（同名冲突会被改成 `name (1).ext`）。 */
	saved: string
	/** error 时的原因。 */
	reason: string
}

// 反馈可能分多帧轮播：一次 $.scan_qr() 只拿到一帧，要按 (total, crc) 攒齐再解 JSON。
let fbTotal = 0
let fbCrc = ''
let fbFrames: Record<number, string> = {}

/**
 * 把一帧反馈喂进收集器。
 *
 * 返回两件事，它们**必须分开**：
 *
 * * `sawFrame` —— 这一帧的形状合法（说明屏幕上的确有本协议的反馈码）。哪怕消息还没攒齐，
 *   它也是真的。**不能用「攒齐了一条消息」来判断屏幕可读**：反馈可能是多帧的，
 *   按那种写法，只要第一条反馈被拆成两帧，就会永久误判为「读不到屏幕」而退回盲发；
 * * `message` —— 攒齐且校验通过的那条消息，否则 `null`。
 */
interface FeedResult {
	sawFrame: boolean
	message: Feedback | null
}

function feedFeedback(raw: string | null): FeedResult {
	const miss: FeedResult = { sawFrame: false, message: null }
	if (raw === null)
		return miss
	const parts = raw.trim().split('.')
	if (parts.length !== 5 || parts[0] !== 'QBB')
		return miss
	const total = Number(parts[1])
	const index = Number(parts[2])
	const crc = parts[3]
	const payload = parts[4]
	if (!Number.isInteger(total) || total < 1)
		return miss
	if (!Number.isInteger(index) || index < 0 || index >= total)
		return miss
	if (crc.length !== 7)
		return miss

	// 换了一批就整批重来（crc 是整条消息的摘要，它变 = 页面换了一条消息）
	if (total !== fbTotal || crc !== fbCrc) {
		fbTotal = total
		fbCrc = crc
		fbFrames = {}
	}
	fbFrames[index] = payload
	dbg(`反馈分帧 ${index + 1}/${total}（${payload.length} 字符），已攒 ${Object.keys(fbFrames).length} 帧`)

	let joined = ''
	for (let i = 0; i < total; i++) {
		const part = fbFrames[i]
		if (part === undefined)
			return { sawFrame: true, message: null }
		joined += part
	}

	// 整条消息的校验：crc 是整条 payload 的摘要（base32 大写 7 字符）
	fbTotal = 0
	fbCrc = ''
	fbFrames = {}
	if ($.crc32(joined, 'base32') !== crc)
		return { sawFrame: true, message: null }

	let parsed: { type?: unknown, status?: unknown, missing?: unknown, saved?: unknown, reason?: unknown }
	try {
		const text = new TextDecoder().decode($.base32_nopad_decode(joined))
		parsed = JSON.parse(text)
	}
	catch {
		// 画面里的其它二维码（宿主自己的出站码、别人的东西）会让解码失败
		return { sawFrame: true, message: null }
	}
	if (parsed.type !== 'feedback' || typeof parsed.status !== 'string')
		return { sawFrame: true, message: null }
	dbg('反馈消息', {
		status: parsed.status,
		missing: parsed.missing,
		saved: parsed.saved,
		reason: parsed.reason,
	})
	return {
		sawFrame: true,
		message: {
			status: parsed.status,
			missing: typeof parsed.missing === 'string' ? parsed.missing : '',
			saved: typeof parsed.saved === 'string' ? parsed.saved : '',
			reason: typeof parsed.reason === 'string' ? parsed.reason : '',
		},
	}
}

/** 缺失区间清单 → 片序号数组：`"0-2,7"` → [0,1,2,7]。 */
function parseMissing(missing: string, total: number): number[] {
	if (missing === '')
		return []
	const out: number[] = []
	for (const part of missing.split(',')) {
		const dash = part.indexOf('-')
		if (dash < 0) {
			const n = Number(part)
			if (!Number.isInteger(n) || n < 0 || n >= total)
				throw new Error('反馈里的缺失区间非法：' + missing)
			out.push(n)
		}
		else {
			const a = Number(part.slice(0, dash))
			const b = Number(part.slice(dash + 1))
			if (!Number.isInteger(a) || !Number.isInteger(b) || a < 0 || b < a || b >= total)
				throw new Error('反馈里的缺失区间非法：' + missing)
			for (let i = a; i <= b; i++)
				out.push(i)
		}
	}
	return out
}

/**
 * 一直读反馈，直到出现 `want` 里的某个状态或超时。
 *
 * 返回 `null` 有两种含义，调用方按「没有反馈」处理：超时了，或者当前环境根本读不了屏幕
 * （命令行 / headless 宿主下 `$.scan_qr()` 恒为 null —— 那种情况由调用方整体降级）。
 */
async function waitFor(want: string[], timeoutMs: number): Promise<Feedback | null> {
	const deadline = Date.now() + timeoutMs
	let last = ''
	while (Date.now() < deadline) {
		const fb = feedFeedback(await $.scan_qr()).message
		if (fb !== null) {
			const marker = fb.status + '|' + fb.missing + '|' + fb.reason
			if (marker !== last) {
				console.log('反馈：' + fb.status
					+ (fb.missing === '' ? '' : ' 缺 ' + fb.missing)
					+ (fb.reason === '' ? '' : '（' + fb.reason + '）'))
				last = marker
			}
			if (want.indexOf(fb.status) >= 0)
				return fb
			// unauthorized 需要用户去页面上点一下，继续等而不是退出
		}
		await sleep(pollMs)
	}
	return null
}

// ─── 1. 取文件并开跑 ─────────────────────────────────────────────────────────

// `srcPath` 为 null 时弹选择框；选中才跑，取消就干净退出（取消不是错误）
const chosen: string | null = srcPath === null
	? await $.pick_path('选择要通过键盘通道发送的文件')
	: srcPath

if (chosen === null) {
	console.log('没有选择文件，已取消。')
} else {
	await sendFile(chosen)
}

// ─── 主流程 ──────────────────────────────────────────────────────────────────

async function sendFile(filePath: string): Promise<void> {
	if (!(await $.exists(filePath))) {
		throw new Error('文件不存在：' + filePath)
	}

	const bytes: ArrayBuffer = await $.read(filePath)
	const rawSize: number = bytes.byteLength
	// 整文件摘要：base32(原始 16 字节) = 26 字符，可直接进帧。
	// 同一个值页面还会拿它当**会话键**的一部分，所以格式必须一次写对。
	const fileMd5: string = $.md5(bytes, 'base32_lower')
	const name: string = $.basename(filePath) || 'received.bin'

	// ── 2. 压缩预判：整文件压不到 95% 以下就整体不压缩 ──
	const packed: ArrayBuffer = $.zstd(bytes, 3)
	const useCompress: boolean = compress && packed.byteLength < rawSize * 0.95

	// ── 3. 分片：压缩（或不压）之后**整段切片** ──
	//
	// 顺序就是「读 → 整文件压缩 → 切片 → 每片 base32 编码」：压缩发生在**整个文件**上，
	// 分片只是把这条流切开。所以 flags 带 zstd 时每片是**同一条 zstd 流的片段**，
	// 接收端收齐后**整体**解压一次（不是逐片解压）。
	// （空文件补一片空载荷，否则接收端永远等不到第 0 片。）
	const stream: ArrayBuffer = useCompress ? packed : bytes
	let parts: ArrayBuffer[] = $.chunks(stream, chunkBytes)
	if (parts.length === 0)
		parts = [new ArrayBuffer(0)]
	const total: number = parts.length
	const payloadSize: number = stream.byteLength

	const ratio = rawSize > 0 ? (payloadSize / rawSize).toFixed(2) : '—'
	console.log(
		'文件 ' + name + '：' + rawSize + ' 字节 → ' +
		(useCompress ? 'zstd ' : '未压缩 ') + payloadSize + ' 字节（' + ratio + '×）',
	)
	console.log('共 ' + total + ' 片，每片 ' + chunkBytes + ' 字节')
	dbg('文件事实', {
		filePath,
		name,
		rawSize,
		payloadSize,
		useCompress,
		chunkSize: chunkBytes,
		total,
		fileMd5,
		delay,
		windowSize,
		maxRounds,
	})

	// 每片的帧体先全部构造好：只读一次源数据，重传时直接用
	const frames: string[] = []
	for (let i = 0; i < total; i++) {
		frames.push(
			chunkFrame(
				i,
				total,
				rawSize,
				chunkBytes,
				$.base32_lower_nopad(parts[i]),
				useCompress,
			),
		)
	}

	// ── 4. 先探一次：能不能读反馈 ──
	//
	// 读不到（命令行 / headless / 用户没授权屏幕录制）就只能盲发：仍会把文件送过去，
	// 但没有续传、没有局部重传、也没有整文件校验。这一点必须明确告诉用户，不能假装成功。
	// 判据是「看到了一帧形状合法的反馈码」，不是「攒齐了一条消息」——
	// 反馈可能是多帧的（缺失区间长的时候），按后者写会在第一条反馈被拆帧时
	// 永久误判为「读不到屏幕」，白白退回盲发。
	const firstScan = await $.scan_qr()
	dbg('首次扫码结果', firstScan === null ? 'null（画面里没有二维码）' : firstScan.slice(0, 80))
	const feedbackAvailable = feedFeedback(firstScan).sawFrame
	dbg('反馈通道可用性', feedbackAvailable)
	if (!feedbackAvailable) {
		console.log('⚠️ 读不到页面反馈（未授权屏幕录制，或当前环境没有屏幕访问）。')
		console.log('   将一次性盲发全部 ' + total + ' 片：没有断点续传，也没有整文件校验。')
	}

	// ── 5. 发探测帧，等页面表态 ──
	const probe = probeFrame(total, rawSize, chunkBytes, name, fileMd5, useCompress)
	dbg(`发送探测帧（${probe.length} 字符）`, probe)
	$.type_str(probe, delay)

	// 待发片序号：默认全部；页面回报 partial 时就只发它列的缺失区间
	let pending: number[] = []
	for (let i = 0; i < total; i++)
		pending.push(i)

	if (feedbackAvailable) {
		const ready = await waitFor(['ready', 'partial', 'complete', 'error'], readyTimeoutMs)
		if (ready === null) {
			console.log('⚠️ 等不到页面反馈。仍将按顺序发送全部分片。')
		}
		else if (ready.status === 'error') {
			throw new Error('页面报错：' + ready.reason)
		}
		else if (ready.status === 'complete') {
			console.log('✓ 远端已有同一文件' + (ready.saved === '' ? '' : '（' + ready.saved + '）') + ' —— 无需发送。')
			return
		}
		else if (ready.status === 'partial') {
			pending = parseMissing(ready.missing, total)
			dbg(`探测回复 partial：missing=${ready.missing} → 待发 ${pending.length} 片`, pending.slice(0, 32))
			console.log('远端已有 ' + (total - pending.length) + '/' + total + ' 片，只发缺口。')
		}
		else {
			dbg(`探测回复 ${ready.status}，从头发送全部分片`)
		}
	}

	if (pending.length === 0) {
		console.log('✓ 页面报告分片已齐，等待它拼装校验。')
	}

	for (let i = 0; i < Math.min(total, 3); i++)
		dbg(`分片 ${i} 帧长 ${frames[i].length} 字符`)
	dbg(`构建完成：${total} 帧，共 ${frames.reduce((sum, frame) => sum + frame.length, 0)} 字符`)

	const frameChars = frames.reduce((sum, frame) => sum + frame.length, 0)
	const perRound = pending.length === 0
		? 0
		: Math.floor(pending.reduce((sum, i) => sum + frames[i].length, 0) * delay / 1000)
	console.log('本轮需敲入约 ' + perRound + ' 秒的字符（单帧共 ' + frameChars + ' 字符）')

	// ── 6. 按窗口发送缺失分片，批间读一次反馈 ──
	const startedAt = Date.now()
	let sent = 0

	for (let round = 0; round <= maxRounds; round++) {
		for (let at = 0; at < pending.length; at++) {
			const seq = pending[at]
			$.type_str(frames[seq], delay)
			sent++
			// 窗口边界才读反馈：把二维码往返摊薄，同时把单轮丢失限制在窗口内
			const atWindowEnd = (at + 1) % windowSize === 0 || at + 1 === pending.length
			if (at % windowSize === 0)
				dbg(`窗口起点：pending[${at}] = seq ${seq}（本窗口帧长 ${frames[seq].length} 字符）`)
			if (!feedbackAvailable || !atWindowEnd)
				continue
			dbg(`窗口结束：已发 ${sent} 片，读一次反馈`)

			const mid = await waitFor(['partial', 'complete', 'error'], finishTimeoutMs)
			if (mid === null)
				continue
			if (mid.status === 'error')
				throw new Error('页面报错：' + mid.reason)
			if (mid.status === 'complete') {
				const elapsed = ((Date.now() - startedAt) / 1000).toFixed(1)
				dbg(`完成：共发出 ${sent} 片，用时 ${elapsed}s，远端文件名 ${mid.saved || '(未回报)'}`)
				console.log('✓ 传输完成，页面已拼好文件且整文件 MD5 校验通过（用时 ' + elapsed + 's）')
				if (mid.saved !== '')
					console.log('  远端文件名：' + mid.saved)
				return
			}
			// partial：只补它列的缺口，剩下的本轮不再发
			const missing = parseMissing(mid.missing, total)
			if (missing.length === 0) {
				// 片都到了但还没 complete：多半正在拼装/校验，再等一轮
				console.log('分片已齐，等待页面拼装校验…')
				pending = []
				break
			}
			dbg(`批间反馈 partial：missing=${mid.missing} → 解析出 ${missing.length} 片待发`)
			const missingSet = new Set(missing)
			const rest = pending.slice(at + 1).filter(seq2 => missingSet.has(seq2))
			dbg(`本轮余下未发且仍缺的片：${rest.length}（原 pending 剩余 ${pending.length - at - 1}）`)
			const elapsed = (Date.now() - startedAt) / 1000
			console.log(
				'已发 ' + sent + ' 片（用时 ' + $.format_duration(Math.floor(elapsed))
				+ '），还缺 ' + missing.length + ' 片',
			)
			pending = missing.concat(rest)
			at = -1 // 重新从头遍历新的 pending
		}

		if (!feedbackAvailable) {
			console.log('✓ 已盲发全部 ' + total + ' 片（未校验，请自行确认远程文件完整）。')
			return
		}

		// ── 7. 等页面收齐并校验 ──
		const done = await waitFor(['complete', 'partial', 'error'], finishTimeoutMs)
		if (done === null) {
			console.log('⚠️ 等不到页面反馈（超时）。可能仍有分片在路上。')
			return
		}
		if (done.status === 'error')
			throw new Error('页面报错：' + done.reason)
		if (done.status === 'complete') {
			const elapsed = ((Date.now() - startedAt) / 1000).toFixed(1)
			console.log('✓ 传输完成，页面已拼好文件且整文件 MD5 校验通过（用时 ' + elapsed + 's）')
			if (done.saved !== '')
				console.log('  远端文件名：' + done.saved)
			return
		}

		pending = parseMissing(done.missing, total)
		dbg(`第 ${round + 1} 轮结束：missing=${done.missing} → 待补 ${pending.length} 片`)
		if (pending.length === 0) {
			console.log('分片已齐但页面尚未报 complete —— 可能正在写盘，稍候。')
			await sleep(2000)
			continue
		}
		console.log('第 ' + (round + 1) + ' 轮补发 ' + pending.length + ' 片。')
	}

	throw new Error('补发 ' + maxRounds + ' 轮后仍未收齐，请检查焦点是否被抢走、或调小 delay 后重试')
}
