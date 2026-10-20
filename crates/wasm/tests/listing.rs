//! listing 回显 fixture 与 per-source 并发帽（config.maximumParallelRequests，issue #138）。
//! fixture 源在 crates/wasm/tests/fixtures/listing-src（no_std cdylib），重建方式：
//! `cd crates/wasm/tests/fixtures/listing-src && cargo build --release --target wasm32-unknown-unknown`，
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_listing.wasm
//! 复制为 crates/wasm/tests/fixtures/listing/main.wasm。
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

/// 并发帽是准入语义：唯一许可被外部占住时调用排队，释放后准入恢复
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
