//! html host 模块 9 个 DOM mutation 函数的端到端测试。
//! fixture 源在 tests/fixtures/mutation-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/mutation-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_mutation.wasm
//! 复制为 tests/fixtures/mutation/main.wasm。
use aidoku_wasm::{DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

const EXPECTED: &str = "set_attr=ok,attr=v1;remove_attr=ok,attr=none;\
set_text=ok,text=clean;set_html=ok,b=1,text=bold;\
prepend=ok;append=ok,first=i,last=u;\
add_class=ok,class=x y z,has_z=1;remove_class=ok,class=x z,sel_z=1;\
remove=ok,p=1;remove_list=ok,p=0";

#[tokio::test]
async fn dom_mutations_visible_to_reads() {
	let config = SourceConfig {
		key: "en.mutation".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mutation/main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_ws: None,
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
