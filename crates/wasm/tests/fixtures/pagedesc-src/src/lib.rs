#![no_std]
//! End-to-end verification source for the page-description pipeline (issue #134):
//! - ch-texts: a Text page whose description is fetched from the source + an unflagged
//!   Text page + a page with inline description (when inline, the host must not call
//!   get_page_description again; this source always bails on the inline page's content)
//! - ch-url: a Url content page carrying hasDescription, verifying description fetching is
//!   independent of content type
//! - ch-fail: get_page_description bails on this chapter's pages, verifying the error
//!   surface of a failing export
//! Assertions live in crates/server/tests/page_description.rs.
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
