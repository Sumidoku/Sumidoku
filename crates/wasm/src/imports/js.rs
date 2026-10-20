//! The pure-script subset of context_* and webview_* is implemented with Boa (page
//! modeling: see crate::webview). With a trawl browser backend configured (the session
//! API of net.browser_trawl), webview_load/webview_get_cookies go through the browser
//! backing (crate::browser), and webview_eval/webview_eval_async are likewise routed into
//! the browser page; when unconfigured, the eval family keeps the Boa page model while
//! load/get_cookies, like webview_set_rule_list, are explicitly unsupported: they return
//! the ABI's -1 family of error codes (JsError has no dedicated unsupported code, and
//! upstream aidoku-rs also returns -1 for these functions) and name the function and
//! reason in the source log. webview_delete_cookie is always explicitly unsupported
//! (trawl has no cookie-edit endpoint).
use crate::{
	browser::{BrowserCookie, BrowserError},
	imports::CookieJar,
	js_engine::JsEngine,
	runtime::{
		env::Ctx,
		store::{FFIResult, Ptr, Rid, StoreItem},
	},
	webview::WebView,
};

// Variant names follow the ABI's JsError; naming is not this repo's call
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

pub fn context_create(ctx: &mut impl Ctx) -> Rid {
	let limits = &ctx.host().limits;
	let engine = JsEngine::new(limits.js_timeout, limits.js_loop_max);
	ctx.host().store.store(StoreItem::JsContext(Box::new(engine)))
}

pub fn context_eval(ctx: &mut impl Ctx, rid: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(source) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = ctx.host().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.eval(&source) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(msg) => fail(ctx, msg),
	}
}

pub fn context_eval_async(
	ctx: &mut impl Ctx,
	rid: Rid,
	string_ptr: Ptr,
	len: u32,
) -> FFIResult {
	let Ok(source) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = ctx.host().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.eval_async(&source) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(msg) => fail(ctx, msg),
	}
}

pub fn context_get(ctx: &mut impl Ctx, rid: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(name) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(engine) = ctx.host().store.get_mut(rid).and_then(StoreItem::as_js_context) else {
		return Result::InvalidContext.into();
	};
	match engine.get(&name) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(msg) => fail(ctx, msg),
	}
}

// The ABI has no error codes distinguishing failure causes, so everything is -1; details
// go to the source log (aligned with AidokuRunner's exceptionHandler printing), truncated
// so a source cannot flood the log with an overlong rejection value
fn fail(ctx: &mut impl Ctx, msg: String) -> FFIResult {
	let truncated: String = msg.chars().take(200).collect();
	ctx.host().log(&format!("js failed: {truncated}"));
	Result::MissingResult.into()
}

pub fn webview_create(ctx: &mut impl Ctx) -> Rid {
	let limits = &ctx.host().limits;
	let view = WebView::new(limits.js_timeout, limits.js_loop_max);
	ctx.host().store.store(StoreItem::WebView(Box::new(view)))
}

pub fn webview_set_rule_list(
	ctx: &mut impl Ctx,
	webview: Rid,
	_string_ptr: Ptr,
	_len: u32,
) -> FFIResult {
	unsupported(ctx, webview, "webview_set_rule_list", "content blocking only applies to real page loads")
}

pub fn webview_load(ctx: &mut impl Ctx, webview: Rid, request: Rid) -> FFIResult {
	if !ctx.host().browser.configured() {
		return unsupported(ctx, webview, "webview_load", "no real page navigation on a server");
	}
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	let Some(req) = ctx.host().store.get_mut(request).and_then(StoreItem::as_request) else {
		return Result::InvalidRequest.into();
	};
	let Some(url) = req.url.clone() else {
		return Result::InvalidRequest.into();
	};
	// The browser backing navigates with GET only (trawl scrape is fixed to GET); iOS can
	// navigate with a whole URLRequest (including a POST body). The server backing honestly
	// reports failure instead of silently degrading to GET
	if req.method != crate::imports::HttpMethod::Get {
		ctx.host().log("webview_load: only GET navigation is supported by the browser backend");
		return Result::MissingResult.into();
	}
	let headers: Vec<(String, String)> = req
		.headers
		.iter()
		.filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
		.collect();
	match ctx.host().browser.load(webview, url.as_str(), &headers) {
		Ok(cookies) => {
			reflux(&mut ctx.host().net.jar, &cookies);
			Result::Success.into()
		}
		Err(e) => browser_fail(ctx, "webview_load", &e),
	}
}

pub fn webview_load_html(
	ctx: &mut impl Ctx,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
	url_ptr: Ptr,
	url_len: u32,
) -> FFIResult {
	let Ok(html) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let base_url = if url_len > 0 {
		let Ok(url) = ctx.read_string(url_ptr, url_len) else {
			return Result::InvalidString.into();
		};
		Some(url)
	} else {
		None
	};
	let Some(view) = ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	let logs = view.load_html(&html, base_url.as_deref());
	for line in logs {
		ctx.host().log(&line);
	}
	Result::Success.into()
}

pub fn webview_wait_for_load(ctx: &mut impl Ctx, webview: Rid) -> FFIResult {
	// In the sandbox load_html completes synchronously; there is no async load to wait for
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	Result::Success.into()
}

pub fn webview_eval(ctx: &mut impl Ctx, webview: Rid, string_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(source) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	};
	if ctx.host().browser.supports_eval() {
		return browser_eval(ctx, webview, &source, "webview_eval");
	}
	let view = ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).expect("rid 刚校验过");
	match view.eval(&source) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(msg) => fail(ctx, msg),
	}
}

pub fn webview_eval_async(
	ctx: &mut impl Ctx,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
) -> FFIResult {
	let Ok(source) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	};
	// With the browser backend, eval awaits the promise server-side, so eval and eval_async naturally share one path
	if ctx.host().browser.supports_eval() {
		return browser_eval(ctx, webview, &source, "webview_eval_async");
	}
	let view = ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).expect("rid 刚校验过");
	match view.eval_async(&source) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(msg) => fail(ctx, msg),
	}
}

// The eval path of the browser backend (trawl): the value-return contract is in browser/trawl.rs
fn browser_eval(ctx: &mut impl Ctx, webview: Rid, source: &str, name: &str) -> FFIResult {
	match ctx.host().browser.eval(webview, source) {
		Ok(value) => ctx.host().store.store(StoreItem::String(value)),
		Err(e) => browser_fail(ctx, name, &e),
	}
}

pub fn webview_add_user_script(
	ctx: &mut impl Ctx,
	webview: Rid,
	string_ptr: Ptr,
	len: u32,
	at_document_end: i32,
	_for_main_frame_only: i32,
) -> FFIResult {
	let Ok(source) = ctx.read_string(string_ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(view) = ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview) else {
		return Result::InvalidHandler.into();
	};
	view.add_user_script(source, at_document_end != 0);
	Result::Success.into()
}

pub fn webview_get_cookies(ctx: &mut impl Ctx, webview: Rid) -> FFIResult {
	if !ctx.host().browser.configured() {
		return unsupported(ctx, webview, "webview_get_cookies", "the sandbox has no cookie store");
	}
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	match ctx.host().browser.cookies(webview) {
		Ok(cookies) => {
			reflux(&mut ctx.host().net.jar, &cookies);
			let abi: Vec<AbiCookie> = cookies.into_iter().map(AbiCookie::from).collect();
			match ctx.host().store.store_encoded(&abi) {
				Ok(rid) => rid,
				Err(_) => Result::FailedEncoding.into(),
			}
		}
		Err(e) => browser_fail(ctx, "webview_get_cookies", &e),
	}
}

// Optional string parameter: len == 0 means absent, otherwise read as UTF-8 (a failed read counts as absent)
fn read_opt(ctx: &mut impl Ctx, ptr: Ptr, len: u32) -> Option<String> {
	if len == 0 { None } else { ctx.read_string(ptr, len).ok() }
}

// The signature is fixed by the source ABI; parameters cannot be merged
#[allow(clippy::too_many_arguments)]
pub fn webview_delete_cookie(
	ctx: &mut impl Ctx,
	webview: Rid,
	name_ptr: Ptr,
	name_len: u32,
	value_ptr: Ptr,
	value_len: u32,
	domain_ptr: Ptr,
	domain_len: u32,
) -> FFIResult {
	if !ctx.host().browser.configured() {
		return unsupported(ctx, webview, "webview_delete_cookie", "the sandbox has no cookie store");
	}
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	// The trawl backend has no cookie-edit endpoint: explicitly unsupported, same as webview_set_rule_list
	if !ctx.host().browser.supports_cookie_edit() {
		ctx.host().log("webview_delete_cookie is not supported by the browser backend: trawl has no cookie-edit endpoint");
		return Result::MissingResult.into();
	}
	// The ABI defines name_len == -1 as clearing the whole store (the input of upstream delete_all_cookies)
	if name_len == u32::MAX {
		return match ctx.host().browser.clear_cookies(webview) {
			Ok(()) => Result::Success.into(),
			Err(e) => browser_fail(ctx, "webview_delete_cookie", &e),
		};
	}
	let Some(name) = read_opt(ctx, name_ptr, name_len) else {
		// Aligned with iOS: name missing and not the -1 clear semantic → MissingCookie
		return Result::MissingCookie.into();
	};
	let value = read_opt(ctx, value_ptr, value_len);
	let domain = read_opt(ctx, domain_ptr, domain_len);
	let cookies = match ctx.host().browser.cookies(webview) {
		Ok(cookies) => cookies,
		Err(e) => return browser_fail(ctx, "webview_delete_cookie", &e),
	};
	// iOS semantics: delete the first match by name (optionally narrowed by value/domain),
	// -8 on no match. value acts only as a selector; the browser-side delete locates by
	// name+domain+path
	let found = cookies.iter().find(|c| {
		c.name == name
			&& value.as_deref().is_none_or(|v| c.value == *v)
			&& domain.as_deref().is_none_or(|d| c.domain == *d)
	});
	match found {
		Some(c) => {
			let (name, domain, path) = (c.name.clone(), c.domain.clone(), c.path.clone());
			match ctx.host().browser.delete_cookie(webview, &name, Some(&domain), Some(&path)) {
				Ok(()) => Result::Success.into(),
				Err(e) => browser_fail(ctx, "webview_delete_cookie", &e),
			}
		}
		None => Result::MissingCookie.into(),
	}
}

// An unsupported function still validates the webview rid first (aligned with iOS's
// InvalidHandler priority); the other parameters are not validated: the operation itself
// is unsupported, so the parameters have no consumer
fn unsupported(ctx: &mut impl Ctx, webview: Rid, name: &str, why: &str) -> FFIResult {
	if ctx.host().store.get_mut(webview).and_then(StoreItem::as_webview).is_none() {
		return Result::InvalidHandler.into();
	}
	ctx.host().log(&format!("{name} is not supported by the sandbox: {why}"));
	Result::MissingResult.into()
}

/// Mirror of the ABI Cookie: the field order is the postcard wire order — renaming or reordering fields breaks the ABI
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

/// Browser-session cookies flow back into the source's session jar: the key to making the
/// login flow work — webview_load opens the login page, and once login completes the
/// source's API requests automatically carry the session cookie. The domain lands under
/// the cookie's own Domain attribute (leading dot stripped), consistent with how the jar
/// attributes a parsed Set-Cookie
fn reflux(jar: &mut CookieJar, cookies: &[BrowserCookie]) {
	for cookie in cookies {
		jar.insert(cookie.domain.trim_start_matches('.'), &cookie.name, &cookie.value);
	}
}

/// The ABI has no error codes distinguishing failure causes, so browser failures are uniformly -1; details go to the source log
fn browser_fail(ctx: &mut impl Ctx, name: &str, e: &BrowserError) -> FFIResult {
	let truncated: String = e.message.chars().take(200).collect();
	ctx.host().log(&format!("{name} failed: {truncated}"));
	Result::MissingResult.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::WasmEnv;
	use crate::runtime::{env::test_ctx::TestCtx, limits::Limits};
	use std::time::Duration;

	struct Harness {
		ctx: TestCtx,
	}

	impl Harness {
		fn new(limits: Limits) -> Self {
			Self::with_browser(limits, crate::browser::BrowserState::unconfigured(Duration::from_secs(1)))
		}

		fn with_browser(limits: Limits, browser: crate::browser::BrowserState) -> Self {
			Self {
				ctx: TestCtx::new(WasmEnv::for_test(limits, browser)),
			}
		}

		fn write_str(&mut self, s: &str) -> (u32, u32) {
			self.ctx.write_str(s)
		}

		fn create(&mut self) -> Rid {
			context_create(&mut self.ctx)
		}

		fn eval(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			context_eval(&mut self.ctx, rid, p, l)
		}

		fn eval_async(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			context_eval_async(&mut self.ctx, rid, p, l)
		}

		fn get(&mut self, rid: Rid, name: &str) -> FFIResult {
			let (p, l) = self.write_str(name);
			context_get(&mut self.ctx, rid, p, l)
		}

		fn destroy(&mut self, rid: Rid) {
			crate::imports::std::destroy(&mut self.ctx, rid);
		}

		fn eval_to_string(&mut self, rid: Rid, source: &str) -> String {
			let result = self.eval(rid, source);
			assert!(result > 0, "eval {source:?} 返回 {result}");
			self.stored_string(result)
		}

		fn stored_string(&self, rid: Rid) -> String {
			let Some(StoreItem::String(s)) = self.ctx.env.store.get(rid) else {
				panic!("rid {rid} 不是 StoreItem::String");
			};
			s.clone()
		}

		fn wv_create(&mut self) -> Rid {
			webview_create(&mut self.ctx)
		}

		fn wv_load_html(&mut self, rid: Rid, html: &str, url: Option<&str>) -> FFIResult {
			let (p, l) = self.write_str(html);
			let (up, ul) = match url {
				Some(u) => self.write_str(u),
				None => (0, 0),
			};
			webview_load_html(&mut self.ctx, rid, p, l, up, ul)
		}

		fn wv_eval(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_eval(&mut self.ctx, rid, p, l)
		}

		fn wv_eval_to_string(&mut self, rid: Rid, source: &str) -> String {
			let result = self.wv_eval(rid, source);
			assert!(result > 0, "webview eval {source:?} 返回 {result}");
			self.stored_string(result)
		}

		fn wv_eval_async(&mut self, rid: Rid, source: &str) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_eval_async(&mut self.ctx, rid, p, l)
		}

		fn wv_add_user_script(&mut self, rid: Rid, source: &str, at_document_end: bool) -> FFIResult {
			let (p, l) = self.write_str(source);
			webview_add_user_script(
				&mut self.ctx,
				rid,
				p,
				l,
				at_document_end as i32,
				0,
			)
		}

		fn wv_wait_for_load(&mut self, rid: Rid) -> FFIResult {
			webview_wait_for_load(&mut self.ctx, rid)
		}

		fn new_request(&mut self, method: crate::imports::HttpMethod, url: Option<&str>) -> Rid {
			let mut req = crate::imports::NetRequest::new(method);
			req.url = url.map(|u| url::Url::parse(u).unwrap());
			self.ctx.env.store.store(StoreItem::Request(Box::new(req)))
		}

		fn wv_load(&mut self, rid: Rid, request: Rid) -> FFIResult {
			webview_load(&mut self.ctx, rid, request)
		}

		fn wv_get_cookies(&mut self, rid: Rid) -> FFIResult {
			webview_get_cookies(&mut self.ctx, rid)
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
				&mut self.ctx,
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
			self.ctx
				.env
				.net
				.jar
				.header_for(&url::Url::parse(url).unwrap())
		}
	}

	/// Decoding mirror of the ABI Cookie (field-by-field reconciliation of the encoding side's AbiCookie field order)
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
		// A non-Promise value is equivalent to awaiting the raw value
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
		assert_eq!(context_eval(&mut h.ctx, 9999, p, l), -2);
		assert_eq!(context_eval_async(&mut h.ctx, 9999, p, l), -2);
		assert_eq!(context_get(&mut h.ctx, 9999, p, l), -2);
		let ctx = h.create();
		assert_eq!(context_eval(&mut h.ctx, ctx, 0x7fff_fff0, 32), -3);
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
		// After a timeout the context is rebuilt wholesale: the engine works, but the previous global state is gone
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
		// A second load_html replaces the whole page state (aligned with loadHTMLString's document-replacement semantics)
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
		assert_eq!(webview_set_rule_list(&mut h.ctx, wv, p, l), -1);
		assert_eq!(webview_load(&mut h.ctx, wv, 9999), -1);
		assert_eq!(webview_get_cookies(&mut h.ctx, wv), -1);
		let (np, nl) = h.write_str("sid");
		assert_eq!(
			webview_delete_cookie(&mut h.ctx, wv, np, nl, 0, 0, 0, 0),
			-1
		);
	}

	#[test]
	fn webview_invalid_rid_maps_to_invalid_handler() {
		let mut h = Harness::new(Limits::default());
		let (p, l) = h.write_str("1 + 1");
		assert_eq!(webview_eval(&mut h.ctx, 9999, p, l), -4);
		assert_eq!(webview_eval_async(&mut h.ctx, 9999, p, l), -4);
		assert_eq!(webview_load_html(&mut h.ctx, 9999, p, l, 0, 0), -4);
		assert_eq!(webview_wait_for_load(&mut h.ctx, 9999), -4);
		assert_eq!(webview_add_user_script(&mut h.ctx, 9999, p, l, 0, 0), -4);
		assert_eq!(webview_set_rule_list(&mut h.ctx, 9999, p, l), -4);
		assert_eq!(webview_load(&mut h.ctx, 9999, 9999), -4);
		assert_eq!(webview_get_cookies(&mut h.ctx, 9999), -4);
		assert_eq!(webview_delete_cookie(&mut h.ctx, 9999, p, l, 0, 0, 0, 0), -4);
		// rids are not interchangeable between js contexts and webviews; cross-kind use
		// reports each side's own error (aligned with iOS: -4 InvalidHandler on the webview
		// side, -2 InvalidContext on the context side)
		let ctx = h.create();
		assert_eq!(webview_eval(&mut h.ctx, ctx, p, l), -4);
		let wv = h.wv_create();
		assert_eq!(context_eval(&mut h.ctx, wv, p, l), -2);
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
			webview_load_html(&mut h.ctx, wv, 0x7fff_fff0, 32, 0, 0),
			-3
		);
		assert_eq!(
			webview_eval(&mut h.ctx, wv, 0x7fff_fff0, 32),
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
		let Some(StoreItem::Encoded(bytes)) = h.ctx.env.store.get(rid) else {
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
		// After navigating to the login page, the browser side has the session cookie
		mock.lock().expect("mock lock").cookies.insert(
			"ctx-1".into(),
			vec![crate::browser::mock::cookie("sid", "s3cret", ".login.example")],
		);

		assert_eq!(h.wv_load(wv, req), 0);
		{
			let mock = mock.lock().expect("mock lock");
			assert_eq!(mock.loads, vec![("ctx-1".to_string(), "https://login.example/form".to_string())]);
		}
		// Reflux: the source's API requests automatically carry the session cookie; with the Domain leading dot stripped, it is also sent to subdomains
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
		// A webview that has never loaded: iOS semantics are returning an empty list rather than an error; the context is only created now
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

		// No match → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("nope"), None, None, None), -8);
		// value as a selector does not match → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, Some("wrong"), None), -8);
		// name matches → 0, deleted on the browser side (located with domain+path)
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), 0);
		assert_eq!(
			mock.lock().expect("mock lock").deleted,
			vec![("ctx-1".to_string(), "sid".to_string(), Some(".login.example".to_string()), Some("/".to_string()))]
		);
		// Already deleted; deleting again → -8
		assert_eq!(h.wv_delete_cookie(wv, Some("sid"), None, None, None), -8);
		// name_len == -1 → clear the whole store
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
		// After destroy the rid is invalid, aligned with InvalidHandler
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
		// Operations succeed once the connection recovers (reconnect semantics are covered by the browser.rs unit tests)
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

	// ---------- Shared e2e fixtures: mock site and site address ----------

	type SeenRequests = std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

	/// Bare HTTP/1.1 site: /login?who=X plants a session cookie, /api records the Cookie
	/// header, all other paths 404 (the browser comes for the favicon). One request per
	/// connection.
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
						// trawl judges a page shorter than 100 chars as an empty-shell load failure, so the fixture page must be padded past that line
						format!("<html><head><title>sign in</title></head><body><h1>member login</h1><form method=\"post\" action=\"/login\">login {who}</form><p>fixture page padded past the scraper's empty-shell threshold</p></body></html>"),
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

	/// The browser runs in a container while the test process runs on the host: both sides
	/// must reach the mock site at the same address for the jar to match by host. The
	/// container reaches back to the host over the gateway, and a UDP connect probes the
	/// host's egress IP; if the probe fails, fall back to host.docker.internal (the
	/// customary name when running inside Linux /docker)
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


	// ---------- e2e: trawl session backend, runs only when SUMIDOKU_TEST_TRAWL_URL points at one ----------

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
		// eval lands in the resident page's main JS realm: href is the login page just navigated to
		let rid = h.wv_eval(wv, "location.href");
		assert!(rid > 0, "eval: {rid}");
		assert_eq!(h.stored_string(rid), format!("{base}/login"));
		// The source's API requests automatically carry the session cookie (the net stack takes it from the jar)
		let api = h.new_request(crate::imports::HttpMethod::Get, Some(&format!("{base}/api")));
		assert_eq!(crate::imports::net::send(&mut h.ctx, api), 0);
		assert_eq!(
			site.cookie_for("/api"),
			Some("sess=alpha-secret".to_string()),
			"API 请求带会话 cookie"
		);
		// destroy issues a DELETE immediately: a newly created webview is a clean session
		h.destroy(wv);
		let wv2 = h.wv_create();
		let rid = h.wv_get_cookies(wv2);
		assert!(rid > 0);
		assert!(decoded_cookies(&h, rid).is_empty(), "新 session 无旧 cookie");
	}
}

