// 各测试二进制只用到部分公共件
#![allow(dead_code)]

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

/// 无 home 导出能力的 fixture 载荷，用于 unsupported 分支
pub fn mutation_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/mutation/main.wasm")
}

/// 发 partial 结果的 fixture 载荷，用于流式 NDJSON 分支
pub fn partial_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/partial/main.wasm")
}

/// canvas 绘制与图像处理器的 fixture 载荷
pub fn canvas_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/canvas/main.wasm")
}

/// 带 MigrationHandler 的 fixture 载荷，用于 breaking-change 原地重键链路
pub fn rekey_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/rekey/main.wasm")
}

/// 实现 ImageRequestProvider 的 fixture 载荷，用于 /sources/{id}/image 代理链路
pub fn imgreq_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/imgreq/main.wasm")
}

/// 声明 page/cover 图像处理器的 fixture 载荷，用于处理器拿原始响应字节的契约链路
pub fn rawimg_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/rawimg/main.wasm")
}

/// 导出 handle_web_login 的 fixture 载荷，用于网页登录链路
pub fn weblogin_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/weblogin/main.wasm")
}

/// 导出 handle_notification 与动态设置的 fixture 载荷，用于设置合并与通知送达链路
pub fn notify_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/notify/main.wasm")
}

/// 同款登录设置但不导出 handle_web_login 的对照载荷
pub fn weblogin_plain_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/weblogin-plain/main.wasm")
}

/// OAuth 登录（method: "oauth"）三分支的 fixture 载荷
pub fn oauth_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/oauth/main.wasm")
}

/// 页描述（PageDescriptionProvider）链路的 fixture 载荷
pub fn pagedesc_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/pagedesc/main.wasm")
}

/// 备选封面（AlternateCoverProvider）链路的 fixture 载荷
pub fn altcovers_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/altcovers/main.wasm")
}

/// 导出 get_base_url 的 fixture 载荷，用于安装/更新时的动态 base URL 解析链路
pub fn baseurl_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/baseurl/main.wasm")
}

pub fn deeplink_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/deeplink/main.wasm")
}

/// 回显收到 listing 的 fixture 载荷，用于 listing 透传与 source config DTO 链路
pub fn listing_wasm_path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/listing/main.wasm")
}

/// 在 sources 目录里登记一个额外 fixture 源（scan 时安装）
pub fn install_fixture_source(tmp: &tempfile::TempDir, key: &str, name: &str, wasm: &std::path::Path) {
	let doc = serde_json::json!({
		"info": {"id": key, "name": name, "version": 1, "languages": ["en"]}
	});
	install_fixture_source_with_doc(tmp, &doc, wasm);
}

/// 带自定义 source.json（listings/config 等节）的 fixture 源登记；info.id 取 doc 里的 id
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

/// NDJSON 流式请求：响应体按行解析成事件数组
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
	(status, headers, bytes.to_vec())
}

/// 二进制 body 请求（.aib 这类上传）：content-type 由调用方指定
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
	(status, headers, bytes.to_vec())
}

/// 自带请求头的通用请求（认证测试要挂 Authorization）
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
	(status, resp_headers, bytes.to_vec())
}

/// scan 安装 example-source 并注册第一本搜索结果的漫画，返回内部 manga id
pub async fn register_example_manga(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = body[0]["id"].as_i64().expect("source id");
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

/// 全表逻辑 dump：WAL 模式下文件字节不代表逻辑状态，逐表按 rowid 取 quote 文本对账
pub async fn db_dump(db_path: &std::path::Path) -> String {
	const TABLES: &[&str] = &[
		"user", "source", "source_setting", "manga", "manga_user", "chapter", "chapter_user", "page",
		"category", "category_manga", "download", "setting", "source_repo", "source_cookie",
		"auth_credential", "auth_token",
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

// ---- sidecar 交互登录的 mock CDP：aidoku_wasm 自带 mock 是 cfg(test) 不外露，
// 这里按端点测试关注点实现同名 trait ----
pub mod browser_mock {
	use aidoku_wasm::browser::{
		BrowserCookie, BrowserError, CdpClient, CdpConnector, FrameSink, KeyInput, MouseInput,
	};
	use std::collections::HashMap;
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	pub struct MockBrowserState {
		pub connects: usize,
		pub contexts: Vec<String>,
		pub disposed: Vec<String>,
		pub loads: Vec<(String, String)>,
		pub viewports: Vec<(String, i64, i64)>,
		pub screencasts: Vec<String>,
		pub screencast_stopped: Vec<String>,
		pub cookies: HashMap<String, Vec<BrowserCookie>>,
		pub local_storage: HashMap<String, String>,
		pub mouse_inputs: usize,
		pub key_inputs: usize,
		pub inserted_text: Vec<String>,
		pub frames: Vec<String>,
	}

	pub fn cookie(name: &str, value: &str, domain: &str) -> BrowserCookie {
		BrowserCookie {
			name: name.into(),
			value: value.into(),
			domain: domain.into(),
			path: "/".into(),
			expires: None,
			secure: false,
			http_only: false,
		}
	}

	pub struct MockBrowserConnector(pub Arc<Mutex<MockBrowserState>>);

	impl CdpConnector for MockBrowserConnector {
		fn connect(&self) -> Result<Box<dyn CdpClient>, BrowserError> {
			self.0.lock().expect("mock lock").connects += 1;
			Ok(Box::new(MockBrowserClient(self.0.clone())))
		}
	}

	struct MockBrowserClient(Arc<Mutex<MockBrowserState>>);

	impl CdpClient for MockBrowserClient {
		fn create_context(&mut self, _label: &str) -> Result<String, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			let id = format!("ctx-{}", state.contexts.len() + 1);
			state.contexts.push(id.clone());
			Ok(id)
		}

		fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.disposed.push(context.to_string());
			state.contexts.retain(|c| c != context);
			Ok(())
		}

		fn load(
			&mut self,
			context: &str,
			url: &str,
			_headers: &[(String, String)],
			_timeout: std::time::Duration,
		) -> Result<Vec<BrowserCookie>, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.loads.push((context.to_string(), url.to_string()));
			Ok(state.cookies.get(context).cloned().unwrap_or_default())
		}

		fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError> {
			let state = self.0.lock().expect("mock lock");
			Ok(state.cookies.get(context).cloned().unwrap_or_default())
		}

		fn delete_cookie(
			&mut self,
			_context: &str,
			_name: &str,
			_domain: Option<&str>,
			_path: Option<&str>,
		) -> Result<(), BrowserError> {
			Ok(())
		}

		fn clear_cookies(&mut self, _context: &str) -> Result<(), BrowserError> {
			Ok(())
		}

		fn set_viewport(&mut self, context: &str, width: i64, height: i64) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.viewports.push((context.to_string(), width, height));
			Ok(())
		}

		fn screencast_start(&mut self, context: &str, sink: FrameSink) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.screencasts.push(context.to_string());
			for frame in state.frames.drain(..) {
				let _ = sink.send(frame);
			}
			Ok(())
		}

		fn screencast_stop(&mut self, context: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.screencast_stopped.push(context.to_string());
			Ok(())
		}

		fn dispatch_mouse(&mut self, _context: &str, _input: &MouseInput) -> Result<(), BrowserError> {
			self.0.lock().expect("mock lock").mouse_inputs += 1;
			Ok(())
		}

		fn dispatch_key(&mut self, _context: &str, _input: &KeyInput) -> Result<(), BrowserError> {
			self.0.lock().expect("mock lock").key_inputs += 1;
			Ok(())
		}

		fn insert_text(&mut self, _context: &str, text: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.inserted_text.push(text.to_string());
			Ok(())
		}

		fn local_storage_values(
			&mut self,
			_context: &str,
			keys: &[String],
		) -> Result<Vec<(String, String)>, BrowserError> {
			let state = self.0.lock().expect("mock lock");
			Ok(keys
				.iter()
				.filter_map(|k| state.local_storage.get(k).map(|v| (k.clone(), v.clone())))
				.collect())
		}
	}
}

/// 装配带 mock CDP 连接器的 app：sidecar 交互登录的端点测试驱动
pub async fn test_app_with_browser(
	browser: std::sync::Arc<std::sync::Mutex<browser_mock::MockBrowserState>>,
) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = sumidoku_server::build_app_with(config, Some(Box::new(browser_mock::MockBrowserConnector(browser))))
		.await
		.expect("build app");
	TestApp { app, tmp, db_path }
}
