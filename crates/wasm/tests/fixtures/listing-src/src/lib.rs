#![no_std]
//! End-to-end verification source for listing pass-through (issue #138): get_manga_list
//! echoes the received listing as the title "{id}|{name}|{kind}" (kind is default/list),
//! so the asserting side can confirm the endpoint's id, name, and kind all truly arrived;
//! the echoed manga carries authors/tags for the browse page's row layout (list kind)
//! smoke to assert the subtitle and tags row. get_search_manga_list echoes
//! "{query}|{id=value,…}", which the browse UI smoke uses to verify that with
//! hidesFiltersWhileSearching, searches no longer carry filter values. get_manga_update
//! fills authors/artists/tags for the detail page's tag/author search smoke.
//! get_dynamic_listings always returns one dynamic listing of List kind.
//! The asserting sides are crates/server/tests/source_listing.rs and crates/wasm/tests/listing.rs.
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
		manga.authors = Some(Vec::from([String::from("Author One")]));
		manga.tags = Some(Vec::from([String::from("Action"), String::from("Fantasy")]));
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
