//! context_* 与 webview_* 的纯脚本子集由 Boa 实装（页面建模见 crate::webview）。
//! 需要真实页面加载/导航/cookie store 的 webview 函数显式不支持：返回 ABI 的
//! -1 家族错误码（JsError 没有 unsupported 专用码，上游 aidoku-rs 对这些函数
//! 也一律 -1），并在源日志点名函数与原因。
use crate::{
	WasmEnv,
	js_engine::JsEngine,
	runtime::store::{FFIResult, Ptr, Rid, StoreItem},
	webview::WebView,
};
use wasmer::FunctionEnvMut;

// 变体名与 ABI 的 JsError 对齐，命名不由本仓决定
#[allow(clippy::enum_variant_names)]
enum Result {
	Success,
	MissingResult,
	InvalidContext,
	InvalidString,
	InvalidHandler,
}

impl From<Result> for i32 {
	fn from(result: Result) -> i32 {
		match result {
			Result::Success => 0,
			Result::MissingResult => -1,
			Result::InvalidContext => -2,
			Result::InvalidString => -3,
			Result::InvalidHandler => -4,
		}
	}
}

pub fn context_create(mut env: FunctionEnvMut<WasmEnv>) -> Rid {
	let limits = &env.data().limits;
	let engine = JsEngine::new(limits.js_timeout, limits.js_loop_max);
	env.data_mut().store.store(StoreItem::JsContext(Box::new(engine)))
}

pub fn context_eval(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(source) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = env.data_mut().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.eval(&source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

pub fn context_eval_async(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	string_ptr: Ptr,
	len: u32,
) -> FFIResult {
	let Ok(source) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = env.data_mut().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.eval_async(&source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

pub fn context_get(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(name) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = env.data_mut().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.get(&name) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

// ABI 没有区分失败原因的错误码，统一 -1；细节进源日志（与 AidokuRunner 的
// exceptionHandler 打印对齐），截断防止源用超长 rejection 值灌爆日志
fn fail(env: &mut FunctionEnvMut<WasmEnv>, msg: String) -> FFIResult {
	let truncated: String = msg.chars().take(200).collect();
	env.data().log(&format!("js failed: {truncated}"));
	Result::MissingResult.into()
}

pub fn webview_create(mut env: FunctionEnvMut<WasmEnv>) -> Rid {
	let limits = &env.data().limits;
	let view = WebView::new(limits.js_timeout, limits.js_loop_max);
	env.data_mut().store.store(StoreItem::WebView(Box::new(view)))
}

pub fn webview_set_rule_list(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	unsupported(&mut env, webview, "webview_set_rule_list", "content blocking only applies to real page loads")
}

pub fn webview_load(mut env: FunctionEnvMut<WasmEnv>, webview: Rid, _request: Rid) -> FFIResult {
	unsupported(&mut env, webview, "webview_load", "no real page navigation on a server")
}

pub fn webview_load_html(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
	url_ptr: Ptr,
	url_len: u32,
) -> FFIResult {
	let Ok(html) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let base_url = if url_len > 0 {
		let Ok(url) = env.data().read_string(&env, url_ptr, url_len) else {
			return Result::InvalidString.into();
		};
		Some(url)
	} else {
		None
	};
	let Some(view) = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	let logs = view.load_html(&html, base_url.as_deref());
	for line in logs {
		env.data().log(&line);
	}
	Result::Success.into()
}

pub fn webview_wait_for_load(mut env: FunctionEnvMut<WasmEnv>, webview: Rid) -> FFIResult {
	// 沙箱里 load_html 同步完成，没有异步加载要等
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	Result::Success.into()
}

pub fn webview_eval(mut env: FunctionEnvMut<WasmEnv>, webview: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(source) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(view) = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	match view.eval(&source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

pub fn webview_eval_async(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
) -> FFIResult {
	let Ok(source) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(view) = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	match view.eval_async(&source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

pub fn webview_add_user_script(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
	at_document_end: i32,
	_for_main_frame_only: i32,
) -> FFIResult {
	let Ok(source) = env.data().read_string(&env, string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(view) = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	view.add_user_script(source, at_document_end != 0);
	Result::Success.into()
}

pub fn webview_get_cookies(mut env: FunctionEnvMut<WasmEnv>, webview: Rid) -> FFIResult {
	unsupported(&mut env, webview, "webview_get_cookies", "the sandbox has no cookie store")
}

// 签名由源 ABI 固定，无法归并参数
#[allow(clippy::too_many_arguments)]
pub fn webview_delete_cookie(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	_name_ptr: Ptr,
	_name_len: u32,
	_value_ptr: Ptr,
	_value_len: u32,
	_domain_ptr: Ptr,
	_domain_len: u32,
) -> FFIResult {
	unsupported(&mut env, webview, "webview_delete_cookie", "the sandbox has no cookie store")
}

// 不支持的函数仍先校验 webview rid（对齐 iOS 的 InvalidHandler 优先级），
// 其余参数不校验：操作本身不支持，参数没有消费方
fn unsupported(env: &mut FunctionEnvMut<WasmEnv>, webview: Rid, name: &str, why: &str) -> FFIResult {
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	env.data().log(&format!("{name} is not supported by the sandbox: {why}"));
	Result::MissingResult.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		runtime::{
			env::{Defaults, NetState},
			limits::Limits,
			store::GlobalStore,
		},
		traits::{DirectPolicy, NoopDefaultsSink, StdoutLogger},
	};
	use std::{collections::HashMap, sync::Arc, time::Duration};
	use wasmer::{FunctionEnv, Memory, MemoryType, Pages, Store};

	struct Harness {
		store: Store,
		env: FunctionEnv<WasmEnv>,
		memory: Memory,
		bump: u32,
	}

	impl Harness {
		fn new(limits: Limits) -> Self {
			let mut store = Store::default();
			let memory = Memory::new(&mut store, MemoryType::new(Pages(4), None, false)).unwrap();
			let env = FunctionEnv::new(
				&mut store,
				WasmEnv {
					memory: Some(memory.clone()),
					store: GlobalStore::new(),
					defaults: Defaults::new(HashMap::new(), Arc::new(NoopDefaultsSink), "test".into()),
					net: NetState::new(Arc::new(DirectPolicy { user_agent: None })),
					limits,
					source_key: "test".into(),
					logger: Arc::new(StdoutLogger),
					partial_sink: None,
				},
			);
			Self { store, env, memory, bump: 1024 }
		}

		fn write_str(&mut self, s: &str) -> (u32, u32) {
			let ptr = self.bump;
			self.bump += s.len() as u32 + 64;
			self.memory.view(&self.store).write(ptr as u64, s.as_bytes()).unwrap();
			(ptr, s.len() as u32)
		}

		fn create(&mut self) -> Rid {
			context_create(self.env.clone().into_mut(&mut self.store))
		}

		fn eval(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			context_eval(self.env.clone().into_mut(&mut self.store), rid, p, l)
		}

		fn eval_async(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			context_eval_async(self.env.clone().into_mut(&mut self.store), rid, p, l)
		}

		fn get(&mut self, rid: Rid, name: &str) -> FFIResult {
			let (p, l) = self.write_str(name);
			context_get(self.env.clone().into_mut(&mut self.store), rid, p, l)
		}

		fn destroy(&mut self, rid: Rid) {
			crate::imports::std::destroy(self.env.clone().into_mut(&mut self.store), rid);
		}

		fn eval_to_string(&mut self, rid: Rid, source: &str) -> String {
			let result = self.eval(rid, source);
			assert!(result > 0, "eval {source:?} 返回 {result}");
			self.stored_string(result)
		}

		fn stored_string(&self, rid: Rid) -> String {
			let Some(StoreItem::String(s)) = self.env.as_ref(&self.store).store.get(rid) else {
				panic!("rid {rid} 不是 StoreItem::String");
			};
			s.clone()
		}

		fn wv_create(&mut self) -> Rid {
			webview_create(self.env.clone().into_mut(&mut self.store))
		}

		fn wv_load_html(&mut self, rid: Rid, html: &str, url: Option<&str>) -> FFIResult {
			let (p, l) = self.write_str(html);
			let (up, ul) = match url {
				Some(u) => self.write_str(u),
				None => (0, 0),
			};
			webview_load_html(self.env.clone().into_mut(&mut self.store), rid, p, l, up, ul)
		}

		fn wv_eval(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_eval(self.env.clone().into_mut(&mut self.store), rid, p, l)
		}

		fn wv_eval_to_string(&mut self, rid: Rid, source: &str) -> String {
			let result = self.wv_eval(rid, source);
			assert!(result > 0, "webview eval {source:?} 返回 {result}");
			self.stored_string(result)
		}

		fn wv_eval_async(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_eval_async(self.env.clone().into_mut(&mut self.store), rid, p, l)
		}

		fn wv_add_user_script(&mut self, rid: Rid, source: &str, at_document_end: bool) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_add_user_script(
				self.env.clone().into_mut(&mut self.store),
				rid,
				p,
				l,
				at_document_end as i32,
				0,
			)
		}

		fn wv_wait_for_load(&mut self, rid: Rid) -> FFIResult {
			webview_wait_for_load(self.env.clone().into_mut(&mut self.store), rid)
		}
	}

	#[test]
	fn eval_and_get_roundtrip() {
		let mut h = Harness::new(Limits::default());
		let ctx = h.create();
		assert!(ctx > 0);
		assert_eq!(h.eval_to_string(ctx, "1 + 2"), "3");
		assert_eq!(h.eval_to_string(ctx, "'a' + 'b'"), "ab");
		assert_eq!(h.eval_to_string(ctx, "var x = 5; x * 2"), "10");
		let rid = h.get(ctx, "x");
		assert!(rid > 0, "get 返回 {rid}");
		assert_eq!(h.stored_string(rid), "5");
	}

	#[test]
	fn get_missing_global_is_undefined() {
		let mut h = Harness::new(Limits::default());
		let ctx = h.create();
		let rid = h.get(ctx, "nope");
		assert!(rid > 0, "get 返回 {rid}");
		assert_eq!(h.stored_string(rid), "undefined");
	}

	#[test]
	fn eval_async_resolves_promises() {
		let mut h = Harness::new(Limits::default());
		let ctx = h.create();
		let rid = h.eval_async(ctx, "Promise.resolve(40 + 2)");
		assert!(rid > 0, "eval_async 返回 {rid}");
		assert_eq!(h.stored_string(rid), "42");
		let rid = h.eval_async(ctx, "(async () => 'async-ok')()");
		assert!(rid > 0, "eval_async 返回 {rid}");
		assert_eq!(h.stored_string(rid), "async-ok");
		// 非 Promise 值等价于 await 原值
		let rid = h.eval_async(ctx, "1 + 1");
		assert!(rid > 0, "eval_async 返回 {rid}");
		assert_eq!(h.stored_string(rid), "2");
	}

	#[test]
	fn eval_failures_map_to_missing_result() {
		let mut h = Harness::new(Limits::default());
		let ctx = h.create();
		assert_eq!(h.eval(ctx, "("), -1);
		assert_eq!(h.eval(ctx, "throw new Error('boom')"), -1);
		assert_eq!(h.eval_async(ctx, "Promise.reject(new Error('bad'))"), -1);
		assert_eq!(h.eval_async(ctx, "new Promise(() => {})"), -1);
	}

	#[test]
	fn contexts_are_isolated() {
		let mut h = Harness::new(Limits::default());
		let ctx_a = h.create();
		let ctx_b = h.create();
		h.eval(ctx_a, "var secret = 'a'");
		let rid = h.get(ctx_b, "secret");
		assert!(rid > 0);
		assert_eq!(h.stored_string(rid), "undefined");
	}

	#[test]
	fn invalid_context_and_string_map_to_codes() {
		let mut h = Harness::new(Limits::default());
		let (p, l) = h.write_str("1 + 1");
		assert_eq!(context_eval(h.env.clone().into_mut(&mut h.store), 9999, p, l), -2);
		assert_eq!(context_eval_async(h.env.clone().into_mut(&mut h.store), 9999, p, l), -2);
		assert_eq!(context_get(h.env.clone().into_mut(&mut h.store), 9999, p, l), -2);
		let ctx = h.create();
		assert_eq!(context_eval(h.env.clone().into_mut(&mut h.store), ctx, 0x7fff_fff0, 32), -3);
	}

	#[test]
	fn destroy_releases_context() {
		let mut h = Harness::new(Limits::default());
		let ctx = h.create();
		h.destroy(ctx);
		assert_eq!(h.eval(ctx, "1 + 1"), -2);
	}

	#[test]
	fn timeout_interrupts_runaway_script() {
		let limits = Limits { js_timeout: Duration::from_millis(200), ..Limits::default() };
		let mut h = Harness::new(limits);
		let ctx = h.create();
		h.eval(ctx, "var before = 1");
		assert_eq!(h.eval(ctx, "while (true) {}"), -1);
		// 超时后 context 整体重建：引擎可用，但之前的全局状态被丢弃
		assert_eq!(h.eval_to_string(ctx, "1 + 1"), "2");
		let rid = h.get(ctx, "before");
		assert!(rid > 0);
		assert_eq!(h.stored_string(rid), "undefined");
	}

	#[test]
	fn loop_iteration_limit_stops_runaway_loop() {
		let limits = Limits { js_loop_max: 100_000, ..Limits::default() };
		let mut h = Harness::new(limits);
		let ctx = h.create();
		assert_eq!(h.eval(ctx, "for (let i = 0; i < 200_000; i++) {}"), -1);
		assert_eq!(h.eval_to_string(ctx, "1 + 1"), "2");
	}

	#[test]
	fn webview_eval_without_load_uses_blank_page() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		assert!(wv > 0);
		assert_eq!(h.wv_eval_to_string(wv, "1 + 2"), "3");
		assert_eq!(h.wv_eval_to_string(wv, "location.href"), "about:blank");
		assert_eq!(h.wv_eval_to_string(wv, "window === globalThis"), "true");
	}

	#[test]
	fn webview_load_html_runs_inline_scripts_in_order() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		let html = "<html><head><script>var token = 'tk' + 42;</script></head>\
			<body><script>var doubled = token + '!';</script></body></html>";
		assert_eq!(h.wv_load_html(wv, html, Some("https://example.com/c/1")), 0);
		assert_eq!(h.wv_eval_to_string(wv, "token"), "tk42");
		assert_eq!(h.wv_eval_to_string(wv, "doubled"), "tk42!");
		assert_eq!(h.wv_eval_to_string(wv, "location.href"), "https://example.com/c/1");
		// 再次 load_html 替换整个页面状态（对齐 loadHTMLString 的 document 替换语义）
		assert_eq!(h.wv_load_html(wv, "<script>var fresh = 1;</script>", None), 0);
		assert_eq!(h.wv_eval_to_string(wv, "typeof token"), "undefined");
		assert_eq!(h.wv_eval_to_string(wv, "location.href"), "about:blank");
	}

	#[test]
	fn webview_load_html_tolerates_script_errors() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		let html = "<script>throw new Error('x')</script><script>var after = 'ok';</script>";
		assert_eq!(h.wv_load_html(wv, html, None), 0);
		assert_eq!(h.wv_eval_to_string(wv, "after"), "ok");
	}

	#[test]
	fn webview_load_html_skips_external_and_non_js_scripts() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		let html = "<script src=\"https://example.com/a.js\">var srcRan = true;</script>\
			<script type=\"application/json\">var jsonRan = true;</script>\
			<script type=\"text/javascript\">var jsRan = true;</script>";
		assert_eq!(h.wv_load_html(wv, html, None), 0);
		assert_eq!(h.wv_eval_to_string(wv, "typeof srcRan"), "undefined");
		assert_eq!(h.wv_eval_to_string(wv, "typeof jsonRan"), "undefined");
		assert_eq!(h.wv_eval_to_string(wv, "jsRan"), "true");
	}

	#[test]
	fn webview_user_scripts_wrap_document_scripts() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		assert_eq!(h.wv_add_user_script(wv, "var marker = 's';", false), 0);
		assert_eq!(h.wv_add_user_script(wv, "marker += 'e';", true), 0);
		assert_eq!(h.wv_load_html(wv, "<script>var order = marker + 'p';</script>", None), 0);
		assert_eq!(h.wv_eval_to_string(wv, "order"), "sp");
		assert_eq!(h.wv_eval_to_string(wv, "marker"), "se");
	}

	#[test]
	fn webview_eval_async_resolves_promises() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		let rid = h.wv_eval_async(wv, "Promise.resolve(6 * 7)");
		assert!(rid > 0, "eval_async 返回 {rid}");
		assert_eq!(h.stored_string(rid), "42");
		assert_eq!(h.wv_eval_async(wv, "Promise.reject(new Error('no'))"), -1);
	}

	#[test]
	fn webview_wait_for_load_is_a_synchronous_noop() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		assert_eq!(h.wv_wait_for_load(wv), 0);
		assert_eq!(h.wv_load_html(wv, "<script>var x = 1;</script>", None), 0);
		assert_eq!(h.wv_wait_for_load(wv), 0);
		assert_eq!(h.wv_eval_to_string(wv, "x"), "1");
	}

	#[test]
	fn webview_unsupported_functions_return_missing_result() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		let (p, l) = h.write_str("[]");
		assert_eq!(webview_set_rule_list(h.env.clone().into_mut(&mut h.store), wv, p, l), -1);
		assert_eq!(webview_load(h.env.clone().into_mut(&mut h.store), wv, 9999), -1);
		assert_eq!(webview_get_cookies(h.env.clone().into_mut(&mut h.store), wv), -1);
		let (np, nl) = h.write_str("sid");
		assert_eq!(
			webview_delete_cookie(h.env.clone().into_mut(&mut h.store), wv, np, nl, 0, 0, 0, 0),
			-1
		);
	}

	#[test]
	fn webview_invalid_rid_maps_to_invalid_handler() {
		let mut h = Harness::new(Limits::default());
		let (p, l) = h.write_str("1 + 1");
		assert_eq!(webview_eval(h.env.clone().into_mut(&mut h.store), 9999, p, l), -4);
		assert_eq!(webview_eval_async(h.env.clone().into_mut(&mut h.store), 9999, p, l), -4);
		assert_eq!(webview_load_html(h.env.clone().into_mut(&mut h.store), 9999, p, l, 0, 0), -4);
		assert_eq!(webview_wait_for_load(h.env.clone().into_mut(&mut h.store), 9999), -4);
		assert_eq!(webview_add_user_script(h.env.clone().into_mut(&mut h.store), 9999, p, l, 0, 0), -4);
		assert_eq!(webview_set_rule_list(h.env.clone().into_mut(&mut h.store), 9999, p, l), -4);
		assert_eq!(webview_load(h.env.clone().into_mut(&mut h.store), 9999, 9999), -4);
		assert_eq!(webview_get_cookies(h.env.clone().into_mut(&mut h.store), 9999), -4);
		assert_eq!(webview_delete_cookie(h.env.clone().into_mut(&mut h.store), 9999, p, l, 0, 0, 0, 0), -4);
		// js context 与 webview 的 rid 不通用，跨种类使用各报各的错（对齐 iOS：
		// webview 侧 -4 InvalidHandler，context 侧 -2 InvalidContext）
		let ctx = h.create();
		assert_eq!(webview_eval(h.env.clone().into_mut(&mut h.store), ctx, p, l), -4);
		let wv = h.wv_create();
		assert_eq!(context_eval(h.env.clone().into_mut(&mut h.store), wv, p, l), -2);
	}

	#[test]
	fn webview_destroy_releases() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		h.destroy(wv);
		assert_eq!(h.wv_eval(wv, "1 + 1"), -4);
		assert_eq!(h.wv_wait_for_load(wv), -4);
	}

	#[test]
	fn webview_invalid_html_string_maps_to_invalid_string() {
		let mut h = Harness::new(Limits::default());
		let wv = h.wv_create();
		assert_eq!(
			webview_load_html(h.env.clone().into_mut(&mut h.store), wv, 0x7fff_fff0, 32, 0, 0),
			-3
		);
		assert_eq!(
			webview_eval(h.env.clone().into_mut(&mut h.store), wv, 0x7fff_fff0, 32),
			-3
		);
	}

	#[test]
	fn webview_loop_limit_stops_runaway_page_script() {
		let limits = Limits { js_loop_max: 100_000, ..Limits::default() };
		let mut h = Harness::new(limits);
		let wv = h.wv_create();
		let html = "<script>for (let i = 0; i < 200_000; i++) {}</script><script>var after = 'ok';</script>";
		assert_eq!(h.wv_load_html(wv, html, None), 0);
		assert_eq!(h.wv_eval_to_string(wv, "after"), "ok");
	}
}
