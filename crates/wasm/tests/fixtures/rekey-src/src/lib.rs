#![no_std]
//! breaking-change key 迁移的端到端验证源：
//! 搜索固定给出 old/ 前缀的两本 manga（外加 old/broken），章节 key 挂在 manga key 下；
//! MigrationHandler 把 old/ 前缀改写成 new/，old/broken 及其章节返回空串走 warnings 路径。
//! 断言方在 crates/server/tests/source_rekey.rs。
use aidoku::{
	alloc::{String, Vec, string::ToString, vec},
	prelude::*,
	Chapter, FilterValue, Manga, MangaPageResult, MigrationHandler, Page, Result, Source,
};

struct RekeySource;

/// old/ 前缀改 new/；old/broken 一支返回空串模拟映射失败，其余原样返回
fn remap(key: &str) -> String {
	if key.starts_with("old/broken") {
		return String::new();
	}
	match key.strip_prefix("old/") {
		Some(rest) => {
			let mut out = String::from("new/");
			out.push_str(rest);
			out
		}
		None => String::from(key),
	}
}

fn chapter_keys(manga_key: &str) -> Vec<Chapter> {
	vec![
		Chapter {
			key: {
				let mut k = manga_key.to_string();
				k.push_str("/ch-1");
				k
			},
			chapter_number: Some(1.0),
			..Chapter::default()
		},
		Chapter {
			key: {
				let mut k = manga_key.to_string();
				k.push_str("/ch-2");
				k
			},
			chapter_number: Some(2.0),
			..Chapter::default()
		},
	]
}

impl Source for RekeySource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		Ok(MangaPageResult {
			entries: vec![
				Manga {
					key: String::from("old/manga-1"),
					title: String::from("Rekey One"),
					..Manga::default()
				},
				Manga {
					key: String::from("old/manga-2"),
					title: String::from("Rekey Two"),
					..Manga::default()
				},
				Manga {
					key: String::from("old/broken"),
					title: String::from("Broken Three"),
					..Manga::default()
				},
			],
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		let mut manga = manga;
		if manga.title.is_empty() {
			manga.title = String::from("Rekey Untitled");
		}
		if needs_chapters {
			manga.chapters = Some(chapter_keys(&manga.key.clone()));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl MigrationHandler for RekeySource {
	fn handle_manga_migration(&self, key: String) -> Result<String> {
		Ok(remap(&key))
	}

	fn handle_chapter_migration(&self, _manga_key: String, chapter_key: String) -> Result<String> {
		Ok(remap(&chapter_key))
	}
}

register_source!(RekeySource, MigrationHandler);
