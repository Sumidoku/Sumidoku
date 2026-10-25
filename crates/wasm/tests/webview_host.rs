//! End-to-end tests for the webview_* sandbox semantics of the js host module.
//! The fixture source lives in tests/fixtures/webview-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/webview-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_webview.wasm
//! over tests/fixtures/webview/main.wasm.
use aidoku_wasm::{
	DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

const EXPECTED: &str = "3,ok,tk42,tk42!,https://example.com/chapter/1,true,ok,ok,undefined,about:blank,ok,undefined,undefined,yes,ok,sp,se,42,err:missing,err:missing,err:missing,err:missing,err:missing,err:missing,raw:handler,raw:cross,raw:crossctx,raw:destroyed";

#[tokio::test]
async fn webview_sandbox_semantics() {
	let config = SourceConfig {
		key: "en.webview".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview/main.wasm"),
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
