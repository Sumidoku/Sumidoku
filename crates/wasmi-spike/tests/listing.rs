//! listing fixture 端到端：断言面与 crates/wasm/tests/listing.rs 及
//! crates/server/tests/source_listing.rs 相同，wasmer/wasmi 两引擎跑同一套
use aidoku::{Chapter, ListingKind, Manga};
use wasmi_spike::wasmer_host::WasmerHost;
use wasmi_spike::wasmi_host::WasmiHost;
use wasmi_spike::{Host, HostExt, Limits, fixture_wasm};

fn listing(id: &str, name: &str, kind: ListingKind) -> aidoku::Listing {
	aidoku::Listing {
		id: id.into(),
		name: name.into(),
		kind,
	}
}

fn full_flow(host: &mut impl Host) {
	let result = host
		.get_manga_list(&listing("hot", "Hot Updates", ListingKind::List), 1)
		.expect("manga list");
	assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
	assert!(!result.has_next_page);

	let listings = host.get_listings().expect("dynamic listings");
	assert_eq!(listings.len(), 1);
	assert_eq!(listings[0].id, "dyn");
	assert_eq!(listings[0].name, "Dynamic Picks");
	assert!(matches!(listings[0].kind, ListingKind::List));

	let search = host
		.get_search_manga_list(
			Some("query".to_string()),
			1,
			vec![aidoku::FilterValue::Text {
				id: "author".into(),
				value: "someone".into(),
			}],
		)
		.expect("search list");
	assert_eq!(search.entries[0].title, "query|author=someone");

	let mut manga = Manga {
		key: "echo".into(),
		..Default::default()
	};
	let updated = host
		.get_manga_update(&manga, true, false)
		.expect("manga update");
	assert_eq!(updated.authors.as_deref(), Some(&["Author One".to_string()][..]));
	assert_eq!(updated.artists.as_deref(), Some(&["Artist One".to_string()][..]));
	assert_eq!(
		updated.tags.as_deref(),
		Some(&["Action".to_string(), "Fantasy".to_string()][..])
	);
	manga.authors = updated.authors.clone();

	let pages = host
		.get_page_list(&manga, &Chapter {
			key: "c1".into(),
			..Default::default()
		})
		.expect("page list");
	assert!(pages.is_empty());
}

#[test]
fn listing_fixture_passes_under_wasmi() {
	let mut host = WasmiHost::build(&fixture_wasm(), Limits::default()).expect("build wasmi host");
	full_flow(&mut host);
}

#[test]
fn listing_fixture_passes_under_wasmer_baseline() {
	let mut host = WasmerHost::build(&fixture_wasm(), Limits::default()).expect("build wasmer host");
	full_flow(&mut host);
}
