//! 页描述（PageDescriptionProvider）管线端到端：fixture 源（tests/fixtures/pagedesc-src）
//! 的 hasDescription 页在页列表载荷里带回源取到的描述文本；无标志页、页内容端点与
//! 下载物化路径保持原样；导出失败按整章失败露出 source_error。
//! fixture 重建：`cd tests/fixtures/pagedesc-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_pagedesc.wasm
//! 复制为 tests/fixtures/pagedesc/main.wasm。
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, connect_db, install_fixture_source, pagedesc_wasm_path, request};
use serde_json::json;
use std::io::Cursor;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;

async fn pagedesc_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.pagedesc", "PageDesc Source", &pagedesc_wasm_path());
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn register_pagedesc_manga(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = body
		.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == "en.pagedesc")
		.and_then(|s| s["id"].as_i64())
		.expect("pagedesc source id");
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

async fn chapter_id_by_key(app: &Router, manga_id: i64, key: &str) -> i64 {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters")
		.iter()
		.find(|c| c["key"] == key)
		.and_then(|c| c["id"].as_i64())
		.unwrap_or_else(|| panic!("chapter {key}"))
}

#[tokio::test]
async fn flagged_pages_carry_fetched_description() {
	let t = pagedesc_app().await;
	let app = t.app.clone();
	let manga_id = register_pagedesc_manga(&app).await;

	let chapter_id = chapter_id_by_key(&app, manga_id, "ch-texts").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let pages = body.as_array().expect("pages");
	assert_eq!(pages.len(), 3);

	// 无 description 的标志页：回源取文本
	assert_eq!(pages[0]["hasDescription"], json!(true));
	assert_eq!(pages[0]["description"], json!("fetched: page zero body"));
	assert_eq!(pages[0]["content"], json!({"Text": "page zero body"}));

	// 无标志页：不碰，description 保持 null
	assert_eq!(pages[1]["hasDescription"], json!(false));
	assert_eq!(pages[1]["description"], json!(null));

	// 内联 description 的标志页：不得再调 get_page_description
	// （fixture 对该 content 恒 bail，误调会让整章 502）
	assert_eq!(pages[2]["hasDescription"], json!(true));
	assert_eq!(pages[2]["description"], json!("inline description"));

	// Url 内容页：取描述与内容类型无关，content 原样保留
	let chapter_id = chapter_id_by_key(&app, manga_id, "ch-url").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let pages = body.as_array().expect("pages");
	assert_eq!(pages.len(), 1);
	assert_eq!(pages[0]["hasDescription"], json!(true));
	assert_eq!(
		pages[0]["description"],
		json!("fetched: https://example.invalid/p0.png")
	);
	assert_eq!(
		pages[0]["content"],
		json!({"Url": ["https://example.invalid/p0.png", null]})
	);
}

#[tokio::test]
async fn unflagged_page_content_endpoint_unchanged() {
	let t = pagedesc_app().await;
	let app = t.app.clone();
	let manga_id = register_pagedesc_manga(&app).await;
	let chapter_id = chapter_id_by_key(&app, manga_id, "ch-texts").await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!({"type": "text", "text": "plain body"}));
}

#[tokio::test]
async fn description_export_failure_surfaces_source_error() {
	let t = pagedesc_app().await;
	let app = t.app.clone();
	let manga_id = register_pagedesc_manga(&app).await;
	let chapter_id = chapter_id_by_key(&app, manga_id, "ch-fail").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);
	assert_eq!(body["error"]["code"], json!("source_error"));

	// 失败的整章不落半成品页：page 表保持空，重试仍走回源（同样报错）
	let pool = connect_db(&t.db_path).await;
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM page WHERE chapter_id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("page count");
	assert_eq!(count, 0);
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn downloads_materialize_pages_unchanged() {
	let t = pagedesc_app().await;
	let app = t.app.clone();
	let manga_id = register_pagedesc_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = chapter_id_by_key(&app, manga_id, "ch-texts").await;

	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let deadline = Instant::now() + Duration::from_secs(15);
	let entry = loop {
		let (status, body) = request(&app, "GET", "/api/v1/downloads", None).await;
		assert_eq!(status, StatusCode::OK);
		if let Some(entry) = body
			.as_array()
			.and_then(|rows| rows.iter().find(|row| row["chapterId"] == chapter_id))
			.filter(|entry| entry["status"] == "finished")
		{
			break entry.clone();
		}
		assert!(Instant::now() < deadline, "download did not finish: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	};

	// cbz 里装的是页 content（Text 页成 txt），不是描述文本
	let rel_path = entry["path"].as_str().expect("download path");
	let cbz = std::fs::read(t.tmp.path().join("downloads").join(rel_path)).expect("read cbz");
	let mut archive = zip::ZipArchive::new(Cursor::new(cbz)).expect("open cbz");
	assert_eq!(archive.len(), 4, "3 pages + ComicInfo");
	use std::io::Read;
	let mut text = String::new();
	archive
		.by_name("000.txt")
		.expect("cbz entry")
		.read_to_string(&mut text)
		.expect("read entry");
	assert_eq!(text, "page zero body");
}
