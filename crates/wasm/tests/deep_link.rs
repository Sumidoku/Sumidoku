//! End-to-end tests for the DeepLinkHandler export (routing a source URL to the wasm side
//! of manga/chapter).
//! The fixture source lives in tests/fixtures/deeplink-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/deeplink-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_deeplink.wasm
//! over tests/fixtures/deeplink/main.wasm.
use aidoku_wasm::{DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

async fn spawn_fixture() -> (SourceHandle, aidoku_wasm::SourceFeatures) {
	let config = SourceConfig {
		key: "en.deeplink".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deeplink/main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	};
	SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source")
}

#[tokio::test]
async fn deep_link_maps_fixture_urls() {
	let (handle, features) = spawn_fixture().await;
	assert!(features.deep_link);

	let manga = handle
		.handle_deep_link("https://deeplink.fixture/manga/m1".into())
		.await
		.expect("manga link");
	match manga {
		Some(aidoku::DeepLinkResult::Manga { key }) => assert_eq!(key, "m1"),
		other => panic!("expected manga result, got {other:?}"),
	}

	let chapter = handle
		.handle_deep_link("https://deeplink.fixture/manga/m1/chapter/c2".into())
		.await
		.expect("chapter link");
	match chapter {
		Some(aidoku::DeepLinkResult::Chapter { manga_key, key }) => {
			assert_eq!(manga_key, "m1");
			assert_eq!(key, "c2");
		}
		other => panic!("expected chapter result, got {other:?}"),
	}

	let listing = handle
		.handle_deep_link("https://deeplink.fixture/listing/latest".into())
		.await
		.expect("listing link");
	match listing {
		Some(aidoku::DeepLinkResult::Listing(listing)) => assert_eq!(listing.id, "latest"),
		other => panic!("expected listing result, got {other:?}"),
	}

	// Once the deep link lands on a listing, the frontend fetches the list directly by it: the fixture must really yield entries
	let entries = handle
		.get_manga_list(
			aidoku::Listing {
				id: "latest".into(),
				name: "Deep Listing".into(),
				kind: aidoku::ListingKind::Default,
			},
			1,
		)
		.await
		.expect("manga list for linked listing");
	assert_eq!(entries.entries.len(), 1);
	assert_eq!(entries.entries[0].key, "m1");

	let unmatched = handle
		.handle_deep_link("https://other.fixture/manga/m1".into())
		.await
		.expect("foreign host");
	assert!(unmatched.is_none());

	let unknown_path = handle
		.handle_deep_link("https://deeplink.fixture/about".into())
		.await
		.expect("unknown path");
	assert!(unknown_path.is_none());
}
