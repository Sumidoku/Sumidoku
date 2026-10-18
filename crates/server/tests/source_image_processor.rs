//! 加密（不可解码）封面/页图走图像处理器的端到端回归（issue #116）：
//! 声明处理器的源拿到原始响应字节，密文不再 502，五条入口链路输出一致的正常 PNG。
//! fixture 源在 crates/wasm/tests/fixtures/rawimg-src（no_std cdylib），
//! 重建方式：`cd crates/wasm/tests/fixtures/rawimg-src && cargo build --release --target wasm32-unknown-unknown`，
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_rawimg.wasm
//! 复制为 crates/wasm/tests/fixtures/rawimg/main.wasm。
mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use common::{TestApp, base_config, connect_db, install_fixture_source, rawimg_wasm_path, request, request_full};
use image::ImageReader;
use serde_json::json;
use std::io::Cursor;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use tower::ServiceExt;

/// 与 rawimg-src/src/lib.rs 对账的同一密钥与明文（2x2 RGBA 红绿/蓝白）
const KEY: u8 = 0x5A;
const PLAIN_PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0,
	0, 0, 114, 182, 13, 36, 0, 0, 0, 18, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 12,
	129, 52, 24, 0, 0, 73, 200, 9, 247, 249, 171, 182, 13, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
	130,
];

fn cipher() -> Vec<u8> {
	PLAIN_PNG.iter().map(|b| b ^ KEY).collect()
}

/// 处理器输出的 PNG 必须就是 plain.png 的 2x2 像素
fn assert_plain_pixels(png: &[u8], label: &str) {
	let image = ImageReader::new(Cursor::new(png))
		.with_guessed_format()
		.expect("png format")
		.decode()
		.expect("png decode")
		.to_rgba8();
	assert_eq!(image.dimensions(), (2, 2), "{label} dimensions");
	for (x, y, px) in image.enumerate_pixels() {
		let expected = match (x, y) {
			(0, 0) => [255, 0, 0, 255],
			(1, 0) => [0, 255, 0, 255],
			(0, 1) => [0, 0, 255, 255],
			_ => [255, 255, 255, 255],
		};
		assert_eq!(px.0, expected, "{label} pixel ({x},{y})");
	}
}

/// 微型图床：/cipher 回密文（octet-stream，无图像 magic），/plain 回明文 PNG
async fn serve_images() -> (u16, tokio::task::JoinHandle<()>) {
	let router = Router::new()
		.route(
			"/cipher",
			get(|| async { ([(header::CONTENT_TYPE, "application/octet-stream")], cipher()) }),
		)
		.route(
			"/plain",
			get(|| async { ([(header::CONTENT_TYPE, "image/png")], PLAIN_PNG.to_vec()) }),
		);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, handle)
}

async fn rawimg_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.rawimg", "Rawimg Source", &rawimg_wasm_path());
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn register_rawimg_manga(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = body
		.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == "en.rawimg")
		.and_then(|s| s["id"].as_i64())
		.expect("rawimg source id");
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

async fn rawimg_source_id(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == "en.rawimg")
		.and_then(|s| s["id"].as_i64())
		.expect("rawimg source id")
}

/// 占位 Url 页改写成 mock 图床地址（页内容形状 mirrors canvas_pipeline 的改写）
async fn rewrite_page_url(t: &TestApp, chapter_id: i64, url: String) {
	let pool = connect_db(&t.db_path).await;
	let stored = json!({
		"content": {"Url": [url, null]},
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	});
	sqlx::query("UPDATE page SET content = ? WHERE chapter_id = ? AND idx = 0")
		.bind(stored.to_string())
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("rewrite page 0");
}

async fn set_cover_url(t: &TestApp, manga_id: i64, url: String) {
	let pool = connect_db(&t.db_path).await;
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?")
		.bind(url)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update cover url");
}

async fn chapter_id_of(app: &Router, manga_id: i64) -> i64 {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("chapter id")
}

/// browse/home 封面代理同样过处理器：密文不再原样透传（#102 入口的关联缺口）
#[tokio::test]
async fn proxy_image_processes_opaque_and_plain_bytes() {
	let t = rawimg_app().await;
	let app = t.app.clone();
	let source_id = rawimg_source_id(&app).await;
	let (port, _upstream) = serve_images().await;

	for path in ["cipher", "plain"] {
		let (status, headers, bytes) = request_full(
			&app,
			"GET",
			&format!("/api/v1/sources/{source_id}/image?url=http://127.0.0.1:{port}/{path}"),
			None,
		)
		.await;
		assert_eq!(status, StatusCode::OK, "proxy {path}");
		assert_eq!(headers["content-type"], "image/png", "proxy {path}");
		assert_plain_pixels(&bytes, "proxy {path}");
	}
}

/// 详情页封面：加密字节过 cover 处理器出 PNG（修复前是 502 source_error）
#[tokio::test]
async fn manga_cover_processes_opaque_bytes() {
	let t = rawimg_app().await;
	let app = t.app.clone();
	let manga_id = register_rawimg_manga(&app).await;
	let (port, _upstream) = serve_images().await;
	set_cover_url(&t, manga_id, format!("http://127.0.0.1:{port}/cipher")).await;

	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_plain_pixels(&bytes, "manga cover");
}

/// reader 页图：加密字节过 page 处理器出 PNG
#[tokio::test]
async fn reader_page_processes_opaque_bytes() {
	let t = rawimg_app().await;
	let app = t.app.clone();
	let manga_id = register_rawimg_manga(&app).await;
	let chapter_id = chapter_id_of(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_images().await;
	rewrite_page_url(&t, chapter_id, format!("http://127.0.0.1:{port}/cipher")).await;

	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_plain_pixels(&bytes, "reader page");
}

/// 下载与 reader 同一条 fetch_page_image：cbz 里是解密后的 PNG
#[tokio::test]
async fn download_processes_opaque_bytes() {
	let t = rawimg_app().await;
	let app = t.app.clone();
	let manga_id = register_rawimg_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = chapter_id_of(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_images().await;
	rewrite_page_url(&t, chapter_id, format!("http://127.0.0.1:{port}/cipher")).await;

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
			.filter(|entry| matches!(entry["status"].as_str(), Some("finished" | "error")))
		{
			break entry.clone();
		}
		assert!(Instant::now() < deadline, "download did not settle: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	};
	assert_eq!(entry["status"], "finished", "download entry: {entry}");

	let rel_path = entry["path"].as_str().expect("download path");
	let cbz = std::fs::read(t.tmp.path().join("downloads").join(rel_path)).expect("read cbz");
	let mut archive = zip::ZipArchive::new(Cursor::new(cbz)).expect("open cbz");
	assert_eq!(archive.len(), 2, "1 page + ComicInfo");
	use std::io::Read;
	let mut data = Vec::new();
	archive
		.by_name("000.png")
		.expect("cbz page entry")
		.read_to_end(&mut data)
		.expect("read page entry");
	assert_plain_pixels(&data, "download page");
}

/// OPDS 封面与详情页封面同一条 fetch_cover_image：输出一致
#[tokio::test]
async fn opds_cover_processes_opaque_bytes() {
	let t = rawimg_app().await;
	let app = t.app.clone();
	let manga_id = register_rawimg_manga(&app).await;
	let (port, _upstream) = serve_images().await;
	set_cover_url(&t, manga_id, format!("http://127.0.0.1:{port}/cipher")).await;

	let request = Request::builder()
		.method("GET")
		.uri(format!("/opds/v1.2/cover/{manga_id}"))
		.header(header::HOST, "opds.test")
		.header("x-forwarded-proto", "https")
		.body(Body::empty())
		.expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	assert_eq!(response.status(), StatusCode::OK);
	assert_eq!(response.headers()["content-type"], "image/png");
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	assert_plain_pixels(&bytes, "opds cover");
}
