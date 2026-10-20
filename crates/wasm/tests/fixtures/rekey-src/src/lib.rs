#![no_std]
//! End-to-end verification source for breaking-change key migration:
//! search always returns two mangas with the old/ prefix (plus old/broken), with chapter
//! keys nested under the manga key; MigrationHandler rewrites the old/ prefix to new/,
//! while old/broken and its chapters return empty strings to take the warnings path.
//! The asserting side is crates/server/tests/source_rekey.rs.
use aidoku::{
	alloc::{String, Vec, string::ToString, vec},
	prelude::*,
	Chapter, FilterValue, Manga, MangaPageResult, MigrationHandler, Page, Result, Source,
};

struct RekeySource;

/// Rewrite the old/ prefix to new/; the old/broken branch returns an empty string to simulate a mapping failure, everything else passes through as-is
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
