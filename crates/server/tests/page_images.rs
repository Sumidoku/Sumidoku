//! Page image serving contract: original bytes pass through byte for byte (a legacy `?width=`
//! downsample parameter is ignored — scaling is the client's job), downloads and CBZ packing
//! serve byte-identical originals, and a source refresh replaces the served generation
mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, base_config, canvas_wasm_path, connect_db, install_fixture_source, register_example_manga, request, request_full, test_app};
use image::ImageReader;
use serde_json::json;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use sumidoku_server::build_app;

// matches the issue's 4K page magnitude: 3000x4500 RGBA ≈ 54MB
const BIG_W: u32 = 3000;
const BIG_H: u32 = 4500;

fn gradient_png(w: u32, h: u32) -> Vec<u8> {
	let image = image::RgbImage::from_fn(w, h, |x, y| {
		image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 255) as u8])
	});
	let mut out = Vec::new();
	let encoder = image::codecs::png::PngEncoder::new_with_quality(
		&mut out,
		image::codecs::png::CompressionType::Fast,
		image::codecs::png::FilterType::Adaptive,
	);
	image::ImageBuffer::write_with_encoder(&image, encoder).expect("encode png");
	out
}

fn decode_dims(bytes: &[u8]) -> (u32, u32) {
	let image = ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.expect("guessed format")
		.decode()
		.expect("decode");
	(image.width(), image.height())
}

struct Upstream {
	port: u16,
	small_hits: Arc<AtomicUsize>,
}

async fn serve_upstream(big: Vec<u8>, small: Vec<u8>) -> Upstream {
	let big_hits = Arc::new(AtomicUsize::new(0));
	let small_hits = Arc::new(AtomicUsize::new(0));
	let counter = |hits: Arc<AtomicUsize>, bytes: Vec<u8>| {
		move || {
			let hits = hits.clone();
			let bytes = bytes.clone();
			async move {
				hits.fetch_add(1, Ordering::SeqCst);
				([(axum::http::header::CONTENT_TYPE, "image/png")], bytes)
			}
		}
	};
	let router = Router::new()
		.route("/big.png", get(counter(big_hits.clone(), big)))
		.route("/small.png", get(counter(small_hits.clone(), small)));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	Upstream {
		port,
		small_hits,
	}
}

async fn rewrite_page(db_path: &std::path::Path, chapter_id: i64, idx: i64, url: &str) {
	let pool = connect_db(db_path).await;
	let stored = json!({
		"content": {"Url": [url, null]},
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	});
	sqlx::query("UPDATE page SET content = ? WHERE chapter_id = ? AND idx = ?")
		.bind(stored.to_string())
		.bind(chapter_id)
		.bind(idx)
		.execute(&pool)
		.await
		.expect("rewrite page");
}

async fn url_page_app() -> (TestApp, i64, Upstream, Vec<u8>, Vec<u8>) {
	let big = gradient_png(BIG_W, BIG_H);
	let small = gradient_png(8, 8);
	let upstream = serve_upstream(big.clone(), small.clone()).await;
	let t = test_app().await;
	let manga_id = register_example_manga(&t.app).await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	rewrite_page(&t.db_path, chapter_id, 0, &format!("http://127.0.0.1:{}/big.png", upstream.port)).await;
	rewrite_page(
		&t.db_path,
		chapter_id,
		1,
		&format!("http://127.0.0.1:{}/small.png", upstream.port),
	)
	.await;
	(t, chapter_id, upstream, big, small)
}

#[tokio::test]
async fn original_bytes_pass_through_and_a_legacy_width_parameter_is_ignored() {
	let (t, chapter_id, upstream, big, small) = url_page_app().await;
	let page_url = |idx: i64| format!("/api/v1/chapter/{chapter_id}/page/{idx}");

	// original bytes pass through byte for byte
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);

	// a legacy downsample width no longer resizes: the same original bytes come back
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=1536", page_url(0)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=256", page_url(1)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, small);

	// the persisted Url-page original serves repeat reads without going back to the source
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(1), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, small);
	assert_eq!(upstream.small_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn svg_original_bytes_and_mime_pass_through() {
	let (t, chapter_id, _upstream, _big, _small) = url_page_app().await;
	let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>"#.to_vec();
	let (port, _svg_upstream) = common::serve_bytes(vec![("/page.svg", "image/svg+xml", svg.clone())]).await;
	rewrite_page(&t.db_path, chapter_id, 1, &format!("http://127.0.0.1:{port}/page.svg")).await;
	let (status, headers, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/svg+xml");
	assert_eq!(bytes, svg);
}

async fn canvas_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.canvas", "Canvas Source", &canvas_wasm_path());
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
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

#[tokio::test]
async fn blob_download_is_byte_identical_and_refresh_uses_product() {
	let t = canvas_app().await;
	let manga_id = register_canvas_manga(&t.app).await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, initial_pages) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let initial_revision = initial_pages[0]["cacheRevision"].as_i64().expect("initial cache revision");

	// swap the materialized blob for a synthetic big image (the page row already points at 0.png, only the file content changes)
	let big = gradient_png(BIG_W, BIG_H);
	let blob_dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	std::fs::write(blob_dir.join("0.png"), &big).expect("overwrite blob");
	let page_url = |idx: i64| format!("/api/v1/chapter/{chapter_id}/page/{idx}");

	// the blob page passes through byte for byte
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);

	// downloads/CBZ packing stay byte-identical to the blob
	let (status, _) = request(&t.app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&t.app,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let deadline = Instant::now() + Duration::from_secs(15);
	let entry = loop {
		let (status, body) = request(&t.app, "GET", "/api/v1/downloads", None).await;
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
	let cleanup_deadline = Instant::now() + Duration::from_secs(5);
	loop {
		let (status, pages) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
		assert_eq!(status, StatusCode::OK);
		let revision = pages[0]["cacheRevision"].as_i64().expect("cache revision");
		if revision > initial_revision && !blob_dir.exists() { break; }
		assert!(Instant::now() < cleanup_deadline, "download cleanup did not advance cache revision: {pages}");
		tokio::task::yield_now().await;
	}
	let rel_path = entry["path"].as_str().expect("download path");
	let cbz = std::fs::read(t.tmp.path().join("downloads").join(rel_path)).expect("read cbz");
	let mut archive = zip::ZipArchive::new(Cursor::new(cbz)).expect("open cbz");
	let mut packed = Vec::new();
	{
		use std::io::Read;
		archive
			.by_name("000.png")
			.expect("cbz page entry")
			.read_to_end(&mut packed)
			.expect("read entry");
	}
	assert_eq!(packed, big, "cbz page bytes must stay byte-identical");

	// A finished product removes source blobs; force refresh keeps the product authoritative.
	assert!(!blob_dir.exists(), "successful download cleanup removes materialized blobs");
	let (status, _) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages?refresh=true"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(!blob_dir.exists(), "force refresh keeps using the product without rebuilding source assets");
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(decode_dims(&bytes), (BIG_W, BIG_H), "finished product stays authoritative after refresh");
	assert_eq!(bytes, big);
}

#[tokio::test]
async fn refreshing_image_pages_serves_the_new_generation() {
	let t = canvas_app().await;
	let manga_id = register_canvas_manga(&t.app).await;
	let (_, chapters) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter");
	let pages_url = format!("/api/v1/chapter/{chapter_id}/pages");
	let (_, first_pages) = request(&t.app, "GET", &pages_url, None).await;
	let first_revision = first_pages[0]["cacheRevision"].as_i64().expect("revision");
	let blob = t.tmp.path().join(format!("page-blobs/{chapter_id}/0.png"));
	std::fs::write(&blob, gradient_png(640, 480)).expect("wide image fixture");
	let image_url = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, _, previous) = request_full(&t.app, "GET", &image_url, None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, refreshed) = request(&t.app, "GET", &format!("{pages_url}?refresh=true"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(refreshed[0]["cacheRevision"].as_i64().expect("new revision") > first_revision);
	let fresh_original = std::fs::read(&blob).expect("fresh Image under same filename");
	let (status, _, bytes) = request_full(&t.app, "GET", &image_url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, fresh_original, "an explicit source refresh must show the new Image generation when there is no finished product");
	assert_ne!(bytes, previous);
}
