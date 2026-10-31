// each test binary uses only part of the shared helpers
#![allow(dead_code)]

pub mod contract;

use axum::{Extension, Router};
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
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
	pub tmp: Arc<tempfile::TempDir>,
	pub db_path: PathBuf,
}

impl TestApp {
	/// The router holds the directory too, so `let TestApp { app, .. }` can't delete it from under the app
	pub fn new(app: Router, tmp: impl Into<Arc<tempfile::TempDir>>, db_path: PathBuf) -> Self {
		let tmp = tmp.into();
		let app = app.layer(Extension(tmp.clone()));
		Self { app, tmp, db_path }
	}
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
		downloads_dir: tmp.path().join("downloads"),
		webui_dir: tmp.path().join("webui"),
		call_concurrency: 8,
		log: "warn".into(),
		..Config::default()
	}
}

pub fn advance_mtime(path: &std::path::Path, previous: std::time::SystemTime) -> std::time::SystemTime {
	let next = previous + std::time::Duration::from_secs(2);
	let file = std::fs::OpenOptions::new().write(true).open(path).expect("open file mtime");
	file.set_modified(next).expect("advance file mtime");
	next
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
	TestApp::new(app, tmp, db_path)
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

/// Phase-2 browse reads the scan index: tests trigger the scan explicitly and wait for the queued tasks
/// (scan + derived analysis) to settle
pub async fn local_scan_and_settle(app: &Router, source_id: Option<i64>) {
	let body = source_id.map(|id| json!({"sourceId": id}));
	let (status, body) = request(app, "POST", "/api/v1/local/scan", body).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let started = std::time::Instant::now();
	loop {
		let (status, body) = request(app, "GET", "/api/v1/tasks", None).await;
		assert_eq!(status, StatusCode::OK);
		let tasks = body.as_array().expect("tasks array");
		let live = tasks
			.iter()
			.filter(|task| task["status"] == "pending" || task["status"] == "running")
			.count();
		if live == 0 {
			return;
		}
		assert!(
			started.elapsed() < std::time::Duration::from_secs(15),
			"tasks never settled: {body}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
	}
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

pub async fn serve_app(app: Router) -> std::net::SocketAddr {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("local addr");
	tokio::spawn(async move {
		axum::serve(listener, app.into_make_service()).await.expect("serve");
	});
	addr
}

pub async fn connect_sse_response(
	addr: std::net::SocketAddr,
	path: &str,
	last_event_id: Option<i64>,
	headers: &[(&str, &str)],
) -> (tokio::net::TcpStream, Vec<u8>, String) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
	let last = last_event_id.map(|id| format!("Last-Event-ID: {id}\r\n")).unwrap_or_default();
	let extra = headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect::<String>();
	let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{last}{extra}\r\n");
	stream.write_all(request.as_bytes()).await.expect("write sse request");
	let mut received = Vec::new();
	let mut tmp = [0u8; 4096];
	loop {
		let n = stream.read(&mut tmp).await.expect("read sse headers");
		assert!(n > 0, "connection closed during sse handshake");
		received.extend_from_slice(&tmp[..n]);
		if received.windows(4).any(|w| w == b"\r\n\r\n") {
			break;
		}
	}
	let head_end = received.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
	let head = String::from_utf8_lossy(&received[..head_end]).into_owned();
	let rest = received[head_end + 4..].to_vec();
	(stream, rest, head)
}

pub async fn connect_sse(
	addr: std::net::SocketAddr,
	path: &str,
	last_event_id: Option<i64>,
	headers: &[(&str, &str)],
) -> (tokio::net::TcpStream, Vec<u8>) {
	let (stream, rest, head) = connect_sse_response(addr, path, last_event_id, headers).await;
	assert!(head.contains("200 OK"), "{head}");
	(stream, rest)
}

pub async fn read_sse_event(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>) -> Value {
	use tokio::io::AsyncReadExt;
	loop {
		if let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
			let frame = buf.drain(..pos + 2).collect::<Vec<_>>();
			let text = String::from_utf8_lossy(&frame);
			if text.starts_with(':') {
				continue;
			}
			let mut kind = None;
			let mut id = None;
			let mut data = None;
			for line in text.lines() {
				if let Some(value) = line.strip_prefix("event:") {
					kind = Some(value.to_string());
				} else if let Some(value) = line.strip_prefix("id:") {
					id = value.parse::<i64>().ok();
				} else if let Some(value) = line.strip_prefix("data:") {
					data = Some(value.to_string());
				}
			}
			if kind.is_none() {
				continue;
			}
			let payload = serde_json::from_str(&data.unwrap_or_default()).unwrap_or(Value::Null);
			return serde_json::json!({ "type": kind, "id": id, "data": payload });
		}
		let mut tmp = [0u8; 4096];
		let n = stream.read(&mut tmp).await.expect("read sse event");
		assert!(n > 0, "connection closed mid-sse-event");
		buf.extend_from_slice(&tmp[..n]);
	}
}

pub async fn read_sse_kind(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, kind: &str) -> Value {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let remaining = deadline.saturating_duration_since(std::time::Instant::now());
		assert!(remaining > std::time::Duration::ZERO, "no {kind} event in time");
		let frame = tokio::time::timeout(remaining, read_sse_event(stream, buf))
			.await
			.expect("sse read timeout");
		if frame["type"] == kind {
			return frame;
		}
	}
}

pub async fn read_sse_data(stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, kind: &str) -> Value {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let remaining = deadline.saturating_duration_since(std::time::Instant::now());
		assert!(remaining > std::time::Duration::ZERO, "no {kind} event data in time");
		let frame = tokio::time::timeout(remaining, read_sse_event(stream, buf))
			.await
			.expect("sse read timeout");
		if frame["data"]["type"] == kind {
			return frame["data"].clone();
		}
	}
}

/// Journals what raw SQL wrote since the last call, parents first, for tests that need exact ids or
/// times. Production writes go through the stores, which journal themselves.
#[allow(dead_code)]
pub async fn publish_raw_writes(pool: &sqlx::SqlitePool) {
	use sumidoku_server::store::{category, chapter, library, manga, progress};
	let mut conn = pool.acquire().await.expect("connection");
	// A migration test can stop at a schema that still has the journal triggers
	let current: bool = sqlx::query_scalar(
		"SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'server_state')
		 AND NOT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'sync_change_origin_insert')",
	)
		.fetch_one(&mut *conn)
		.await
		.expect("schema");
	if !current {
		return;
	}
	// Sync stubs have no name yet and stay quiet at version 0
	let unpublished = |kind: &str, key_sql: &str, table: &str, name: &str| {
		format!(
			"SELECT {key_sql} FROM {table} t WHERE NOT EXISTS (
				SELECT 1 FROM sync_change sc WHERE sc.entity_kind = '{kind}' AND sc.entity_key = CAST(t.id AS TEXT)
				AND sc.modified_at >= t.last_modified_at
			) AND t.{name} <> '' ORDER BY t.id"
		)
	};
	let ids: Vec<i64> = sqlx::query_scalar(&unpublished("manga", "t.id", "manga", "title")).fetch_all(&mut *conn).await.expect("manga");
	for id in ids {
		manga::publish(&mut conn, id).await.expect("publish manga");
	}
	let ids: Vec<i64> = sqlx::query_scalar(
		"SELECT t.id FROM chapter t WHERE NOT EXISTS (SELECT 1 FROM sync_chapter_state s WHERE s.chapter_id = t.id) ORDER BY t.id",
	)
	.fetch_all(&mut *conn)
	.await
	.expect("chapters");
	for id in ids {
		chapter::publish(&mut conn, id).await.expect("publish chapter");
	}
	let ids: Vec<i64> = sqlx::query_scalar(&unpublished("category", "t.id", "category", "name")).fetch_all(&mut *conn).await.expect("categories");
	for id in ids {
		category::publish(&mut conn, id).await.expect("publish category");
	}
	let rows: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT t.manga_id, t.user_id FROM manga_user t WHERE NOT EXISTS (
			SELECT 1 FROM sync_change sc WHERE sc.entity_kind = 'manga_user'
			AND sc.entity_key = t.manga_id || ':' || t.user_id AND sc.modified_at >= t.last_modified_at
		) ORDER BY t.manga_id, t.user_id",
	)
	.fetch_all(&mut *conn)
	.await
	.expect("library rows");
	for (manga_id, user_id) in rows {
		library::publish(&mut conn, manga_id, user_id).await.expect("publish library row");
	}
	let rows: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT t.chapter_id, t.user_id FROM chapter_user t WHERE NOT EXISTS (
			SELECT 1 FROM sync_change sc WHERE sc.entity_kind = 'chapter_user'
			AND sc.entity_key = t.chapter_id || ':' || t.user_id AND sc.modified_at >= t.last_modified_at
		) AND NOT (t.is_downloaded = 1 AND t.last_page_read = 0 AND t.is_read = 0 AND t.is_bookmarked = 0 AND t.last_read_at IS NULL)
		ORDER BY t.chapter_id, t.user_id",
	)
	.fetch_all(&mut *conn)
	.await
	.expect("progress rows");
	for (chapter_id, user_id) in rows {
		progress::publish(&mut conn, chapter_id, user_id).await.expect("publish progress");
	}
	let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
		"SELECT t.category_id, t.manga_id, t.user_id FROM category_manga t WHERE NOT EXISTS (
			SELECT 1 FROM sync_change sc WHERE sc.entity_kind = 'category_manga'
			AND sc.entity_key = t.category_id || ':' || t.manga_id || ':' || t.user_id AND sc.modified_at >= t.last_modified_at
		) ORDER BY t.category_id, t.manga_id, t.user_id",
	)
	.fetch_all(&mut *conn)
	.await
	.expect("memberships");
	for (category_id, manga_id, user_id) in rows {
		category::publish_member(&mut conn, category_id, manga_id, user_id).await.expect("publish membership");
	}
}

/// Journals a category written with raw SQL, for tests that need exact times or ids
#[allow(dead_code)]
pub async fn publish_category(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::category::publish(&mut conn, id).await.expect("publish category");
}

#[allow(dead_code)]
pub async fn delete_category(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::category::delete(&mut conn, id).await.expect("delete category");
}

#[allow(dead_code)]
pub async fn delete_manga(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::manga::delete(&mut conn, id).await.expect("delete manga");
}

#[allow(dead_code)]
pub async fn delete_chapter(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::chapter::delete(&mut conn, id).await.expect("delete chapter");
}

#[allow(dead_code)]
pub async fn delete_source(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::source::delete(&mut conn, id).await.expect("delete source");
}

#[allow(dead_code)]
pub async fn delete_library_row(pool: &sqlx::SqlitePool, manga_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::library::delete(&mut conn, manga_id, 1).await.expect("delete library row");
}

#[allow(dead_code)]
pub async fn delete_progress(pool: &sqlx::SqlitePool, chapter_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::progress::delete(&mut conn, chapter_id, 1).await.expect("delete progress");
}

#[allow(dead_code)]
pub async fn delete_membership(pool: &sqlx::SqlitePool, category_id: i64, manga_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::category::remove_manga(&mut conn, category_id, manga_id, 1).await.expect("delete membership");
}

#[allow(dead_code)]
pub async fn publish_manga(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::manga::publish(&mut conn, id).await.expect("publish manga");
}

#[allow(dead_code)]
pub async fn publish_chapter(pool: &sqlx::SqlitePool, id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::chapter::publish(&mut conn, id).await.expect("publish chapter");
}

#[allow(dead_code)]
pub async fn publish_library(pool: &sqlx::SqlitePool, manga_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::library::publish(&mut conn, manga_id, 1).await.expect("publish library row");
}

#[allow(dead_code)]
pub async fn publish_progress(pool: &sqlx::SqlitePool, chapter_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::progress::publish(&mut conn, chapter_id, 1).await.expect("publish progress");
}

#[allow(dead_code)]
pub async fn publish_membership(pool: &sqlx::SqlitePool, category_id: i64, manga_id: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::category::publish_member(&mut conn, category_id, manga_id, 1).await.expect("publish membership");
}

#[allow(dead_code)]
pub async fn set_chapter_version(pool: &sqlx::SqlitePool, id: i64, modified_at: i64) {
	let mut conn = pool.acquire().await.expect("connection");
	sumidoku_server::store::chapter::set_modified_at(&mut conn, id, modified_at).await.expect("set chapter version");
}
