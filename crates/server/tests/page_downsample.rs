//! 页图降采样契约：?width= 变体（尺寸/256 量化/磁盘缓存/不放大）与无参原图字节不变；
//! 下载与 CBZ 打包不经过降采样分支，字节与今天一致
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

// 对齐 issue 的 4K 页量级：3000x4500 RGBA ≈ 54MB
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
	big_hits: Arc<AtomicUsize>,
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
		big_hits,
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
async fn downsampled_variant_is_resized_quantized_and_cached() {
	let (t, chapter_id, upstream, big, _small) = url_page_app().await;
	let page_url = format!("/api/v1/chapter/{chapter_id}/page/0");

	let (status, headers, bytes) = request_full(&t.app, "GET", &format!("{page_url}?width=1536"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(decode_dims(&bytes), (1536, BIG_H * 1536 / BIG_W));
	assert!(bytes.len() < big.len(), "variant must be smaller than original");
	assert_eq!(upstream.big_hits.load(Ordering::SeqCst), 1);

	// 变体落盘：同桶再次请求不再回源
	let (status, _, _) = request_full(&t.app, "GET", &format!("{page_url}?width=1536"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(upstream.big_hits.load(Ordering::SeqCst), 1);

	// 宽度向上量化到 256 的整数倍：1500 与 1536 共用一个变体
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{page_url}?width=1500"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(decode_dims(&bytes), (1536, BIG_H * 1536 / BIG_W));
	assert_eq!(upstream.big_hits.load(Ordering::SeqCst), 1);

	// 不同桶各存各的变体，互不污染
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{page_url}?width=256"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(decode_dims(&bytes), (256, BIG_H * 256 / BIG_W));
	assert_eq!(upstream.big_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn original_bytes_untouched_without_width_or_when_narrow() {
	let (t, chapter_id, upstream, big, small) = url_page_app().await;
	let page_url = |idx: i64| format!("/api/v1/chapter/{chapter_id}/page/{idx}");

	// 无 width 参数：原图字节逐字节直通
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);

	// 请求宽度不小于原图：不放大，原字节直通
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=8192", page_url(0)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);

	// 窄于请求宽度的图原样直通，且直通的 Url 页每次回源取新鲜原图
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=1536", page_url(1)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, small);
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(1), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, small);
	assert_eq!(upstream.small_hits.load(Ordering::SeqCst), 2);
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
async fn blob_variant_download_byte_identical_and_refresh_wipes_variants() {
	let t = canvas_app().await;
	let manga_id = register_canvas_manga(&t.app).await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);

	// 物化 blob 换成合成大图（页行已指向 0.png，只换文件内容）
	let big = gradient_png(BIG_W, BIG_H);
	let blob_dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	std::fs::write(blob_dir.join("0.png"), &big).expect("overwrite blob");
	let small_blob = std::fs::read(blob_dir.join("1.png")).expect("read small blob");
	let page_url = |idx: i64| format!("/api/v1/chapter/{chapter_id}/page/{idx}");

	// blob 路径降采样
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=1536", page_url(0)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(decode_dims(&bytes), (1536, BIG_H * 1536 / BIG_W));

	// 无参与窄页仍逐字节直通
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, big);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("{}?width=1536", page_url(1)), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, small_blob);

	// 下载/CBZ 不经过降采样分支：打包字节与 blob 逐字节一致
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

	// 整章刷新重写 blob 目录时降采样变体一并失效
	assert!(blob_dir.join("variants").exists(), "variant cache must exist before refresh");
	let (status, _) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages?refresh=true"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		!blob_dir.join("variants").exists(),
		"chapter reload must wipe downsampled variants"
	);
	let (status, _, bytes) = request_full(&t.app, "GET", &page_url(0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(decode_dims(&bytes), (8, 8), "refresh restores the materialized blob");
}
