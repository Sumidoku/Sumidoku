//! End-to-end canvas byte pipeline: the fixture source (tests/fixtures/canvas-src) emits Image pages,
//! exercised once through each of three paths: reader endpoint, download-to-disk, and
//! process_page_image/process_cover_image.
mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, base_config, canvas_wasm_path, connect_db, install_fixture_source, request, request_full};
use image::ImageReader;
use serde_json::json;
use std::io::Cursor;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;

// 1x1 transparent PNG, the upstream image-host payload for the processor path
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

async fn canvas_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.canvas", "Canvas Source", &canvas_wasm_path());
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp::new(app, tmp, db_path)
}

async fn register_canvas_manga(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = body
		.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == "en.canvas")
		.and_then(|s| s["id"].as_i64())
		.expect("canvas source id");
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

async fn canvas_chapter(app: &Router, manga_id: i64) -> i64 {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("chapter id")
}

fn decode_png(bytes: &[u8]) -> image::RgbaImage {
	ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.expect("png format")
		.decode()
		.expect("png decode")
		.to_rgba8()
}

fn assert_solid(image: &image::RgbaImage, expected: [u8; 4], label: &str) {
	for (x, y, px) in image.enumerate_pixels() {
		assert_eq!(px.0, expected, "{label} pixel ({x},{y})");
	}
}

async fn serve_bytes(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> (u16, tokio::task::JoinHandle<()>) {
	let mut router = Router::new();
	for (path, mime, bytes) in routes {
		let handler = move || {
			let mime = mime.to_string();
			let bytes = bytes.clone();
			async move { ([(axum::http::header::CONTENT_TYPE, mime)], bytes) }
		};
		router = router.route(path, get(handler));
	}
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, handle)
}

async fn fetch_page_png(app: &Router, chapter_id: i64, index: i64) -> Vec<u8> {
	let (status, headers, bytes) =
		request_full(app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	bytes
}

#[tokio::test]
async fn image_pages_read_through_reader_endpoint() {
	let t = canvas_app().await;
	let app = t.app.clone();
	let manga_id = register_canvas_manga(&app).await;
	let chapter_id = canvas_chapter(&app, manga_id).await;

	// Triggers load_pages materialization; the persisted content should be reduced to just the blob file name
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let (stored,): (String,) = sqlx::query_as("SELECT content FROM page WHERE chapter_id = ? AND idx = 0")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("page row");
	let content: serde_json::Value = serde_json::from_str(&stored).expect("page json");
	assert_eq!(content["content"], json!({"Image": "0.png"}));

	let red = decode_png(&fetch_page_png(&app, chapter_id, 0).await);
	assert_eq!(red.dimensions(), (8, 8));
	assert_solid(&red, [255, 0, 0, 255], "red page");

	let green = decode_png(&fetch_page_png(&app, chapter_id, 1).await);
	assert_eq!(green.dimensions(), (4, 8));
	assert_solid(&green, [0, 255, 0, 255], "cropped page");

	let text = decode_png(&fetch_page_png(&app, chapter_id, 2).await);
	assert_eq!(text.dimensions(), (32, 16));
	assert!(
		text.pixels().any(|px| px.0 != [255, 255, 255, 255]),
		"draw_text must paint glyphs"
	);
}

#[tokio::test]
async fn image_pages_download_as_png() {
	let t = canvas_app().await;
	let app = t.app.clone();
	let manga_id = register_canvas_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = canvas_chapter(&app, manga_id).await;

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

	let rel_path = entry["path"].as_str().expect("download path");
	let cbz = std::fs::read(t.tmp.path().join("downloads").join(rel_path)).expect("read cbz");
	let mut archive = zip::ZipArchive::new(Cursor::new(cbz)).expect("open cbz");
	assert_eq!(archive.len(), 4, "3 pages + ComicInfo");
	for (name, dims, color) in [
		("000.png", (8, 8), [255, 0, 0, 255]),
		("001.png", (4, 8), [0, 255, 0, 255]),
	] {
		use std::io::Read;
		let mut data = Vec::new();
		archive
			.by_name(name)
			.expect("cbz entry")
			.read_to_end(&mut data)
			.expect("read entry");
		let image = decode_png(&data);
		assert_eq!(image.dimensions(), dims);
		assert_solid(&image, color, name);
	}
}

#[tokio::test]
async fn removed_image_product_rebuilds_blobs_for_read_and_redownload() {
	let t = canvas_app().await;
	let app = t.app.clone();
	let manga_id = register_canvas_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = canvas_chapter(&app, manga_id).await;
	let (status, initial_pages) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let mut previous_revision = initial_pages[0]["cacheRevision"].as_i64().expect("initial cache revision");
	let blob_dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());

	for round in 0..2 {
		let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
		assert_eq!(status, StatusCode::OK);
		let deadline = Instant::now() + Duration::from_secs(15);
		loop {
			let (status, body) = request(&app, "GET", "/api/v1/downloads", None).await;
			assert_eq!(status, StatusCode::OK);
			if body.as_array().and_then(|rows| rows.iter().find(|row| row["chapterId"] == chapter_id)).is_some_and(|entry| entry["status"] == "finished") { break; }
			assert!(Instant::now() < deadline, "download round {round} did not finish: {body}");
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
		let cleanup_deadline = Instant::now() + Duration::from_secs(5);
		loop {
			let (status, pages) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
			assert_eq!(status, StatusCode::OK);
			let revision = pages[0]["cacheRevision"].as_i64().expect("cache revision");
			if revision > previous_revision && !blob_dir.exists() { previous_revision = revision; break; }
			assert!(Instant::now() < cleanup_deadline, "download cleanup did not advance cache revision: {pages}");
			tokio::task::yield_now().await;
		}
		assert!(!blob_dir.exists(), "finished product cleanup removes materialized Image blobs");
		let red = decode_png(&fetch_page_png(&app, chapter_id, 0).await);
		assert_solid(&red, [255, 0, 0, 255], "product page after cleanup");
		if round == 0 {
			let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
			assert_eq!(status, StatusCode::NO_CONTENT);
			let (status, pages) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
			assert_eq!(status, StatusCode::OK);
			previous_revision = pages[0]["cacheRevision"].as_i64().expect("delete cache revision");
			let red = decode_png(&fetch_page_png(&app, chapter_id, 0).await);
			assert_solid(&red, [255, 0, 0, 255], "source page after product removal");
			assert!(blob_dir.join("0.png").exists(), "missing Image row was rematerialized before reading");
		}
	}
}

#[tokio::test]
async fn processors_transform_fetched_images() {
	let t = canvas_app().await;
	let app = t.app.clone();
	let manga_id = register_canvas_manga(&app).await;
	let chapter_id = canvas_chapter(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);

	// A Url page goes through process_page_image: the fixture always returns a 2x2 blue block
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	let stored = json!({
		"content": {"Url": [format!("http://127.0.0.1:{port}/img.png"), null]},
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
	let blue = decode_png(&fetch_page_png(&app, chapter_id, 0).await);
	assert_eq!(blue.dimensions(), (2, 2));
	assert_solid(&blue, [0, 0, 255, 255], "processed page");

	// The cover goes through process_cover_image: the fixture always returns a 3x3 green block
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?")
		.bind(format!("http://127.0.0.1:{port}/img.png"))
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update cover url");
	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	let green = decode_png(&bytes);
	assert_eq!(green.dimensions(), (3, 3));
	assert_solid(&green, [0, 255, 0, 255], "processed cover");
}
