#![no_std]
//! 服务端图片代理（/sources/{id}/image）测试的 fixture 源：
//! ImageRequestProvider 给图片请求加固定 Referer，断言代理走了源声明的请求规格。
use aidoku::{
	FilterValue, ImageRequestProvider, Manga, MangaPageResult, PageContext, Result, Source,
	alloc::{String, Vec},
	imports::net::Request,
	prelude::*,
};

/// 断言值与 crates/server/tests/source_image.rs 共享（mock 上游只认它）
pub const REFERER: &str = "https://imgreq.fixture";

struct ImgreqFixtureSource;

impl Source for ImgreqFixtureSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut manga = Manga::default();
		manga.key = String::from("imgreq-manga");
		manga.title = String::from("Imgreq Manga");
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: aidoku::Chapter) -> Result<Vec<aidoku::Page>> {
		Ok(Vec::new())
	}
}

impl ImageRequestProvider for ImgreqFixtureSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		Ok(Request::get(url)?.header("Referer", REFERER))
	}
}

register_source!(ImgreqFixtureSource, ImageRequestProvider);
