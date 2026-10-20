//! The listing-echo fixture and the per-source concurrency cap (config.maximumParallelRequests, issue #138).
//! The fixture source lives in crates/wasm/tests/fixtures/listing-src (no_std cdylib); rebuild with:
//! `cd crates/wasm/tests/fixtures/listing-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_listing.wasm
//! over crates/wasm/tests/fixtures/listing/main.wasm.
use aidoku_wasm::{
	DirectPolicy, Limits, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

fn fixture_wasm() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/listing/main.wasm")
}

fn config() -> SourceConfig {
	SourceConfig {
		key: "en.listing".to_string(),
		wasm_path: fixture_wasm(),
		limits: Limits::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	}
}

fn listing(id: &str, name: &str, kind: aidoku::ListingKind) -> aidoku::Listing {
	aidoku::Listing {
		id: id.into(),
		name: name.into(),
		kind,
	}
}

#[tokio::test]
async fn get_manga_list_receives_the_full_listing() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn");
	let result = handle
		.get_manga_list(listing("hot", "Hot Updates", aidoku::ListingKind::List), 1)
		.await
		.expect("manga list");
	assert_eq!(result.entries[0].title, "hot|Hot Updates|list");
}

/// The concurrency cap is admission semantics: when the sole permit is held externally, calls queue; admission resumes once it is released
#[tokio::test]
async fn source_call_limit_gates_admission() {
	let limit = Arc::new(Semaphore::new(1));
	let (handle, _) =
		SourceHandle::spawn_with_source_limit(config(), Arc::new(Semaphore::new(8)), Some(limit.clone()))
			.await
			.expect("spawn");
	let permit = limit.clone().acquire_owned().await.expect("permit");
	let blocked = tokio::time::timeout(
		Duration::from_millis(500),
		handle.get_manga_list(listing("hot", "Hot Updates", aidoku::ListingKind::List), 1),
	)
	.await;
	assert!(blocked.is_err(), "call must queue behind the exhausted source limit");
	drop(permit);
	let result = handle
		.get_manga_list(listing("dyn", "Dynamic Picks", aidoku::ListingKind::List), 1)
		.await
		.expect("admitted after release");
	assert_eq!(result.entries[0].title, "dyn|Dynamic Picks|list");
}
