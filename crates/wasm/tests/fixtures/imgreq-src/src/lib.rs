#![no_std]
//! Fixture source for testing the server-side image proxy (/sources/{id}/image):
//! ImageRequestProvider adds a fixed Referer to image requests, asserting the proxy used
//! the request spec the source declared.
use aidoku::{
	FilterValue, ImageRequestProvider, Manga, MangaPageResult, PageContext, Result, Source,
	alloc::{String, Vec},
	imports::net::Request,
	prelude::*,
};

/// The asserted value is shared with crates/server/tests/source_image.rs (the mock upstream only accepts it)
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
