#![no_std]
//! js host 模块 webview_* 沙箱语义的端到端验证源：
//! get_page_description 把 load_html/eval/user script/不支持函数错误码与
//! rid 生命周期的调用结果拼成确定字符串，由 crates/wasm/tests/webview_host.rs
//! 断言全文。无效 rid、跨种类 rid 与 destroy 生命周期走裸 FFI 声明验证。
use aidoku::{
	Chapter, FilterValue, Manga, MangaPageResult, Page, PageDescriptionProvider, Result, Source,
	alloc::{String, Vec},
	imports::{
		js::{Cookie, JsError, WebView, WebViewUserScript},
		net::Request,
	},
	prelude::*,
};

#[link(wasm_import_module = "js")]
unsafe extern "C" {
	fn context_create() -> i32;
	fn context_eval(context: i32, string_ptr: *const u8, len: usize) -> i32;
	fn webview_create() -> i32;
	fn webview_eval(webview: i32, string_ptr: *const u8, len: usize) -> i32;
}

#[link(wasm_import_module = "std")]
unsafe extern "C" {
	#[link_name = "destroy"]
	fn std_destroy(rid: i32);
}

struct WebViewFixtureSource;

impl Source for WebViewFixtureSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		bail!("not used")
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

fn show(result: core::result::Result<String, JsError>) -> String {
	match result {
		Ok(value) => value,
		Err(JsError::MissingResult) => String::from("err:missing"),
		Err(JsError::InvalidContext) => String::from("err:context"),
		Err(JsError::InvalidString) => String::from("err:string"),
		Err(JsError::InvalidHandler) => String::from("err:handler"),
		Err(_) => String::from("err:other"),
	}
}

fn show_unit(result: core::result::Result<(), JsError>) -> String {
	match result {
		Ok(()) => String::from("ok"),
		Err(error) => show(Err(error)),
	}
}

fn show_cookies(result: core::result::Result<Vec<Cookie>, aidoku::AidokuError>) -> String {
	match result {
		Ok(_) => String::from("cookies"),
		Err(aidoku::AidokuError::JsError(error)) => show(Err(error)),
		Err(_) => String::from("err:other"),
	}
}

fn raw_webview_eval(webview: i32, source: &str) -> i32 {
	unsafe { webview_eval(webview, source.as_ptr(), source.len()) }
}

impl PageDescriptionProvider for WebViewFixtureSource {
	fn get_page_description(&self, _page: Page) -> Result<String> {
		let mut out = String::new();
		let mut push = |part: String| {
			if !out.is_empty() {
				out.push_str(",");
			}
			out.push_str(&part);
		};

		let wv = WebView::new();
		push(show(wv.eval("1 + 2")));

		let html = "<html><head><script>var token = 'tk' + 42;</script></head>\
			<body><script>var doubled = token + '!';</script></body></html>";
		push(show_unit(wv.load_html(html, Some("https://example.com/chapter/1"))));
		wv.wait_for_load();
		push(show(wv.eval("token")));
		push(show(wv.eval("doubled")));
		push(show(wv.eval("location.href")));
		push(show(wv.eval("window === globalThis")));

		// 页面脚本抛错不影响加载结果与后续脚本
		push(show_unit(wv.load_html(
			"<script>throw new Error('x')</script><script>var after = 'ok';</script>",
			None,
		)));
		push(show(wv.eval("after")));
		// 再次 load_html 替换整个页面状态
		push(show(wv.eval("typeof token")));
		push(show(wv.eval("location.href")));

		// 外链脚本与非 JS type 的脚本不执行
		push(show_unit(wv.load_html(
			"<script src=\"https://example.com/a.js\">var srcRan = true;</script>\
				<script type=\"application/json\">var jsonRan = true;</script>\
				<script>var inl = 'yes';</script>",
			None,
		)));
		push(show(wv.eval("typeof srcRan")));
		push(show(wv.eval("typeof jsonRan")));
		push(show(wv.eval("inl")));

		// user script 分别在页面脚本前/后执行
		let _ = wv.add_user_script(WebViewUserScript::new(String::from("var marker = 's';")));
		let end_script = WebViewUserScript {
			source: String::from("marker += 'e';"),
			at_document_end: true,
			for_main_frame_only: false,
		};
		let _ = wv.add_user_script(end_script);
		push(show_unit(wv.load_html("<script>var order = marker + 'p';</script>", None)));
		push(show(wv.eval("order")));
		push(show(wv.eval("marker")));

		push(show(wv.eval_async("Promise.resolve(6 * 7)")));
		push(show(wv.eval_async("Promise.reject(new Error('no'))")));

		// 需要真实页面加载/导航/cookie 的函数显式不支持
		push(show_unit(wv.set_rule_list("[]")));
		let request = Request::get("https://example.com")?;
		push(show_unit(wv.load(request)));
		push(show_cookies(wv.get_cookies()));
		push(show_unit(wv.delete_all_cookies()));

		push(show(wv.eval("(")));

		// 裸 FFI：无效 rid 与跨种类 rid 的错误码
		push(String::from(if raw_webview_eval(9999, "1") == -4 { "raw:handler" } else { "raw:bad" }));
		let ctx = unsafe { context_create() };
		push(String::from(if raw_webview_eval(ctx, "1") == -4 { "raw:cross" } else { "raw:bad" }));
		let raw_wv = unsafe { webview_create() };
		push(String::from(
			if unsafe { context_eval(raw_wv, "1".as_ptr(), 1) } == -2 { "raw:crossctx" } else { "raw:bad" },
		));
		unsafe { std_destroy(raw_wv) };
		push(String::from(if raw_webview_eval(raw_wv, "1") == -4 { "raw:destroyed" } else { "raw:bad" }));

		Ok(out)
	}
}

register_source!(WebViewFixtureSource, PageDescriptionProvider);
