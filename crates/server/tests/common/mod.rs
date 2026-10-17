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

pub struct TestApp {
	pub app: Router,
	pub tmp: tempfile::TempDir,
	pub db_path: PathBuf,
}

pub fn base_config(tmp: &tempfile::TempDir) -> Config {
	Config {
		addr: "127.0.0.1:0".into(),
		database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
		sources_dir: tmp.path().join("sources"),
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
	let mut builder = Request::builder().method(method).uri(path);
	if body.is_some() {
		builder = builder.header("content-type", "application/json");
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
