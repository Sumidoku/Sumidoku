// each test binary uses only part of the shared helpers
#![allow(dead_code)]

pub mod contract;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use tower::ServiceExt;

pub fn fixture_dir() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/example")
}

/// Fixture payload without a home export, for the unsupported branch
pub fn mutation_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/mutation/main.wasm")
}

/// Fixture payload that emits partial results, for the streaming NDJSON branch
pub fn partial_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/partial/main.wasm")
}

/// Fixture payload for canvas drawing and the image processors
pub fn canvas_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/canvas/main.wasm")
}

/// Fixture payload with a MigrationHandler, for the breaking-change in-place rekey path
pub fn rekey_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/rekey/main.wasm")
}

/// Fixture payload implementing ImageRequestProvider, for the /sources/{id}/image proxy path
pub fn imgreq_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/imgreq/main.wasm")
}

/// Fixture payload declaring page/cover image processors, for the contract path where processors get raw response bytes
pub fn rawimg_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/rawimg/main.wasm")
}

/// Fixture payload exporting handle_web_login, for the web login path
pub fn weblogin_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/weblogin/main.wasm")
}

/// Fixture payload exporting handle_notification and dynamic settings, for the settings merge and notification delivery path
pub fn notify_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/notify/main.wasm")
}

/// Control payload with the same login settings but no handle_web_login export
pub fn weblogin_plain_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/weblogin-plain/main.wasm")
}

/// Fixture payload for the three OAuth login (method: "oauth") branches
pub fn oauth_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/oauth/main.wasm")
}

/// Fixture payload for the page description (PageDescriptionProvider) path
pub fn pagedesc_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/pagedesc/main.wasm")
}

/// Fixture payload for the alternate covers (AlternateCoverProvider) path
pub fn altcovers_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/altcovers/main.wasm")
}

/// Fixture payload exporting get_base_url, for the dynamic base URL resolution path at install/update time
pub fn baseurl_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/baseurl/main.wasm")
}

pub fn deeplink_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/deeplink/main.wasm")
}

/// Fixture payload that echoes the listing it received, for the listing pass-through and source config DTO path
pub fn listing_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/listing/main.wasm")
}

/// Fixture payload with every home component field set, for the home endpoint pass-through contract
pub fn home_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/home/main.wasm")
}

/// Registers an extra fixture source in the sources directory (installed at scan time)
pub fn install_fixture_source(tmp: &tempfile::TempDir, key: &str, name: &str, wasm: &std::path::Path) {
	let doc = serde_json::json!({
		"info": {"id": key, "name": name, "version": 1, "languages": ["en"]}
	});
	install_fixture_source_with_doc(tmp, &doc, wasm);
}

/// Registers a fixture source with a custom source.json (listings/config sections, etc.); info.id comes from the doc's id
pub fn install_fixture_source_with_doc(tmp: &tempfile::TempDir, doc: &Value, wasm: &std::path::Path) {
	let key = doc["info"]["id"].as_str().expect("fixture info.id");
	let dir = tmp.path().join("sources").join(key);
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(wasm, dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(dir.join("source.json"), doc.to_string()).expect("write source.json");
}

pub struct TestApp {
	pub app: Router,
	pub tmp: tempfile::TempDir,
	pub db_path: PathBuf,
}

#[derive(Clone, Default)]
pub struct LogBuffer {
	bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl LogBuffer {
	pub fn take(&self) -> String {
		let mut guard = self.bytes.lock().expect("log buffer");
		String::from_utf8(std::mem::take(&mut *guard)).expect("utf8 logs")
	}

	pub fn snapshot(&self) -> String {
		String::from_utf8(self.bytes.lock().expect("log buffer").clone()).expect("utf8 logs")
	}
}

pub struct LogWriter {
	bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for LogWriter {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.bytes.lock().expect("log buffer").extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl tracing_subscriber::fmt::MakeWriter<'_> for LogBuffer {
	type Writer = LogWriter;

	fn make_writer(&self) -> Self::Writer {
		LogWriter {
			bytes: self.bytes.clone(),
		}
	}
}

pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
	let logs = LogBuffer::default();
	let subscriber = tracing_subscriber::fmt()
		.with_writer(logs.clone())
		.with_ansi(false)
		.with_max_level(tracing::Level::INFO)
		.finish();
	let guard = tracing::subscriber::set_default(subscriber);
	(logs, guard)
}

pub async fn wait_for_log(logs: &LogBuffer, needle: &str) -> String {
	let started = std::time::Instant::now();
	loop {
		let text = logs.snapshot();
		if text.contains(needle) {
			return text;
		}
		assert!(
			started.elapsed() < std::time::Duration::from_secs(5),
			"timed out waiting for log {needle:?}; logs:\n{text}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
	}
}

pub fn base_config(tmp: &tempfile::TempDir) -> Config {
	Config {
		addr: "127.0.0.1:0".into(),
		config_dir: tmp.path().to_path_buf(),
		database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
		covers_dir: tmp.path().join("covers"),
		webui_dir: tmp.path().join("webui"),
		call_concurrency: 8,
		log: "warn".into(),
		..Config::default()
	}
}

pub async fn test_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

pub async fn request(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
	let (status, _, bytes) = request_full(app, method, path, body).await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

pub async fn request_full(
	app: &Router,
	method: &str,
	path: &str,
	body: Option<Value>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
	request_full_accept(app, method, path, body, None).await
}

/// NDJSON streaming request: the response body is parsed line by line into an event array
pub async fn request_ndjson(
	app: &Router,
	method: &str,
	path: &str,
	body: Option<Value>,
) -> (StatusCode, HeaderMap, Vec<Value>) {
	let (status, headers, bytes) =
		request_full_accept(app, method, path, body, Some("application/x-ndjson")).await;
	let lines = bytes
		.split(|b| *b == b'\n')
		.filter(|line| !line.is_empty())
		.map(|line| serde_json::from_slice(line).expect("ndjson line"))
		.collect();
	(status, headers, lines)
}

async fn request_full_accept(
	app: &Router,
	method: &str,
	path: &str,
	body: Option<Value>,
	accept: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
	let mut builder = Request::builder().method(method).uri(path);
	if body.is_some() {
		builder = builder.header("content-type", "application/json");
	}
	if let Some(accept) = accept {
		builder = builder.header("accept", accept);
	}
	let request = builder
		.body(match body {
			Some(value) => Body::from(serde_json::to_vec(&value).expect("serialize body")),
			None => Body::empty(),
		})
		.expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	let status = response.status();
	let headers = response.headers().clone();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	contract::validate_response(method, path, status, &headers, &bytes)
		.unwrap_or_else(|error| panic!("OpenAPI response contract failed for {method} {path} {status}: {error}"));
	(status, headers, bytes.to_vec())
}

/// Binary body request (uploads like .aib): the caller specifies the content-type
pub async fn request_bytes(
	app: &Router,
	method: &str,
	path: &str,
	body: Vec<u8>,
	content_type: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
	let request = Request::builder()
		.method(method)
		.uri(path)
		.header("content-type", content_type)
		.body(Body::from(body))
		.expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	let status = response.status();
	let headers = response.headers().clone();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	contract::validate_response(method, path, status, &headers, &bytes)
		.unwrap_or_else(|error| panic!("OpenAPI response contract failed for {method} {path} {status}: {error}"));
	(status, headers, bytes.to_vec())
}

/// Generic request with caller-supplied headers (auth tests need to attach Authorization)
pub async fn request_with_headers(
	app: &Router,
	method: &str,
	path: &str,
	body: Option<Value>,
	headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
	let mut builder = Request::builder().method(method).uri(path);
	if body.is_some() {
		builder = builder.header("content-type", "application/json");
	}
	for (key, value) in headers {
		builder = builder.header(*key, *value);
	}
	let request = builder
		.body(match body {
			Some(value) => Body::from(serde_json::to_vec(&value).expect("serialize body")),
			None => Body::empty(),
		})
		.expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	let status = response.status();
	let resp_headers = response.headers().clone();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	contract::validate_response(method, path, status, &resp_headers, &bytes)
		.unwrap_or_else(|error| panic!("OpenAPI response contract failed for {method} {path} {status}: {error}"));
	(status, resp_headers, bytes.to_vec())
}

/// Finds an entry in the sources list by key: the built-in local source is persisted by the migration (smallest id on a fresh DB), so indexing by position is no longer reliable
pub fn source_by_key<'a>(body: &'a Value, key: &str) -> &'a Value {
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.unwrap_or_else(|| panic!("source {key} not found in {body}"))
}

/// Installs example-source via scan and registers the manga of the first search result, returning the internal manga id
pub async fn register_example_manga(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	body["id"].as_i64().expect("manga id")
}

pub async fn connect_db(db_path: &std::path::Path) -> sqlx::SqlitePool {
	sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rw", db_path.display()))
		.await
		.expect("connect db")
}

pub async fn sync_change_count(db_path: &std::path::Path) -> i64 {
	let pool = connect_db(db_path).await;
	let count = sqlx::query_scalar("SELECT COUNT(*) FROM sync_change")
		.fetch_one(&pool)
		.await
		.expect("sync change count");
	pool.close().await;
	count
}

/// Full-table logical dump: in WAL mode the file bytes don't represent logical state, so reconcile per table by rowid using quote() text
pub async fn db_dump(db_path: &std::path::Path) -> String {
	const TABLES: &[&str] = &[
		"user", "source", "source_setting", "manga", "manga_user", "chapter", "chapter_user", "page",
		"category", "category_manga", "download", "setting", "source_repo", "source_cookie",
		"auth_token", "sync_change",
	];
	let pool = connect_db(db_path).await;
	let mut out = String::new();
	for table in TABLES {
		let cols: Vec<(String,)> =
			sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{table}')"))
				.fetch_all(&pool)
				.await
				.expect("table info");
		let quoted = cols
			.iter()
			.map(|(c,)| format!("quote(\"{c}\")"))
			.collect::<Vec<_>>()
			.join("||','||");
		let rows: Vec<(String,)> =
			sqlx::query_as(&format!("SELECT {quoted} FROM \"{table}\" ORDER BY rowid"))
				.fetch_all(&pool)
				.await
				.expect("table rows");
		out.push_str(&format!("== {table}\n"));
		for (row,) in rows {
			out.push_str(&row);
			out.push('\n');
		}
	}
	pool.close().await;
	out
}

// 1x1 transparent PNG
pub const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// Starts a minimal static server returning bytes (aborting the handle shuts it down)
pub async fn serve_bytes(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> (u16, tokio::task::JoinHandle<()>) {
	let mut router = Router::new();
	for (path, mime, bytes) in routes {
		let handler = move || {
			let mime = mime.to_string();
			let bytes = bytes.clone();
			async move { ([(axum::http::header::CONTENT_TYPE, mime)], bytes) }
		};
		router = router.route(path, axum::routing::get(handler));
	}
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, handle)
}

/// Points page 0 at the local image host after pages are cached, so the worker runs fully offline via the cache
pub async fn point_page_at_local(pool: &sqlx::SqlitePool, chapter_id: i64, port: u16) {
	let stored = json!({
		"content": {"Url": [format!("http://127.0.0.1:{port}/img.png"), null]},
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	});
	sqlx::query("UPDATE page SET content = ? WHERE chapter_id = ? AND idx = 0")
		.bind(stored.to_string())
		.bind(chapter_id)
		.execute(pool)
		.await
		.expect("rewrite page 0");
}

/// Serves the app on a random port for real (the WS handshake needs real TCP)
pub async fn serve_app(app: Router) -> std::net::SocketAddr {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local addr");
	tokio::spawn(async move {
		axum::serve(listener, app.into_make_service()).await.expect("serve");
	});
	addr
}

/// Minimal WS client: completes the handshake and returns the stream plus the leftover buffer after the headers (token goes in the Authorization header)
pub async fn connect_ws(
	addr: std::net::SocketAddr,
	path: &str,
	token: Option<&str>,
) -> (tokio::net::TcpStream, Vec<u8>) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
	let auth = token.map(|t| format!("Authorization: {t}\r\n")).unwrap_or_default();
	let handshake = format!(
		"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{auth}\r\n"
	);
	stream.write_all(handshake.as_bytes()).await.expect("write handshake");
	let mut received = Vec::new();
	let mut tmp = [0u8; 4096];
	loop {
		let n = stream.read(&mut tmp).await.expect("read handshake");
		assert!(n > 0, "connection closed during handshake");
		received.extend_from_slice(&tmp[..n]);
		if received.windows(4).any(|w| w == b"\r\n\r\n") {
			break;
		}
	}
	let head_end = received
		.windows(4)
		.position(|w| w == b"\r\n\r\n")
		.expect("header end");
	let head = String::from_utf8_lossy(&received[..head_end]);
	assert!(head.contains("101"), "{head}");
	let rest = received[head_end + 4..].to_vec();
	(stream, rest)
}

/// Reads one text frame (server frames are not masked)
pub async fn read_ws_frame(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>) -> Vec<u8> {
	use tokio::io::AsyncReadExt;
	async fn fill(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, want: usize) {
		let mut tmp = [0u8; 4096];
		while buf.len() < want {
			let n = stream.read(&mut tmp).await.expect("read frame");
			assert!(n > 0, "connection closed mid-frame");
			buf.extend_from_slice(&tmp[..n]);
		}
	}
	fill(stream, buf, 2).await;
	let (offset, len) = match buf[1] & 0x7f {
		126 => {
			fill(stream, buf, 4).await;
			(4, u16::from_be_bytes([buf[2], buf[3]]) as usize)
		}
		127 => {
			fill(stream, buf, 10).await;
			(10, u64::from_be_bytes(buf[2..10].try_into().expect("len")) as usize)
		}
		n => (2, n as usize),
	};
	fill(stream, buf, offset + len).await;
	let payload = buf[offset..offset + len].to_vec();
	buf.drain(..offset + len);
	payload
}

/// Reads until an event frame of the given type arrives (intermediate frames discarded), 10s backstop
pub async fn read_ws_kind(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, kind: &str) -> Value {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let remaining = deadline.saturating_duration_since(std::time::Instant::now());
		assert!(remaining > std::time::Duration::ZERO, "no {kind} event in time");
		let frame = tokio::time::timeout(remaining, read_ws_frame(stream, buf))
			.await
			.expect("ws read timeout");
		let value: Value = serde_json::from_slice(&frame).expect("event json");
		if value["type"] == kind {
			return value;
		}
	}
}
