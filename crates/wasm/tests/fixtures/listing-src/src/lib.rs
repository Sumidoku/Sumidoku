#![no_std]
//! listing 透传（issue #138）的端到端验证源：get_manga_list 把收到的
//! listing 回显成标题 "{id}|{name}|{kind}"（kind 为 default/list），
//! 断言方据此确认端点传来的 id、name、kind 全部真实到达。
//! get_search_manga_list 回显 "{query}|{id=value,…}"，配合浏览 UI 冒烟
//! 验证 hidesFiltersWhileSearching 时搜索不再携带过滤器值。
//! get_manga_update 填 authors/artists/tags，供详情页 tag/作者搜索冒烟。
//! get_dynamic_listings 固定回一条 List 类型的动态 listing。
//! 断言方在 crates/server/tests/source_listing.rs 与 crates/wasm/tests/listing.rs。
use aidoku::{
	Chapter, DynamicListings, FilterValue, Listing, ListingKind, ListingProvider, Manga,
	MangaPageResult, Page, Result, Source,
	alloc::{String, Vec},
	prelude::*,
};

struct ListingSource;

impl Source for ListingSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		_page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut parts: Vec<String> = Vec::new();
		for filter in &filters {
			if let FilterValue::Text { id, value } = filter {
				parts.push(format!("{id}={value}"));
			}
		}
		let mut manga = Manga::default();
		manga.key = String::from("search-echo");
		manga.title = format!("{}|{}", query.unwrap_or_default(), parts.join(","));
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, mut manga: Manga, needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		if needs_details {
			manga.authors = Some(Vec::from([String::from("Author One")]));
			manga.artists = Some(Vec::from([String::from("Artist One")]));
			manga.tags = Some(Vec::from([String::from("Action"), String::from("Fantasy")]));
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

fn kind_name(kind: ListingKind) -> &'static str {
	match kind {
		ListingKind::Default => "default",
		ListingKind::List => "list",
	}
}

impl ListingProvider for ListingSource {
	fn get_manga_list(&self, listing: Listing, _page: i32) -> Result<MangaPageResult> {
		let mut manga = Manga::default();
		manga.key = String::from("echo");
		manga.title = format!("{}|{}|{}", listing.id, listing.name, kind_name(listing.kind));
		Ok(MangaPageResult {
			entries: Vec::from([manga]),
			has_next_page: false,
		})
	}
}

impl DynamicListings for ListingSource {
	fn get_dynamic_listings(&self) -> Result<Vec<Listing>> {
		Ok(Vec::from([Listing {
			id: String::from("dyn"),
			name: String::from("Dynamic Picks"),
			kind: ListingKind::List,
		}]))
	}
}

register_source!(ListingSource, ListingProvider, DynamicListings);
