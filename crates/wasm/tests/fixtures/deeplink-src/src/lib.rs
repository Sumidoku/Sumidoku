#![no_std]
//! 深链路由（DeepLinkHandler）的端到端验证源（issue #137）：
//! https://deeplink.fixture/manga/{key} → Manga，
//! https://deeplink.fixture/manga/{manga_key}/chapter/{key} → Chapter，
//! https://deeplink.fixture/listing/{id} → Listing，其余 → None。
//! get_manga_update 固定回 c1/c2 两章，端点据此把 chapter key 解析成内部 id。
//! 断言方在 crates/server/tests/source_deep_link.rs 与 crates/wasm/tests/deep_link.rs。
use aidoku::{
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Listing, ListingProvider, Manga,
	MangaPageResult, Page, Result, Source,
	alloc::{String, Vec},
	prelude::*,
};

struct DeepLinkSource;

impl Source for DeepLinkSource {
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

	fn get_manga_update(&self, mut manga: Manga, needs_details: bool, needs_chapters: bool) -> Result<Manga> {
		if needs_details {
			manga.title = format!("Deep Manga {}", manga.key);
		}
		if needs_chapters {
			manga.chapters = Some(Vec::from([chapter("c1", 1.0), chapter("c2", 2.0)]));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

fn chapter(key: &str, number: f32) -> Chapter {
	let mut chapter = Chapter::default();
	chapter.key = String::from(key);
	chapter.title = Some(format!("Chapter {number}"));
	chapter.chapter_number = Some(number);
	chapter
}

impl DeepLinkHandler for DeepLinkSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(path) = url.strip_prefix("https://deeplink.fixture") else {
			return Ok(None);
		};
		let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
		match segments.as_slice() {
			["manga", key] => Ok(Some(DeepLinkResult::Manga {
				key: String::from(*key),
			})),
			["manga", manga_key, "chapter", key] => Ok(Some(DeepLinkResult::Chapter {
				manga_key: String::from(*manga_key),
				key: String::from(*key),
			})),
			["listing", id] => Ok(Some(DeepLinkResult::Listing(Listing {
				id: String::from(*id),
				name: String::from("Deep Listing"),
				kind: aidoku::ListingKind::Default,
			}))),
			_ => Ok(None),
		}
	}
}

register_source!(DeepLinkSource, DeepLinkHandler, ListingProvider);

impl ListingProvider for DeepLinkSource {
	fn get_manga_list(&self, _listing: Listing, _page: i32) -> Result<MangaPageResult> {
		let mut manga = Manga::default();
		manga.key = String::from("m1");
		manga.title = String::from("Deep Manga m1");
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}
}
