use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use suzuri_server::build_app;
use suzuri_server::config::Config;
use tower::ServiceExt;

fn fixture_dir() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/example")
}

async fn test_app() -> (Router, tempfile::TempDir) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let config = Config {
		addr: "127.0.0.1:0".into(),
		database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
		sources_dir: tmp.path().join("sources"),
		call_concurrency: 8,
		log: "warn".into(),
	};
	let app = build_app(config).await.expect("build app");
	(app, tmp)
}

async fn request(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
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
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

#[tokio::test]
async fn full_flow() {
	let (app, _tmp) = test_app().await;

	// health
	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");

	// 未安装时 404
	let (status, body) = request(&app, "GET", "/api/v1/manga/999", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	// scan 安装
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!(["en.example-source"]));
	assert_eq!(body["updated"], json!([]));
	assert_eq!(body["skipped"], json!([]));

	// 再 scan 幂等：老源走 updated
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!([]));
	assert_eq!(body["updated"], json!(["en.example-source"]));

	// sources 列表
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	assert_eq!(sources.len(), 1);
	assert_eq!(sources[0]["key"], "en.example-source");
	assert_eq!(sources[0]["name"], "Example Source");
	assert_eq!(sources[0]["enabled"], true);
	let source_id = sources[0]["id"].as_i64().expect("source id");

	// 搜索 20 条
	let search_path = format!("/api/v1/sources/{source_id}/search");
	let (status, body) = request(&app, "POST", &search_path, Some(json!({"page": 1}))).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 20);
	assert_eq!(body["hasNextPage"], true);
	assert_eq!(entries[0]["sourceId"], source_id);
	let manga_key = entries[0]["key"].as_str().expect("manga key").to_string();

	// query 过滤
	let (status, body) = request(
		&app,
		"POST",
		&search_path,
		Some(json!({"query": "Manga 3", "page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let filtered = body["entries"].as_array().expect("filtered entries");
	assert!(!filtered.is_empty());
	assert!(
		filtered
			.iter()
			.all(|m| m["title"].as_str().unwrap_or_default().contains("Manga 3")),
		"all titles should contain 'Manga 3': {filtered:?}"
	);

	// 入库
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");

	// 详情
	let manga_path = format!("/api/v1/manga/{manga_id}");
	let (status, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["initialized"], true);
	assert_eq!(body["chapterCount"], 8);

	// 章节：8 章按 source_order
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters array");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0]["chapter_number"], json!(8.0));
	assert!(chapters.iter().all(|c| c["id"].is_i64() && c["pageCount"].is_i64()));
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");

	// 页：2 页，Url + Text
	let pages_path = format!("/api/v1/chapter/{chapter_id}/pages");
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	assert_eq!(pages[0]["index"], 0);
	assert_eq!(
		pages[0]["content"]["Url"][0],
		"https://aidoku.app/images/icon.png"
	);
	assert_eq!(pages[1]["index"], 1);
	assert!(
		pages[1]["content"]["Text"]
			.as_str()
			.expect("text page")
			.starts_with("# Title")
	);

	// 第二次走 page 缓存
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("cached pages").len(), 2);

	// 设置热重启：setting=true → settings 多出一个 Toggle
	let settings_path = format!("/api/v1/sources/{source_id}/settings");
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 2);
	assert_eq!(body["values"]["setting"], true);

	// setting=false → 只剩一个
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": false}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 1);
	assert_eq!(body["values"]["setting"], false);
}
