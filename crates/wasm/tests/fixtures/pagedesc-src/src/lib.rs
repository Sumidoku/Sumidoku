#![no_std]
//! 页描述管线的端到端验证源（issue #134）：
//! - ch-texts：回源取描述的 Text 页 + 无标志 Text 页 + 内联 description 的页
//!   （内联时宿主不得再调 get_page_description，本源对内联页的 content 恒 bail）
//! - ch-url：Url 内容页带 hasDescription，验证取描述与内容类型无关
//! - ch-fail：get_page_description 对该章页 bail，验证导出失败的错误面
//! 断言在 crates/server/tests/page_description.rs。
use aidoku::{
	Chapter, FilterValue, Manga, MangaPageResult, Page, PageContent, PageDescriptionProvider, Result, Source,
	alloc::{String, Vec},
	prelude::*,
};

struct PageDescSource;

fn chapter(key: &str, number: f32) -> Chapter {
	let mut chapter = Chapter::default();
	chapter.key = String::from(key);
	chapter.chapter_number = Some(number);
	chapter
}

fn text_page(body: &str, has_description: bool, description: Option<&str>) -> Page {
	Page {
		content: PageContent::Text(String::from(body)),
		has_description,
		description: description.map(String::from),
		..Page::default()
	}
}

impl Source for PageDescSource {
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
		manga.key = String::from("pagedesc-manga");
		manga.title = String::from("PageDesc Manga");
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, mut manga: Manga, needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		if needs_details {
			manga.title = String::from("PageDesc Manga");
		}
		if needs_chapters {
			manga.chapters = Some(Vec::from([
				chapter("ch-texts", 1.0),
				chapter("ch-url", 2.0),
				chapter("ch-fail", 3.0),
			]));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		Ok(match chapter.key.as_str() {
			"ch-url" => Vec::from([Page {
				content: PageContent::url("https://example.invalid/p0.png"),
				has_description: true,
				..Page::default()
			}]),
			"ch-fail" => Vec::from([text_page("desc boom", true, None)]),
			_ => Vec::from([
				text_page("page zero body", true, None),
				text_page("plain body", false, None),
				text_page("inline body", true, Some("inline description")),
			]),
		})
	}
}

impl PageDescriptionProvider for PageDescSource {
	fn get_page_description(&self, page: Page) -> Result<String> {
		match &page.content {
			PageContent::Text(text) => {
				if text == "desc boom" || text == "inline body" {
					bail!("description export failed")
				}
				Ok(format!("fetched: {text}"))
			}
			PageContent::Url(url, _) => Ok(format!("fetched: {url}")),
			_ => bail!("unexpected page content"),
		}
	}
}

register_source!(PageDescSource, PageDescriptionProvider);
