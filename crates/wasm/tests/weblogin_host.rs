//! WebLoginHandler 导出的宿主侧端到端测试。
//! fixture 源在 tests/fixtures/weblogin-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/weblogin-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_weblogin.wasm
//! 复制为 tests/fixtures/weblogin/main.wasm。
use aidoku_wasm::{
	DefaultValue, DefaultsSink, DirectPolicy, Error, NoopCookieSink, SourceConfig, SourceHandle,
	StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Default)]
struct RecordingSink {
	writes: Mutex<Vec<(String, DefaultValue)>>,
}

impl DefaultsSink for RecordingSink {
	fn set(&self, _source_key: &str, key: &str, value: &DefaultValue) {
		self.writes
			.lock()
			.expect("sink lock")
			.push((key.to_string(), value.clone()));
	}
}

fn strings(values: &[&str]) -> Vec<String> {
	values.iter().map(|v| v.to_string()).collect()
}

async fn spawn(wasm: &str, sink: Arc<RecordingSink>) -> (SourceHandle, aidoku_wasm::SourceFeatures) {
	let config = SourceConfig {
		key: "en.weblogin".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{wasm}/main.wasm")),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink,
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_ws: None,
	};
	SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source")
}

fn written(writes: &[(String, DefaultValue)], key: &str) -> Option<DefaultValue> {
	writes.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[tokio::test]
async fn web_login_presets_cookie_defaults_before_the_call() {
	let sink = Arc::new(RecordingSink::default());
	let (handle, features) = spawn("weblogin", sink.clone()).await;
	assert!(features.web_login);

	let cookies = HashMap::from([
		("session".to_string(), "s1".to_string()),
		("other".to_string(), "o1".to_string()),
	]);
	let ok = handle
		.handle_web_login("login".into(), cookies)
		.await
		.expect("web login");
	assert!(ok, "cookies 与预置 keys 都含 session，fixture 应判成功");

	let writes = sink.writes.lock().expect("sink lock");
	// 预置按 cookie 名排序，server 端 SQL 落库同序，两处写同一值不打架
	let Some(DefaultValue::StringArray(keys)) = written(&writes, "login.keys") else {
		panic!("missing login.keys write: {writes:?}")
	};
	assert_eq!(keys, strings(&["other", "session"]));
	let Some(DefaultValue::StringArray(values)) = written(&writes, "login.values") else {
		panic!("missing login.values write: {writes:?}")
	};
	assert_eq!(values, strings(&["o1", "s1"]));
	// fixture 的观测：导出拿到的 key/cookies 参数，以及调用期间从 defaults
	// 读回的预置 keys（Aidoku 顺序：先存后调，源靠 defaults_get_map 取 cookie）
	let Some(DefaultValue::StringArray(seen)) = written(&writes, "weblogin.seen") else {
		panic!("missing weblogin.seen write: {writes:?}")
	};
	assert_eq!(seen, strings(&["login", "other=o1,session=s1", "other,session"]));

	// 先存后调的顺序本身也要钉住：预置必须排在 fixture 的观测写之前
	let pos = |key: &str| writes.iter().position(|(k, _)| k == key).expect(key);
	assert!(pos("login.keys") < pos("weblogin.seen"));
	assert!(pos("login.values") < pos("weblogin.seen"));
	// marker 不在宿主层写：服务端重启前同步落库，异步 sink 会跟重启读库竞态
	assert!(written(&writes, "login").is_none(), "marker 应由服务端写: {writes:?}");
}

#[tokio::test]
async fn web_login_handler_can_veto_and_accepts_empty() {
	let sink = Arc::new(RecordingSink::default());
	let (handle, _) = spawn("weblogin", sink).await;

	// fixture 只认 session：缺它返回 false，服务端据此 502
	let cookies = HashMap::from([("other".to_string(), "o1".to_string())]);
	let ok = handle
		.handle_web_login("login".into(), cookies)
		.await
		.expect("web login");
	assert!(!ok);

	// 空 cookies 是登出语义，源确认即可（Aidoku 空 cookies 也照调导出）
	let ok = handle
		.handle_web_login("login".into(), HashMap::new())
		.await
		.expect("web login empty");
	assert!(ok);
}

#[tokio::test]
async fn web_login_unsupported_without_export() {
	let sink = Arc::new(RecordingSink::default());
	let (handle, features) = spawn("weblogin-plain", sink.clone()).await;
	assert!(!features.web_login);

	let err = handle
		.handle_web_login("login".into(), HashMap::new())
		.await
		.expect_err("web login unsupported");
	assert!(matches!(err, Error::Unsupported("handle_web_login")));
	assert!(
		sink.writes.lock().expect("sink lock").is_empty(),
		"不支持时不得预置 defaults"
	);
}
