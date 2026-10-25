//! End-to-end tests for the MigrationHandler export (the wasm side of breaking-change key
//! migration).
//! The fixture source lives in tests/fixtures/rekey-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/rekey-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_rekey.wasm
//! over tests/fixtures/rekey/main.wasm.
use aidoku_wasm::{DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

async fn spawn_fixture() -> (SourceHandle, aidoku_wasm::SourceFeatures) {
	let config = SourceConfig {
		key: "en.rekey".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rekey/main.wasm"),
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
async fn key_migration_maps_old_prefix() {
	let (handle, features) = spawn_fixture().await;
	assert!(features.migration);

	let migrated = handle
		.handle_key_migration(0, "old/manga-1".into(), None)
		.await
		.expect("manga migration");
	assert_eq!(migrated, "new/manga-1");

	let chapter = handle
		.handle_key_migration(1, "old/manga-1".into(), Some("old/manga-1/ch-2".into()))
		.await
		.expect("chapter migration");
	assert_eq!(chapter, "new/manga-1/ch-2");

	// The old/broken branch returns an empty string: the server keeps the row on its old key and puts it into warnings
	let broken = handle
		.handle_key_migration(0, "old/broken".into(), None)
		.await
		.expect("broken migration");
	assert_eq!(broken, "");

	// Keys without the old/ prefix are returned as-is: the server treats them as no-ops and skips
	let untouched = handle
		.handle_key_migration(0, "new/manga-1".into(), None)
		.await
		.expect("no-op migration");
	assert_eq!(untouched, "new/manga-1");
}
