//! End-to-end tests for the four context_* functions of the js host module.
//! The fixture source lives in tests/fixtures/js-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/js-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_js.wasm
//! over tests/fixtures/js/main.wasm.
use aidoku_wasm::{
	DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

const EXPECTED: &str = "3,fn1,tk42,tk42,undefined,err:missing,42,await,err:missing,err:missing,raw:context,raw:destroyed";

#[tokio::test]
async fn js_context_eval_get_and_isolation() {
	let config = SourceConfig {
		key: "en.js".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/js/main.wasm"),
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
	let (handle, features) = SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	assert!(features.page_description);

	let out = handle
		.get_page_description(aidoku::Page::default())
		.await
		.expect("page description");
	assert_eq!(out, EXPECTED);
}
