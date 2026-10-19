#![no_std]
//! 备选封面（AlternateCoverProvider）管线的端到端验证源（issue #135）：
//! 默认返回 3 个固定 https://altcovers.fixture/cover-N.png 供离线断言；
//! defaults 里给了 coverBase 时改返回 {coverBase}/cover-N.png，端点测试与
//! 手动冒烟据此把备选指到本地 mock 图床，跑通选完回源取图的全链。
//! 断言在 crates/server/tests/alternate_covers.rs。
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
