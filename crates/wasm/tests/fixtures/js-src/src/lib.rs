#![no_std]
//! js host 模块 context_* 四函数的端到端验证源：
//! get_page_description 把 eval/eval_async/get、context 隔离与错误码路径的
//! 调用结果拼成确定字符串，由 crates/wasm/tests/js_host.rs 断言全文。
//! 无效 rid 与 destroy 生命周期走 aidoku 封装之外的裸 FFI 声明验证。
use aidoku::{
	Chapter, FilterValue, Manga, MangaPageResult, Page, PageDescriptionProvider, Result, Source,
	alloc::{String, Vec},
	imports::js::{JsContext, JsError},
	prelude::*,
};

#[link(wasm_import_module = "js")]
unsafe extern "C" {
	fn context_create() -> i32;
	fn context_eval(context: i32, string_ptr: *const u8, len: usize) -> i32;
}

#[link(wasm_import_module = "std")]
unsafe extern "C" {
	#[link_name = "destroy"]
	fn std_destroy(rid: i32);
}

struct JsFixtureSource;

impl Source for JsFixtureSource {
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
		Err(_) => String::from("err:other"),
	}
}

fn raw_eval(context: i32, source: &str) -> i32 {
	unsafe { context_eval(context, source.as_ptr(), source.len()) }
}

impl PageDescriptionProvider for JsFixtureSource {
	fn get_page_description(&self, _page: Page) -> Result<String> {
		let mut out = String::new();
		let mut push = |part: String| {
			if !out.is_empty() {
				out.push_str(",");
			}
			out.push_str(&part);
		};

		let ctx = JsContext::new();
		push(show(ctx.eval("1 + 2")));
		push(show(ctx.eval("(function(){ return 'fn' + 1; })()")));
		push(show(ctx.eval("var token = 'tk' + 42; token")));
		push(show(ctx.get("token")));

		let other = JsContext::new();
		push(show(other.get("token")));

		push(show(ctx.eval("(")));

		push(show(ctx.eval_async("Promise.resolve(7 * 6)")));
		push(show(ctx.eval_async("(async () => { const x = await Promise.resolve('aw'); return x + 'ait'; })()")));
		push(show(ctx.eval_async("Promise.reject(new Error('no'))")));
		push(show(ctx.eval_async("new Promise(() => {})")));

		push(String::from(if raw_eval(9999, "1 + 1") == -2 { "raw:context" } else { "raw:bad" }));
		let raw = unsafe { context_create() };
		unsafe { std_destroy(raw) };
		push(String::from(if raw_eval(raw, "1 + 1") == -2 { "raw:destroyed" } else { "raw:bad" }));

		Ok(out)
	}
}

register_source!(JsFixtureSource, PageDescriptionProvider);
