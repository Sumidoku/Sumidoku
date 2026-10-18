//! context_* 由 QuickJS 实装；webview_* 在服务器上无 WebView 语义，保持未实装。
use crate::{
	WasmEnv,
	js_engine::JsEngine,
	runtime::store::{FFIResult, Ptr, Rid, StoreItem},
};
use wasmer::FunctionEnvMut;

const UNIMPLEMENTED: i32 = -1;

// 变体名与 ABI 的 JsError 对齐，命名不由本仓决定
#[allow(clippy::enum_variant_names)]
enum Result {
	MissingResult,
	InvalidContext,
	InvalidString,
}

impl From<Result> for i32 {
	fn from(result: Result) -> i32 {
		match result {
			Result::MissingResult => -1,
			Result::InvalidContext => -2,
			Result::InvalidString => -3,
		}
	}
}

pub fn context_create(mut env: FunctionEnvMut<WasmEnv>) -> Rid {
	let limits = &env.data().limits;
	match JsEngine::new(limits.js_memory_max, limits.js_timeout) {
		Ok(engine) => env.data_mut().store.store(StoreItem::JsContext(Box::new(engine))),
		Err(_) => Result::MissingResult.into(),
	}
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

pub fn webview_create(_env: FunctionEnvMut<WasmEnv>) -> Rid {
	UNIMPLEMENTED
}

pub fn webview_set_rule_list(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_load(_env: FunctionEnvMut<WasmEnv>, _webview: Rid, _request: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_load_html(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
	_url_ptr: Ptr,
	_url_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_wait_for_load(_env: FunctionEnvMut<WasmEnv>, _webview: Rid) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_eval(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_eval_async(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_add_user_script(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
	_at_document_end: i32,
	_for_main_frame_only: i32,
) -> FFIResult {
	UNIMPLEMENTED
}

pub fn webview_get_cookies(_env: FunctionEnvMut<WasmEnv>, _webview: Rid) -> FFIResult {
	UNIMPLEMENTED
}

// 签名由源 ABI 固定，无法归并参数
#[allow(clippy::too_many_arguments)]
pub fn webview_delete_cookie(
	_env: FunctionEnvMut<WasmEnv>,
	_webview: Rid,
	_name_ptr: Ptr,
	_name_len: u32,
	_value_ptr: Ptr,
	_value_len: u32,
	_domain_ptr: Ptr,
	_domain_len: u32,
) -> FFIResult {
	UNIMPLEMENTED
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
		assert_eq!(h.eval(ctx, "while (true) {}"), -1);
		// 中断后引擎仍可用
		assert_eq!(h.eval_to_string(ctx, "1 + 1"), "2");
	}

	#[test]
	fn memory_limit_aborts_huge_allocation() {
		let limits = Limits { js_memory_max: 8 * 1024 * 1024, ..Limits::default() };
		let mut h = Harness::new(limits);
		let ctx = h.create();
		assert_eq!(h.eval(ctx, "var s = 'x'; for (let i = 0; i < 100; i++) { s += s; } s.length"), -1);
		assert_eq!(h.eval_to_string(ctx, "1 + 1"), "2");
	}
}
