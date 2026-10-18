//! js host 模块 context_* 四函数的端到端测试。
//! fixture 源在 tests/fixtures/js-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/js-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_js.wasm
//! 复制为 tests/fixtures/js/main.wasm。
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
		browser_ws: None,
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
