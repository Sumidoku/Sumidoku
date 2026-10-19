//! 备选封面端点：GET /api/v1/manga/{id}/covers/alternates 从源取回备选列表，
//! PUT /api/v1/manga/{id}/cover 把选择落库为 manga.cover_url 并让封面磁盘缓存
//! （本体 + sidecar）失效，下一次 GET cover 按新 URL 回源取图。fixture 源在
//! crates/wasm/tests/fixtures/altcovers-src（no_std cdylib），重建方式：
//! `cd crates/wasm/tests/fixtures/altcovers-src && cargo build --release --target wasm32-unknown-unknown`，
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_altcovers.wasm
//! 复制为 crates/wasm/tests/fixtures/altcovers/main.wasm。
mod common;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use common::{
	TestApp, altcovers_wasm_path, base_config, connect_db, install_fixture_source,
	pagedesc_wasm_path, request, request_full,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use sumidoku_server::build_app;

// 三张备选各回不同字节，断言取回的是哪一张；服务端不校验图片字节
const PNG_ONE: &[u8] = b"fake-png-one";
const PNG_TWO: &[u8] = b"fake-png-two";
const PNG_THREE: &[u8] = b"fake-png-three";

type SeenPaths = Arc<Mutex<Vec<String>>>;

/// 记录请求路径的微型图床：/cover-N.png 按 N 回不同字节
async fn serve_images() -> (u16, SeenPaths, tokio::task::JoinHandle<()>) {
	let seen: SeenPaths = Arc::new(Mutex::new(Vec::new()));
	let seen_handler = seen.clone();
	let router = Router::new().fallback(move |request: Request| {
		let seen = seen_handler.clone();
		async move {
			let path = request.uri().path().to_string();
			seen.lock().expect("seen lock").push(path.clone());
			let body = match path.as_str() {
				"/cover-1.png" => PNG_ONE,
				"/cover-2.png" => PNG_TWO,
				"/cover-3.png" => PNG_THREE,
				_ => b"unknown",
			};
			([(axum::http::header::CONTENT_TYPE, "image/png")], body.to_vec())
		}
	});
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, seen, handle)
}

/// altcovers（有 AlternateCoverProvider）+ pagedesc（无导出，501 对照）双源的 app
async fn dual_source_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.altcovers", "AltCovers Source", &altcovers_wasm_path());
	install_fixture_source(&tmp, "en.pagedesc", "PageDesc Source", &pagedesc_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn source_id_by_key(app: &Router, key: &str) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("source id")
}

async fn register_manga(app: &Router, source_id: i64) -> i64 {
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

#[tokio::test]
async fn lists_alternate_covers() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		body,
		json!([
			"https://altcovers.fixture/cover-1.png",
			"https://altcovers.fixture/cover-2.png",
			"https://altcovers.fixture/cover-3.png",
		])
	);
}

#[tokio::test]
async fn selection_persists_and_is_served() {
	let (port, seen, _upstream) = serve_images().await;
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	// coverBase 指到 mock 图床，备选列表变成能真实取图的本地 URL（注册前写入，
	// 实例懒加载时即从库读到）
	let base = format!("http://127.0.0.1:{port}");
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"coverBase": base})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let covers: Vec<String> = serde_json::from_value(body).expect("covers array");
	assert_eq!(
		covers,
		vec![
			format!("{base}/cover-1.png"),
			format!("{base}/cover-2.png"),
			format!("{base}/cover-3.png"),
		]
	);

	// 选第一张：取图后落磁盘缓存（本体 + sidecar）
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": covers[0]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!(covers[0]));
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/cover"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, PNG_ONE);
	let cache = t.tmp.path().join("covers").join(manga_id.to_string());
	let sidecar = t.tmp.path().join("covers").join(format!("{manga_id}.content-type"));
	assert!(cache.is_file(), "cover cache body should exist");
	assert!(sidecar.is_file(), "cover cache sidecar should exist");

	// 换第二张：落库的同时缓存连 sidecar 一起清，再 GET 按新 URL 回源取图
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": covers[1]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!(covers[1]));
	assert!(!cache.exists(), "stale cover cache should be removed");
	assert!(!sidecar.exists(), "stale sidecar should be removed");
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/cover"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, PNG_TWO);

	let pool = connect_db(&t.db_path).await;
	let (cover_url,): (Option<String>,) = sqlx::query_as("SELECT cover_url FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("cover_url row");
	assert_eq!(cover_url.as_deref(), Some(covers[1].as_str()));

	// 两张各回源一次：第二张不是旧缓存顶替的
	assert_eq!(
		*seen.lock().expect("seen lock"),
		vec!["/cover-1.png".to_string(), "/cover-2.png".to_string()]
	);
}

#[tokio::test]
async fn select_rejects_bad_cover_url() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	let manga_id = register_manga(&app, source_id).await;

	for url in ["file:///etc/passwd", "not a url", "ftp://127.0.0.1/c.png"] {
		let (status, _) = request(
			&app,
			"PUT",
			&format!("/api/v1/manga/{manga_id}/cover"),
			Some(json!({"url": url})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "url={url}");
	}
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!("https://altcovers.fixture/cover-0.png"));
}

#[tokio::test]
async fn alternates_unsupported_is_501() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.pagedesc").await;
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], json!("unsupported"));
}

#[tokio::test]
async fn unknown_manga_is_404() {
	let t = dual_source_app().await;
	let app = t.app.clone();

	let (status, _) = request(&app, "GET", "/api/v1/manga/9999/covers/alternates", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(
		&app,
		"PUT",
		"/api/v1/manga/9999/cover",
		Some(json!({"url": "https://altcovers.fixture/cover-1.png"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}
