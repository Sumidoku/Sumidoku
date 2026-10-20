//! MigrationHandler 导出的端到端测试（breaking-change key 迁移的 wasm 侧）。
//! fixture 源在 tests/fixtures/rekey-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/rekey-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_rekey.wasm
//! 复制为 tests/fixtures/rekey/main.wasm。
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

	// old/broken 一支返回空串：服务端据此把该行留旧 key 并进 warnings
	let broken = handle
		.handle_key_migration(0, "old/broken".into(), None)
		.await
		.expect("broken migration");
	assert_eq!(broken, "");

	// 非 old/ 前缀原样返回：服务端视为 no-op 跳过
	let untouched = handle
		.handle_key_migration(0, "new/manga-1".into(), None)
		.await
		.expect("no-op migration");
	assert_eq!(untouched, "new/manga-1");
}
