//! Host-side end-to-end tests for the WebLoginHandler export.
//! The fixture source lives in tests/fixtures/weblogin-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/weblogin-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_weblogin.wasm
//! over tests/fixtures/weblogin/main.wasm.
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
		browser_trawl: None,
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
	// Presets are sorted by cookie name; the server side's SQL insert uses the same order, so the two writers never conflict over the same value
	let Some(DefaultValue::StringArray(keys)) = written(&writes, "login.keys") else {
		panic!("missing login.keys write: {writes:?}")
	};
	assert_eq!(keys, strings(&["other", "session"]));
	let Some(DefaultValue::StringArray(values)) = written(&writes, "login.values") else {
		panic!("missing login.values write: {writes:?}")
	};
	assert_eq!(values, strings(&["o1", "s1"]));
	// The fixture's observation: the key/cookies parameters the export received, and the
	// preset keys read back from defaults during the call (Aidoku order: store first, then
	// call — the source reads cookies via defaults_get_map)
	let Some(DefaultValue::StringArray(seen)) = written(&writes, "weblogin.seen") else {
		panic!("missing weblogin.seen write: {writes:?}")
	};
	assert_eq!(seen, strings(&["login", "other=o1,session=s1", "other,session"]));

	// The store-first-then-call order itself must also be pinned: presets must be written before the fixture's observation write
	let pos = |key: &str| writes.iter().position(|(k, _)| k == key).expect(key);
	assert!(pos("login.keys") < pos("weblogin.seen"));
	assert!(pos("login.values") < pos("weblogin.seen"));
	// The marker is not written at the host layer: the server persists it synchronously before restarting — an async sink would race the restart's read of the store
	assert!(written(&writes, "login").is_none(), "marker 应由服务端写: {writes:?}");
}

#[tokio::test]
async fn web_login_handler_can_veto_and_accepts_empty() {
	let sink = Arc::new(RecordingSink::default());
	let (handle, _) = spawn("weblogin", sink).await;

	// The fixture only accepts session: without it, it returns false and the server 502s on that basis
	let cookies = HashMap::from([("other".to_string(), "o1".to_string())]);
	let ok = handle
		.handle_web_login("login".into(), cookies)
		.await
		.expect("web login");
	assert!(!ok);

	// Empty cookies are the log-out semantic; the source just acknowledges them (Aidoku also calls the export with empty cookies)
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
