#![no_std]
//! End-to-end verification source for the alternate-cover (AlternateCoverProvider)
//! pipeline (issue #135): by default it returns 3 fixed
//! https://altcovers.fixture/cover-N.png for offline assertions; when coverBase is set in
//! defaults it returns {coverBase}/cover-N.png instead, which the endpoint test and the
//! manual smoke use to point alternates at a local mock image host and run the full chain
//! of picking one and fetching it from the source.
//! Assertions live in crates/server/tests/alternate_covers.rs.
use aidoku::{
	AlternateCoverProvider, Chapter, FilterValue, Manga, MangaPageResult, Page, Result, Source,
	alloc::{String, Vec},
	imports::defaults::defaults_get,
	prelude::*,
};

struct AltCoversSource;

impl Source for AltCoversSource {
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
		manga.key = String::from("altcovers-manga");
		manga.title = String::from("AltCovers Manga");
		manga.cover = Some(String::from("https://altcovers.fixture/cover-0.png"));
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, mut manga: Manga, needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		if needs_details {
			manga.title = String::from("AltCovers Manga");
			manga.cover = Some(String::from("https://altcovers.fixture/cover-0.png"));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl AlternateCoverProvider for AltCoversSource {
	fn get_alternate_covers(&self, _manga: Manga) -> Result<Vec<String>> {
		let base = defaults_get::<String>("coverBase")
			.unwrap_or_else(|| String::from("https://altcovers.fixture"));
		Ok(Vec::from([
			format!("{base}/cover-1.png"),
			format!("{base}/cover-2.png"),
			format!("{base}/cover-3.png"),
		]))
	}
}

register_source!(AltCoversSource, AlternateCoverProvider);
