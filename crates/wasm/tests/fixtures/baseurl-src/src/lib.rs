#![no_std]
//! End-to-end verification source for the dynamic base URL chain (issue #136):
//! get_base_url returns the value of dynamic_url in defaults — the test injects it via
//! PUT settings, simulating a changed resolution after a source update; the default falls
//! back to a fixed value. The value "fail" bails, verifying that a failing export falls
//! back to the static URL inside the package. The asserting side is
//! crates/server/tests/source_base_url.rs.
use aidoku::{
	BaseUrlProvider, Chapter, FilterValue, Manga, MangaPageResult, Page, Result, Source,
	alloc::{String, Vec},
	imports::defaults::defaults_get,
	prelude::*,
};

struct BaseUrlSource;

impl Source for BaseUrlSource {
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

impl BaseUrlProvider for BaseUrlSource {
	fn get_base_url(&self) -> Result<String> {
		match defaults_get::<String>("dynamic_url").as_deref() {
			Some("fail") => bail!("base url export failed"),
			Some(url) => Ok(String::from(url)),
			None => Ok(String::from("https://dyn-v1.example")),
		}
	}
}

register_source!(BaseUrlSource, BaseUrlProvider);
