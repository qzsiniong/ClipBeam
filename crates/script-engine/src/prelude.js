// src/prelude.js —— 运行时前置脚本（在用户脚本之前执行一次）
//
// quickjs-ng 的核心只实现 ECMAScript，不含 Web/Node 的宿主 API。这里补齐脚本作者
// 最常用的几个全局对象，实现方式是「薄壳 + Rust 原语」：
//
//   * 类语义（`new X()`、getter）和错误类型（RangeError / TypeError）放在 JS 里，
//     因为规范要求的就是这些 JS 层面的行为；
//   * 真正的编码转换交给 Rust 原语 `globalThis.__primitives`（见 src/bindings/text.rs），
//     由 encoding_rs 提供全部 WHATWG 编码（utf-8 / gbk / gb18030 / big5 / shift_jis …）。
//
// 与 src/bindings.d.ts 中的声明一一对应，改这里时记得同步更新类型提示。

(() => {
	"use strict";

	// 引导用的原语对象：引擎在 setup 阶段挂上，本文件执行完会被删掉
	// （所以这里必须把它们收进闭包，后面 console / TextDecoder 都靠它们）。
	const prim = globalThis.__primitives;
	if (!prim) {
		throw new Error("script-engine prelude: 内部对象 __primitives 未注册（运行时初始化顺序有问题）");
	}

	// ── 把 BufferSource 归一化成 ArrayBuffer ──────────────────────────────
	// 视图（Uint8Array / DataView…）可能只覆盖底层 buffer 的一段，需要切出精确区间。
	// 返回 null 表示「没有传参数」，由调用方决定语义（TextDecoder.decode() 返回空串）。
	function toArrayBuffer(input) {
		if (input === undefined) {
			return null;
		}
		if (input instanceof ArrayBuffer) {
			return input;
		}
		if (ArrayBuffer.isView(input)) {
			return input.buffer.slice(input.byteOffset, input.byteOffset + input.byteLength);
		}
		throw new TypeError("The provided value is not of type (ArrayBuffer or ArrayBufferView)");
	}

	// ── TextDecoder ──────────────────────────────────────────────────────
	class TextDecoder {
		#encoding; // 规范名（小写），如 "utf-8" / "gbk"
		#fatal;
		#ignoreBOM;

		constructor(label = "utf-8", options = {}) {
			// 未知标签要在构造时就抛 RangeError（构造即校验），而不是等到 decode
			const canonical = prim.encodingName(String(label));
			if (canonical === null) {
				throw new RangeError(`The encoding label provided ('${label}') is invalid.`);
			}

			const opts = options === undefined || options === null ? {} : options;
			this.#encoding = canonical.toLowerCase();
			this.#fatal = Boolean(opts.fatal);
			this.#ignoreBOM = Boolean(opts.ignoreBOM);
		}

		get encoding() {
			return this.#encoding;
		}

		get fatal() {
			return this.#fatal;
		}

		get ignoreBOM() {
			return this.#ignoreBOM;
		}

		// 注意：不支持第二个参数 { stream: true }（一次性解码整块数据）。
		decode(input) {
			const bytes = toArrayBuffer(input);
			if (bytes === null) {
				return "";
			}

			const text = prim.decodeText(bytes, this.#encoding, this.#fatal, this.#ignoreBOM);
			if (text === null) {
				// 两种情况：fatal 模式下数据非法，或传入的 ArrayBuffer 已被 detach
				throw new TypeError(`The encoded data was not valid for encoding ${this.#encoding}`);
			}
			return text;
		}
	}

	// ── TextEncoder（规范固定 UTF-8）──────────────────────────────────────
	class TextEncoder {
		constructor() {}

		get encoding() {
			return "utf-8";
		}

		encode(input = "") {
			return new Uint8Array(prim.encodeText(String(input)));
		}
	}

	// ── console ──────────────────────────────────────────────────────────
	// 只做最常见的 5 个方法；字符串原样输出，其余值用 JSON.stringify 展开，
	// 失败（循环引用等）时退回 String()。
	function format(args) {
		return args
			.map((value) => {
				if (typeof value === "string") {
					return value;
				}
				try {
					const json = JSON.stringify(value);
					return json === undefined ? String(value) : json;
				} catch {
					return String(value);
				}
			})
			.join(" ");
	}

	function writer(level) {
		return (...args) => {
			prim.print(level, format(args));
		};
	}

	globalThis.TextDecoder = TextDecoder;
	globalThis.TextEncoder = TextEncoder;
	globalThis.console = {
		log: writer("log"),
		info: writer("info"),
		debug: writer("debug"),
		warn: writer("warn"),
		error: writer("error"),
	};
})();
