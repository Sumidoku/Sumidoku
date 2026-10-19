use aidoku_wasm::{
	DefaultValue, DirectPolicy, Error, Limits, NoopCookieSink, NoopDefaultsSink, SourceConfig,
	SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

fn fixture_wasm() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/example/main.wasm")
}

fn config(key: &str, defaults: HashMap<String, DefaultValue>) -> SourceConfig {
	SourceConfig {
		key: key.to_string(),
		wasm_path: fixture_wasm(),
		limits: Limits::default(),
		defaults,
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_ws: None,
		browser_trawl: None,
	}
}

async fn spawn(key: &str, defaults: HashMap<String, DefaultValue>) -> SourceHandle {
	let (handle, _) = SourceHandle::spawn(config(key, defaults), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	handle
}

#[tokio::test]
async fn features_detected() {
	let (handle, features) = SourceHandle::spawn(config("en.example", HashMap::new()), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn");
	assert!(features.manga_list);
	assert!(features.home);
	assert!(features.dynamic_filters);
	assert!(features.dynamic_settings);
	assert!(features.dynamic_listings);
	assert!(features.notification);
	assert!(features.deep_link);
	assert!(features.migration);
	assert!(features.alternate_covers);
	assert!(features.page_description);
	assert!(features.cover_image_processor);
	assert!(!features.page_image_processor);
	assert!(!features.image_request);
	assert!(!features.base_url);
	assert!(handle.is_alive());
}

#[tokio::test]
async fn search_details_pages_flow() {
	let handle = spawn("en.example", HashMap::new()).await;

	let result = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect("search");
	assert_eq!(result.entries.len(), 20);
	assert!(result.has_next_page);
	assert_eq!(result.entries[0].title, "Manga 1");

	let filtered = handle
		.get_search_manga_list(Some("Manga 3".into()), 1, Vec::new())
		.await
		.expect("search with query");
	assert!(!filtered.entries.is_empty());
	assert!(filtered.entries.iter().all(|m| m.title.contains("Manga 3")));

	let manga = filtered.entries[0].clone();
	let updated = handle
		.get_manga_update(manga.clone(), false, true)
		.await
		.expect("manga update");
	let chapters = updated.chapters.expect("chapters");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0].chapter_number, Some(8.0));

	let pages = handle
		.get_page_list(manga, chapters[0].clone())
		.await
		.expect("page list");
	assert_eq!(pages.len(), 2);
	match &pages[0].content {
		aidoku::PageContent::Url(url, _) => assert_eq!(url, "https://aidoku.app/images/icon.png"),
		other => panic!("expected url page, got {other:?}"),
	}
	match &pages[1].content {
		aidoku::PageContent::Text(md) => assert!(md.starts_with("# Title")),
		other => panic!("expected text page, got {other:?}"),
	}

	let desc = handle
		.get_page_description(aidoku::Page::default())
		.await
		.expect("page description");
	assert!(desc.starts_with("# Title"));
}

#[tokio::test]
async fn listings_home_filters() {
	let handle = spawn("en.example", HashMap::new()).await;

	let listings = handle.get_listings().await.expect("listings");
	assert_eq!(listings.len(), 1);
	assert_eq!(listings[0].id, "listing");

	let listed = handle
		.get_manga_list(listings[0].clone(), 1)
		.await
		.expect("manga list");
	assert_eq!(listed.entries.len(), 1);

	let unsupported = handle
		.get_manga_list(
			aidoku::Listing {
				id: "test".into(),
				name: "Test".into(),
				kind: aidoku::ListingKind::Default,
			},
			1,
		)
		.await;
	match unsupported {
		Err(Error::Message(msg)) => assert!(msg.contains("Not supported"), "got: {msg}"),
		other => panic!("expected message error, got {other:?}"),
	}

	let home = handle.get_home().await.expect("home");
	assert_eq!(home.components.len(), 7);

	let filters = handle.get_filters().await.expect("filters");
	assert_eq!(filters.len(), 7);

	let covers = handle
		.get_alternate_covers(aidoku::Manga::default())
		.await
		.expect("alternate covers");
	assert_eq!(covers.len(), 1);

	let linked = handle
		.handle_deep_link("https://example.local/manga/1".into())
		.await
		.expect("deep link");
	assert!(matches!(
		linked,
		Some(aidoku::DeepLinkResult::Manga { .. })
	));

	let migrated = handle
		.handle_key_migration(0, "abc".into(), None)
		.await
		.expect("key migration");
	assert_eq!(migrated, "/abc");

	handle
		.handle_notification("test".into())
		.await
		.expect("notification");
}

#[tokio::test]
async fn settings_reflect_defaults_and_restart() {
	let mut defaults = HashMap::new();
	defaults.insert("setting".to_string(), DefaultValue::Bool(true));
	let handle = spawn("en.example", defaults).await;

	let settings = handle.get_settings().await.expect("settings");
	assert_eq!(settings.len(), 2, "setting=true 时应多出 setting2");

	let mut off = HashMap::new();
	off.insert("setting".to_string(), DefaultValue::Bool(false));
	let features = handle.restart(off).await.expect("restart");
	assert!(features.dynamic_settings);
	let settings = handle.get_settings().await.expect("settings after restart");
	assert_eq!(settings.len(), 1, "setting=false 时只剩一个开关");
}

#[tokio::test]
async fn parallel_sources() {
	let a = spawn("en.a", HashMap::new()).await;
	let b = spawn("en.b", HashMap::new()).await;
	let (ra, rb) = tokio::join!(
		a.get_search_manga_list(None, 1, Vec::new()),
		b.get_search_manga_list(None, 2, Vec::new()),
	);
	assert_eq!(ra.expect("a").entries.len(), 20);
	assert_eq!(rb.expect("b").entries.len(), 20);
}
