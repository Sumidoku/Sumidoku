//! context_* 与 webview_* 的纯脚本子集由 Boa 实装（页面建模见 crate::webview）。
//! 配置了真浏览器后端（net.browser_ws 的 CDP sidecar 或 net.browser_trawl 的
//! trawl session API）时，webview_load/webview_get_cookies 走浏览器背衬
//! （crate::browser）；webview_eval/webview_eval_async 在后端支持时（trawl）
//! 同样走浏览器，CDP 后端与未配置时维持 Boa 页面模型。webview_delete_cookie
//! 仅 CDP 支持，trawl 无对应端点。未配置时这几个函数与 webview_set_rule_list
//! 一样显式不支持：返回 ABI 的 -1 家族错误码（JsError 没有 unsupported 专用码，
//! 上游 aidoku-rs 对这些函数也一律 -1），并在源日志点名函数与原因。
use crate::{
	WasmEnv,
	browser::{BrowserCookie, BrowserError},
	imports::CookieJar,
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
	InvalidRequest,
	FailedEncoding,
	MissingCookie,
}

impl From<Result> for i32 {
	fn from(result: Result) -> i32 {
		match result {
			Result::Success => 0,
			Result::MissingResult => -1,
			Result::InvalidContext => -2,
			Result::InvalidString => -3,
			Result::InvalidHandler => -4,
			Result::InvalidRequest => -5,
			Result::FailedEncoding => -7,
			Result::MissingCookie => -8,
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

pub fn webview_load(mut env: FunctionEnvMut<WasmEnv>, webview: Rid, request: Rid) -> FFIResult {
	if !env.data().browser.configured() {
		return unsupported(&mut env, webview, "webview_load", "no real page navigation on a server");
	}
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	let Some(req) = env.data_mut().store.get_mut(request).and_then(StoreItem::as_request) else {
		return Result::InvalidRequest.into();
	};
	let Some(url) = req.url.clone() else {
		return Result::InvalidRequest.into();
	};
	// CDP 的 Page.navigate 只有 GET；iOS 能整包 URLRequest（含 POST body）导航，
	// 服务器背衬诚实报失败，不静默退化成 GET
	if req.method != crate::imports::HttpMethod::Get {
		env.data().log("webview_load: only GET navigation is supported by the browser backend");
		return Result::MissingResult.into();
	}
	let headers: Vec<(String, String)> = req
		.headers
		.iter()
		.filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
		.collect();
	match env.data_mut().browser.load(webview, url.as_str(), &headers) {
		Ok(cookies) => {
			reflux(&mut env.data_mut().net.jar, &cookies);
			Result::Success.into()
		}
		Err(e) => browser_fail(&mut env, "webview_load", &e),
	}
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
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	};
	if env.data().browser.supports_eval() {
		return browser_eval(&mut env, webview, &source, "webview_eval");
	}
	let view = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).expect("rid 刚校验过");
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
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	};
	// 浏览器后端的 eval 由服务端 await promise，eval 与 eval_async 天然同一路径
	if env.data().browser.supports_eval() {
		return browser_eval(&mut env, webview, &source, "webview_eval_async");
	}
	let view = env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).expect("rid 刚校验过");
	match view.eval_async(&source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(msg) => fail(&mut env, msg),
	}
}

// 浏览器后端的 eval 路径（trawl）：值回传契约见 browser/trawl.rs
fn browser_eval(env: &mut FunctionEnvMut<WasmEnv>, webview: Rid, source: &str, name: &str) -> FFIResult {
	match env.data_mut().browser.eval(webview, source) {
		Ok(value) => env.data_mut().store.store(StoreItem::String(value)),
		Err(e) => browser_fail(env, name, &e),
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
	if !env.data().browser.configured() {
		return unsupported(&mut env, webview, "webview_get_cookies", "the sandbox has no cookie store");
	}
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	match env.data_mut().browser.cookies(webview) {
		Ok(cookies) => {
			reflux(&mut env.data_mut().net.jar, &cookies);
			let abi: Vec<AbiCookie> = cookies.into_iter().map(AbiCookie::from).collect();
			match env.data_mut().store.store_encoded(&abi) {
				Ok(rid) => rid,
				Err(_) => Result::FailedEncoding.into(),
			}
		}
		Err(e) => browser_fail(&mut env, "webview_get_cookies", &e),
	}
}

// 签名由源 ABI 固定，无法归并参数
#[allow(clippy::too_many_arguments)]
pub fn webview_delete_cookie(
	mut env: FunctionEnvMut<WasmEnv>,
	webview: Rid,
	name_ptr: Ptr,
	name_len: u32,
	value_ptr: Ptr,
	value_len: u32,
	domain_ptr: Ptr,
	domain_len: u32,
) -> FFIResult {
	if !env.data().browser.configured() {
		return unsupported(&mut env, webview, "webview_delete_cookie", "the sandbox has no cookie store");
	}
	if env.data_mut().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	// trawl 后端没有 cookie 编辑端点：与 webview_set_rule_list 同款的显式不支持
	if !env.data().browser.supports_cookie_edit() {
		env.data().log("webview_delete_cookie is not supported by the browser backend: trawl has no cookie-edit endpoint");
		return Result::MissingResult.into();
	}
	// ABI 约定 name_len == -1 为清空全 store（上游 delete_all_cookies 的入参）
	if name_len == u32::MAX {
		return match env.data_mut().browser.clear_cookies(webview) {
			Ok(()) => Result::Success.into(),
			Err(e) => browser_fail(&mut env, "webview_delete_cookie", &e),
		};
	}
	let read_opt = |env: &FunctionEnvMut<WasmEnv>, ptr: Ptr, len: u32| {
		if len == 0 { None } else { env.data().read_string(env, ptr, len).ok() }
	};
	let Some(name) = read_opt(&env, name_ptr, name_len) else {
		// 与 iOS 对齐：name 缺失且不是 -1 清空语义 → MissingCookie
		return Result::MissingCookie.into();
	};
	let value = read_opt(&env, value_ptr, value_len);
	let domain = read_opt(&env, domain_ptr, domain_len);
	let cookies = match env.data_mut().browser.cookies(webview) {
		Ok(cookies) => cookies,
		Err(e) => return browser_fail(&mut env, "webview_delete_cookie", &e),
	};
	// iOS 语义：按 name（可选 value/domain 收窄）取首个匹配删除，无匹配 -8。
	// value 只作选择器，CDP 删除按 name+domain+path 定位
	let found = cookies.iter().find(|c| {
		c.name == name
			&& value.as_deref().is_none_or(|v| c.value == *v)
			&& domain.as_deref().is_none_or(|d| c.domain == *d)
	});
	match found {
		Some(c) => {
			let (name, domain, path) = (c.name.clone(), c.domain.clone(), c.path.clone());
			match env.data_mut().browser.delete_cookie(webview, &name, Some(&domain), Some(&path)) {
				Ok(()) => Result::Success.into(),
				Err(e) => browser_fail(&mut env, "webview_delete_cookie", &e),
			}
		}
		None => Result::MissingCookie.into(),
	}
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

/// ABI Cookie 的镜像：字段顺序即 postcard 线序，改名/换序都会破 ABI
#[derive(serde::Serialize)]
struct AbiCookie {
	name: String,
	value: String,
	expires_date: Option<i64>,
	domain: String,
	path: String,
	is_secure: bool,
	is_http_only: bool,
}

impl From<BrowserCookie> for AbiCookie {
	fn from(cookie: BrowserCookie) -> Self {
		Self {
			name: cookie.name,
			value: cookie.value,
			expires_date: cookie.expires,
			domain: cookie.domain,
			path: cookie.path,
			is_secure: cookie.secure,
			is_http_only: cookie.http_only,
		}
	}
}

/// 浏览器会话 cookie 回流进源的会话 jar：登录流程成立的关键——webview_load
/// 打开登录页，登录完成后源 API 请求自动带会话 cookie。域名按 cookie 自身的
/// Domain 属性（去前导点）落库，与 jar 解析 Set-Cookie 的归属一致
fn reflux(jar: &mut CookieJar, cookies: &[BrowserCookie]) {
	for cookie in cookies {
		jar.insert(cookie.domain.trim_start_matches('.'), &cookie.name, &cookie.value);
	}
}

/// ABI 没有区分失败原因的错误码，浏览器失败统一 -1；细节进源日志
fn browser_fail(env: &mut FunctionEnvMut<WasmEnv>, name: &str, e: &BrowserError) -> FFIResult {
	let truncated: String = e.message.chars().take(200).collect();
	env.data().log(&format!("{name} failed: {truncated}"));
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
			Self::with_browser(limits, crate::browser::BrowserState::new(None, Duration::from_secs(1)))
		}

		fn with_browser(limits: Limits, browser: crate::browser::BrowserState) -> Self {
			let mut store = Store::default();
			let memory = Memory::new(&mut store, MemoryType::new(Pages(4), None, false)).unwrap();
			let env = FunctionEnv::new(
				&mut store,
				WasmEnv {
					memory: Some(memory.clone()),
					store: GlobalStore::new(),
					defaults: Defaults::new(HashMap::new(), Arc::new(NoopDefaultsSink), "test".into()),
					net: NetState::new(
						Arc::new(DirectPolicy { user_agent: None }),
						crate::imports::CookieJar::default(),
					),
					browser,
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

		fn new_request(&mut self, method: crate::imports::HttpMethod, url: Option<&str>) -> Rid {
			let mut req = crate::imports::NetRequest::new(method);
			req.url = url.map(|u| url::Url::parse(u).unwrap());
			self.env
				.as_mut(&mut self.store)
				.store
				.store(StoreItem::Request(Box::new(req)))
		}

		fn wv_load(&mut self, rid: Rid, request: Rid) -> FFIResult {
			webview_load(self.env.clone().into_mut(&mut self.store), rid, request)
		}

		fn wv_get_cookies(&mut self, rid: Rid) -> FFIResult {
			webview_get_cookies(self.env.clone().into_mut(&mut self.store), rid)
		}

		fn wv_delete_cookie(
			&mut self,
			rid: Rid,
			name: Option<&str>,
			name_len: Option<u32>,
			value: Option<&str>,
			domain: Option<&str>,
		) -> FFIResult {
			let (np, nl) = match name {
				Some(n) => self.write_str(n),
				None => (0, 0),
			};
			let (vp, vl) = match value {
				Some(v) => self.write_str(v),
				None => (0, 0),
			};
			let (dp, dl) = match domain {
				Some(d) => self.write_str(d),
				None => (0, 0),
			};
			webview_delete_cookie(
				self.env.clone().into_mut(&mut self.store),
				rid,
				np,
				name_len.unwrap_or(nl),
				vp,
				vl,
				dp,
				dl,
			)
		}

		fn jar_header(&self, url: &str) -> Option<String> {
			self.env
				.as_ref(&self.store)
				.net
				.jar
				.header_for(&url::Url::parse(url).unwrap())
		}
	}

	/// ABI Cookie 的解码镜像（编码侧 AbiCookie 字段顺序的逐字段对账）
	#[derive(Debug, serde::Deserialize, PartialEq)]
	struct WireCookie {
		name: String,
		value: String,
		expires_date: Option<i64>,
		domain: String,
		path: String,
		is_secure: bool,
		is_http_only: bool,
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

	fn browser_harness(mock: std::sync::Arc<std::sync::Mutex<crate::browser::mock::MockState>>) -> Harness {
		Harness::with_browser(
			Limits::default(),
			crate::browser::BrowserState::with_connector(
				Box::new(crate::browser::mock::MockConnector(mock)),
				Duration::from_secs(5),
			),
		)
	}

	fn decoded_cookies(h: &Harness, rid: Rid) -> Vec<WireCookie> {
		let Some(StoreItem::Encoded(bytes)) = h.env.as_ref(&h.store).store.get(rid) else {
			panic!("rid {rid} 不是 StoreItem::Encoded");
		};
		postcard::from_bytes(bytes).expect("cookie 列表 postcard 解码")
	}

	#[test]
	fn webview_browser_load_navigates_and_refluxes_cookies() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some("https://login.example/form"));
		// 登录页导航后浏览器侧有了会话 cookie
		mock.lock().expect("mock lock").cookies.insert(
			"ctx-1".into(),
			vec![crate::browser::mock::cookie("sid", "s3cret", ".login.example")],
		);

		assert_eq!(h.wv_load(wv, req), 0);
		{
			let mock = mock.lock().expect("mock lock");
			assert_eq!(mock.loads, vec![("ctx-1".to_string(), "https://login.example/form".to_string())]);
		}
		// 回流：源 API 请求自动带会话 cookie；Domain 前导点去掉后对子域名同样送出
		assert_eq!(h.jar_header("https://login.example/api"), Some("sid=s3cret".to_string()));
		assert_eq!(h.jar_header("https://www.login.example/api"), Some("sid=s3cret".to_string()));
		assert_eq!(h.jar_header("https://other.example/"), None);
	}

	#[test]
	fn webview_browser_load_maps_bad_request_to_invalid_request() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		assert_eq!(h.wv_load(wv, 9999), -5, "request rid 不存在");
		let no_url = h.new_request(crate::imports::HttpMethod::Get, None);
		assert_eq!(h.wv_load(wv, no_url), -5, "request 没有 url");
		assert!(mock.lock().expect("mock lock").loads.is_empty(), "无效请求不该触达浏览器");
	}

	#[test]
	fn webview_browser_load_rejects_post_navigation() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Post, Some("https://login.example/form"));
		assert_eq!(h.wv_load(wv, req), -1);
		assert!(mock.lock().expect("mock lock").loads.is_empty());
	}

	#[test]
	fn webview_browser_get_cookies_returns_abi_shape_and_refluxes() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		// 尚未 load 过的 webview：iOS 语义是返回空列表而非报错，context 此时才创建
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0, "空 cookie store 返回编码后的空列表: {rid}");
		assert_eq!(decoded_cookies(&h, rid), vec![]);

		let mut session = crate::browser::mock::cookie("sid", "s3cret", ".login.example");
		session.expires = Some(1_800_000_000);
		session.secure = true;
		session.http_only = true;
		mock.lock().expect("mock lock").cookies.insert("ctx-1".into(), vec![session]);
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0, "get_cookies 返回 {rid}");
		assert_eq!(
			decoded_cookies(&h, rid),
			vec![WireCookie {
				name: "sid".into(),
				value: "s3cret".into(),
				expires_date: Some(1_800_000_000),
				domain: ".login.example".into(),
				path: "/".into(),
				is_secure: true,
				is_http_only: true,
			}]
		);
		assert_eq!(h.jar_header("https://login.example/api"), Some("sid=s3cret".to_string()));
	}

	#[test]
	fn webview_browser_delete_cookie_matching_and_clear_all() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		mock.lock().expect("mock lock").cookies.insert(
			"ctx-1".into(),
			vec![
				crate::browser::mock::cookie("sid", "s3cret", ".login.example"),
				crate::browser::mock::cookie("theme", "dark", "login.example"),
			],
		);

		// 无匹配 → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("nope"), None, None, None), -8);
		// value 作选择器不匹配 → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, Some("wrong"), None), -8);
		// name 匹配 → 0，浏览器侧删除（定位带 domain+path）
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), 0);
		assert_eq!(
			mock.lock().expect("mock lock").deleted,
			vec![("ctx-1".to_string(), "sid".to_string(), Some(".login.example".to_string()), Some("/".to_string()))]
		);
		// 已删，再删 → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), -8);
		// name_len == -1 → 清空全 store
		assert_eq!(h.wv_delete_cookie(wv, None, Some(u32::MAX), None, None), 0);
		assert_eq!(mock.lock().expect("mock lock").cleared, vec!["ctx-1".to_string()]);
		assert!(mock.lock().expect("mock lock").cookies.is_empty());
	}

	#[test]
	fn webview_browser_destroy_closes_context() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some("https://login.example/"));
		assert_eq!(h.wv_load(wv, req), 0);
		h.destroy(wv);
		assert_eq!(mock.lock().expect("mock lock").disposed, vec!["ctx-1".to_string()]);
		// destroy 后 rid 失效，对齐 InvalidHandler
		assert_eq!(h.wv_get_cookies(wv), -4);
		assert_eq!(h.wv_load(wv, req), -4);
	}

	#[test]
	fn webview_browser_failures_map_to_missing_result() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		mock.lock().expect("mock lock").fail_connect = true;
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some("https://login.example/"));
		assert_eq!(h.wv_load(wv, req), -1, "连不上浏览器对外归一 -1");
		assert_eq!(h.wv_get_cookies(wv), -1);
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), -1);
		// 连接恢复后操作成功（重连语义在 browser.rs 单测覆盖）
		mock.lock().expect("mock lock").fail_connect = false;
		assert_eq!(h.wv_load(wv, req), 0);
	}

	#[test]
	fn webview_browser_two_rids_are_isolated() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		let mut h = browser_harness(mock.clone());
		let wv_a = h.wv_create();
		let wv_b = h.wv_create();
		{
			let mut mock = mock.lock().expect("mock lock");
			mock.cookies
				.insert("ctx-1".into(), vec![crate::browser::mock::cookie("sid", "a", "a.example")]);
			mock.cookies
				.insert("ctx-2".into(), vec![crate::browser::mock::cookie("sid", "b", "b.example")]);
		}
		let rid_a = h.wv_get_cookies(wv_a);
		let rid_b = h.wv_get_cookies(wv_b);
		assert!(rid_a > 0 && rid_b > 0);
		assert_eq!(decoded_cookies(&h, rid_a)[0].value, "a");
		assert_eq!(decoded_cookies(&h, rid_b)[0].value, "b");
	}

	#[test]
	fn webview_eval_routes_to_browser_when_backend_supports_it() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		{
			let mut state = mock.lock().expect("mock lock");
			state.supports_eval = true;
			state.eval_value = Some("tok42".into());
		}
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		let rid = h.wv_eval(wv, "window.token");
		assert!(rid > 0, "eval 返回 {rid}");
		assert_eq!(h.stored_string(rid), "tok42", "值来自浏览器后端而不是 Boa");
		let rid = h.wv_eval_async(wv, "fetch('/token').then(r => r.text())");
		assert!(rid > 0, "eval_async 返回 {rid}");
		assert_eq!(h.stored_string(rid), "tok42", "两个 FFI 天然映射到同一操作");
		let mock = mock.lock().expect("mock lock");
		assert_eq!(
			mock.evals,
			vec![
				("ctx-1".to_string(), "window.token".to_string()),
				("ctx-1".to_string(), "fetch('/token').then(r => r.text())".to_string()),
			],
			"eval 落在浏览器会话上"
		);
	}

	#[test]
	fn webview_eval_browser_failure_maps_to_missing_result() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		{
			let mut state = mock.lock().expect("mock lock");
			state.supports_eval = true;
			state.eval_error = Some(false);
		}
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		assert_eq!(h.wv_eval(wv, "nope()"), -1);
		assert_eq!(h.wv_eval_async(wv, "Promise.reject(1)"), -1);
	}

	#[test]
	fn webview_delete_cookie_unsupported_on_backend_without_cookie_edit() {
		let mock = std::sync::Arc::new(std::sync::Mutex::new(crate::browser::mock::MockState::default()));
		mock.lock().expect("mock lock").cookie_edit_unsupported = true;
		let mut h = browser_harness(mock.clone());
		let wv = h.wv_create();
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), -1, "单个删除显式不支持");
		assert_eq!(h.wv_delete_cookie(wv, None, Some(u32::MAX), None, None), -1, "清空同样不支持");
		let mock = mock.lock().expect("mock lock");
		assert!(mock.deleted.is_empty() && mock.cleared.is_empty(), "不触达浏览器");
	}

	// ---------- e2e：真 CDP，SUMIDOKU_TEST_BROWSER_WS 指向才跑 ----------

	type SeenRequests = std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

	/// 裸 HTTP/1.1 站点：/login?who=X 种会话 cookie，/api 记录 Cookie 头，
	/// 其余路径 404（浏览器会来抓 favicon）。每连接一个请求。
	struct MockSite {
		port: u16,
		seen: SeenRequests,
	}

	fn mock_site() -> MockSite {
		use std::io::{Read, Write};
		let listener = std::net::TcpListener::bind("0.0.0.0:0").expect("bind mock site");
		let port = listener.local_addr().expect("local addr").port();
		let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
		let seen2 = seen.clone();
		std::thread::spawn(move || {
			for stream in listener.incoming() {
				let Ok(mut stream) = stream else { break };
				let mut head = Vec::new();
				let mut chunk = [0u8; 8192];
				loop {
					match stream.read(&mut chunk) {
						Ok(0) | Err(_) => break,
						Ok(n) => {
							head.extend_from_slice(&chunk[..n]);
							if head.windows(4).any(|w| w == b"\r\n\r\n") {
								break;
							}
						}
					}
				}
				let head = String::from_utf8_lossy(&head).to_string();
				let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
				let cookie = head
					.lines()
					.find(|l| l.to_ascii_lowercase().starts_with("cookie:"))
					.map(|l| l[7..].trim().to_string());
				seen2.lock().expect("seen lock").push((path.clone(), cookie));
				let (status, extra, body) = if path.starts_with("/login") {
					let who = path.split("who=").nth(1).unwrap_or("alpha");
					(
						"200 OK",
						format!("set-cookie: sess={who}-secret; Path=/\r\ncontent-type: text/html\r\n"),
						format!("<html><body><form>login {who}</form></body></html>"),
					)
				} else if path == "/api" {
					("200 OK", "content-type: application/json\r\n".to_string(), "{}".to_string())
				} else {
					("404 Not Found", "content-type: text/plain\r\n".to_string(), "nf".to_string())
				};
				let response = format!(
					"HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}",
					body.len()
				);
				if stream.write_all(response.as_bytes()).is_err() {
					break;
				}
			}
		});
		MockSite { port, seen }
	}

	impl MockSite {
		fn base(&self) -> String {
			format!("http://{}:{}", e2e_site_host(), self.port)
		}

		fn cookie_for(&self, path: &str) -> Option<String> {
			self.seen
				.lock()
				.expect("seen lock")
				.iter()
				.find(|(p, _)| p == path)
				.and_then(|(_, c)| c.clone())
		}
	}

	/// 浏览器在容器里、测试进程在宿主机上：两边要用同一个地址访问 mock 站点，
	/// jar 才能按 host 匹配。容器经网关回连宿主机，UDP connect 探出宿主出口 IP；
	/// 探不到回退 host.docker.internal（Linux /docker 内运行时的惯用名）
	fn e2e_site_host() -> String {
		if let Ok(host) = std::env::var("SUMIDOKU_TEST_BROWSER_HOST")
			&& !host.is_empty()
		{
			return host;
		}
		if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
			&& sock.connect("192.168.5.2:53").is_ok()
			&& let Ok(addr) = sock.local_addr()
		{
			return addr.ip().to_string();
		}
		"host.docker.internal".to_string()
	}

	fn e2e_harness() -> Option<Harness> {
		let ws = std::env::var("SUMIDOKU_TEST_BROWSER_WS").ok().filter(|v| !v.is_empty())?;
		Some(Harness::with_browser(
			Limits::default(),
			crate::browser::BrowserState::new(Some(ws), Duration::from_secs(30)),
		))
	}

	#[test]
	fn e2e_webview_login_flow_cookies_reach_jar_and_requests() {
		let Some(mut h) = e2e_harness() else {
			eprintln!("SUMIDOKU_TEST_BROWSER_WS 未设置，跳过");
			return;
		};
		let site = mock_site();
		let base = site.base();
		let host = e2e_site_host();
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login")));
		assert_eq!(h.wv_load(wv, req), 0, "webview_load 真导航");
		// 导航后 cookie 已回流 jar
		assert_eq!(
			h.jar_header(&format!("{base}/api")),
			Some("sess=alpha-secret".to_string()),
			"登录 cookie 回流进 jar"
		);
		// get_cookies 读回 ABI 形状
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0, "get_cookies: {rid}");
		let cookies = decoded_cookies(&h, rid);
		assert!(
			cookies.iter().any(|c| c.name == "sess" && c.value == "alpha-secret" && c.domain == host),
			"cookie 形状: {cookies:?}"
		);
		// 源 API 请求自动带会话 cookie（net 栈从 jar 取）
		let api = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/api")));
		assert_eq!(crate::imports::net::send(h.env.clone().into_mut(&mut h.store), api), 0);
		assert_eq!(
			site.cookie_for("/api"),
			Some("sess=alpha-secret".to_string()),
			"API 请求带会话 cookie"
		);
	}

	#[test]
	fn e2e_webview_contexts_are_isolated() {
		let Some(mut h) = e2e_harness() else {
			eprintln!("SUMIDOKU_TEST_BROWSER_WS 未设置，跳过");
			return;
		};
		let site = mock_site();
		let base = site.base();
		let wv_a = h.wv_create();
		let wv_b = h.wv_create();
		let req_a = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login?who=alpha")));
		let req_b = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login?who=beta")));
		assert_eq!(h.wv_load(wv_a, req_a), 0);
		assert_eq!(h.wv_load(wv_b, req_b), 0);
		let rid_a = h.wv_get_cookies(wv_a);
		let rid_b = h.wv_get_cookies(wv_b);
		assert!(rid_a > 0 && rid_b > 0);
		let cookies_a = decoded_cookies(&h, rid_a);
		let cookies_b = decoded_cookies(&h, rid_b);
		assert!(
			cookies_a.iter().any(|c| c.name == "sess" && c.value == "alpha-secret")
				&& !cookies_a.iter().any(|c| c.value == "beta-secret"),
			"wv_a 只见自己的会话: {cookies_a:?}"
		);
		assert!(
			cookies_b.iter().any(|c| c.name == "sess" && c.value == "beta-secret")
				&& !cookies_b.iter().any(|c| c.value == "alpha-secret"),
			"wv_b 只见自己的会话: {cookies_b:?}"
		);
	}

	#[test]
	fn e2e_webview_delete_and_clear_cookies() {
		let Some(mut h) = e2e_harness() else {
			eprintln!("SUMIDOKU_TEST_BROWSER_WS 未设置，跳过");
			return;
		};
		let site = mock_site();
		let base = site.base();
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login")));
		assert_eq!(h.wv_load(wv, req), 0);

		assert_eq!(h.wv_delete_cookie(wv, Some("sess"), None, None, None), 0, "按 name 删除");
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0);
		assert!(
			!decoded_cookies(&h, rid).iter().any(|c| c.name == "sess"),
			"sess 已删: {:?}",
			decoded_cookies(&h, rid)
		);
		assert_eq!(h.wv_delete_cookie(wv, Some("sess"), None, None, None), -8, "已删再删无匹配");

		let req = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login")));
		assert_eq!(h.wv_load(wv, req), 0);
		assert_eq!(h.wv_delete_cookie(wv, None, Some(u32::MAX), None, None), 0, "清空全 store");
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0);
		assert!(decoded_cookies(&h, rid).is_empty(), "清空后为空: {:?}", decoded_cookies(&h, rid));
	}

	// ---------- e2e：trawl session 后端，SUMIDOKU_TEST_TRAWL_URL 指向才跑 ----------

	fn e2e_trawl_harness() -> Option<Harness> {
		let url = std::env::var("SUMIDOKU_TEST_TRAWL_URL").ok().filter(|v| !v.is_empty())?;
		Some(Harness::with_browser(
			Limits::default(),
			crate::browser::BrowserState::new_trawl("e2e", &url, Duration::from_secs(30)),
		))
	}

	#[test]
	fn e2e_trawl_webview_login_flow_cookies_reach_jar_and_requests() {
		let Some(mut h) = e2e_trawl_harness() else {
			eprintln!("SUMIDOKU_TEST_TRAWL_URL 未设置，跳过");
			return;
		};
		let site = mock_site();
		let base = site.base();
		let wv = h.wv_create();
		let req = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/login")));
		assert_eq!(h.wv_load(wv, req), 0, "webview_load 经 trawl 真导航");
		assert_eq!(
			h.jar_header(&format!("{base}/api")),
			Some("sess=alpha-secret".to_string()),
			"登录 cookie 回流进 jar"
		);
		let rid = h.wv_get_cookies(wv);
		assert!(rid > 0, "get_cookies: {rid}");
		assert!(
			decoded_cookies(&h, rid).iter().any(|c| c.name == "sess" && c.value == "alpha-secret"),
			"cookie 形状: {:?}",
			decoded_cookies(&h, rid)
		);
		// eval 打在驻留页的主 JS 环境：href 是刚导航的登录页
		let rid = h.wv_eval(wv, "location.href");
		assert!(rid > 0, "eval: {rid}");
		assert_eq!(h.stored_string(rid), format!("{base}/login"));
		// 源 API 请求自动带会话 cookie（net 栈从 jar 取）
		let api = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/api")));
		assert_eq!(crate::imports::net::send(h.env.clone().into_mut(&mut h.store), api), 0);
		assert_eq!(
			site.cookie_for("/api"),
			Some("sess=alpha-secret".to_string()),
			"API 请求带会话 cookie"
		);
		// destroy 立即 DELETE：新建的 webview 是干净 session
		h.destroy(wv);
		let wv2 = h.wv_create();
		let rid = h.wv_get_cookies(wv2);
		assert!(rid > 0);
		assert!(decoded_cookies(&h, rid).is_empty(), "新 session 无旧 cookie");
	}
}

