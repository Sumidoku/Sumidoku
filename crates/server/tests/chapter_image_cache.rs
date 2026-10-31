//! Persistent remote image originals, download reuse/cleanup, and finished-product reader precedence.
mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{
	TestApp, base_config, connect_db, register_example_manga, request, request_full, test_app,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use sumidoku_server::build_app;

struct Upstream {
	port: u16,
	first_hits: Arc<AtomicUsize>,
	second_hits: Arc<AtomicUsize>,
	first: Vec<u8>,
	second: Vec<u8>,
}

fn payload(seed: u32) -> Vec<u8> {
	let image = image::RgbImage::from_fn(640, 640, |x, y| {
		let mut value = x.wrapping_mul(747_796_405) ^ y.wrapping_mul(2_891_336_453) ^ seed;
		value = value.wrapping_mul(277_803_737);
		image::Rgb([(value >> 16) as u8, (value >> 8) as u8, value as u8])
	});
	let mut bytes = Vec::new();
	image::DynamicImage::ImageRgb8(image)
		.write_to(
			&mut std::io::Cursor::new(&mut bytes),
			image::ImageFormat::Png,
		)
		.expect("encode image");
	assert!(
		bytes.len() >= 300 * 1024,
		"fixture should exercise a real image response: {} bytes",
		bytes.len()
	);
	bytes
}

fn zip_payload(name: &str, bytes: &[u8]) -> Vec<u8> {
	use std::io::Write;
	let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
	archive
		.start_file(name, zip::write::SimpleFileOptions::default())
		.expect("zip entry");
	archive.write_all(bytes).expect("zip image");
	archive.finish().expect("finish zip").into_inner()
}

async fn upstream() -> Upstream {
	let first = payload(1);
	let second = payload(2);
	let zip = zip_payload("page/1.png", &second);
	let first_hits = Arc::new(AtomicUsize::new(0));
	let second_hits = Arc::new(AtomicUsize::new(0));
	let handler = |hits: Arc<AtomicUsize>, bytes: Vec<u8>, mime: &'static str| {
		move || {
			let hits = hits.clone();
			let bytes = bytes.clone();
			async move {
				hits.fetch_add(1, Ordering::SeqCst);
				([(axum::http::header::CONTENT_TYPE, mime)], bytes)
			}
		}
	};
	let app = Router::new()
		.route(
			"/first.png",
			get(handler(first_hits.clone(), first.clone(), "image/png")),
		)
		.route(
			"/second.zip",
			get(handler(second_hits.clone(), zip, "application/zip")),
		);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind local upstream");
	let port = listener.local_addr().expect("upstream addr").port();
	tokio::spawn(async move {
		axum::serve(listener, app)
			.await
			.expect("serve local upstream")
	});
	Upstream {
		port,
		first_hits,
		second_hits,
		first,
		second,
	}
}

struct GatedUpstream {
	port: u16,
	first_hits: Arc<AtomicUsize>,
	second_hits: Arc<AtomicUsize>,
	second_started: Arc<tokio::sync::Notify>,
	release_second: Arc<tokio::sync::Semaphore>,
	first: Vec<u8>,
	second: Vec<u8>,
}

async fn gated_upstream() -> GatedUpstream {
	let first = payload(3);
	let second = payload(4);
	let zip = zip_payload("page/1.png", &second);
	let first_hits = Arc::new(AtomicUsize::new(0));
	let second_hits = Arc::new(AtomicUsize::new(0));
	let second_started = Arc::new(tokio::sync::Notify::new());
	let release_second = Arc::new(tokio::sync::Semaphore::new(0));
	let first_response = first.clone();
	let app = Router::new()
		.route(
			"/first.png",
			get({
				let hits = first_hits.clone();
				move || {
					let hits = hits.clone();
					let bytes = first_response.clone();
					async move {
						hits.fetch_add(1, Ordering::SeqCst);
						([(axum::http::header::CONTENT_TYPE, "image/png")], bytes)
					}
				}
			}),
		)
		.route(
			"/second.zip",
			get({
				let hits = second_hits.clone();
				let started = second_started.clone();
				let release = release_second.clone();
				move || {
					let hits = hits.clone();
					let started = started.clone();
					let release = release.clone();
					let bytes = zip.clone();
					async move {
						hits.fetch_add(1, Ordering::SeqCst);
						started.notify_one();
						release.acquire().await.expect("release gate").forget();
						(
							[(axum::http::header::CONTENT_TYPE, "application/zip")],
							bytes,
						)
					}
				}
			}),
		);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind gated upstream");
	let port = listener.local_addr().expect("gated upstream addr").port();
	tokio::spawn(async move {
		axum::serve(listener, app)
			.await
			.expect("serve gated upstream")
	});
	GatedUpstream {
		port,
		first_hits,
		second_hits,
		second_started,
		release_second,
		first,
		second,
	}
}

async fn rewrite_urls(db_path: &Path, chapter_id: i64, port: u16) {
	let pool = connect_db(db_path).await;
	for (index, content) in [
		(
			0,
			json!({"Url": [format!("http://127.0.0.1:{port}/first.png"), null]}),
		),
		(
			1,
			json!({"Zip": [format!("http://127.0.0.1:{port}/second.zip"), "page/1.png"]}),
		),
	] {
		sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, ?, ?) ON CONFLICT(chapter_id, idx) DO UPDATE SET content = excluded.content")
			.bind(chapter_id)
			.bind(index)
			.bind(json!({"content": content, "thumbnail": null, "hasDescription": false, "description": null}).to_string())
			.execute(&pool)
			.await
			.expect("rewrite URL page");
	}
}

async fn remote_chapter() -> (TestApp, i64, Upstream) {
	let upstream = upstream().await;
	let (t, chapter_id) = remote_chapter_at(upstream.port).await;
	(t, chapter_id, upstream)
}

async fn remote_chapter_at(port: u16) -> (TestApp, i64) {
	let t = test_app().await;
	let manga_id = register_example_manga(&t.app).await;
	let (status, body) = request(
		&t.app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/chapters"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	rewrite_urls(&t.db_path, chapter_id, port).await;
	// Reconcile the deliberately rewritten page JSON so exact cache accounting assertions
	// start from the same persisted state a process restart sees.
	let tmp = t.tmp;
	let db_path = t.db_path;
	drop(t.app);
	let app = build_app(base_config(&tmp))
		.await
		.expect("restart fixture app");
	(TestApp { app, tmp, db_path }, chapter_id)
}

fn tree_bytes(path: &Path) -> u64 {
	if !path.exists() {
		return 0;
	}
	let mut total = 0;
	for entry in std::fs::read_dir(path).expect("read cache directory") {
		let entry = entry.expect("cache entry");
		let kind = entry.file_type().expect("cache file type");
		if kind.is_dir() {
			total += tree_bytes(&entry.path());
		} else if kind.is_file() {
			total += entry.metadata().expect("cache metadata").len();
		}
	}
	total
}

async fn page_json_bytes(db_path: &Path, chapter_id: i64) -> i64 {
	let pool = connect_db(db_path).await;
	sqlx::query_scalar(
		"SELECT coalesce(sum(length(CAST(content AS BLOB))), 0) FROM page WHERE chapter_id = ?",
	)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("page JSON bytes")
}

async fn wait_download(app: &Router, chapter_id: i64) -> Value {
	let deadline = Instant::now() + Duration::from_secs(12);
	loop {
		let (status, body) = request(app, "GET", "/api/v1/downloads", None).await;
		assert_eq!(status, StatusCode::OK);
		if let Some(entry) = body
			.as_array()
			.and_then(|rows| rows.iter().find(|row| row["chapterId"] == chapter_id))
		{
			if entry["status"] == "finished" {
				return entry.clone();
			}
			assert_ne!(entry["status"], "error", "download failed: {entry}");
		}
		assert!(Instant::now() < deadline, "download did not finish: {body}");
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}

async fn wait_revision_after(app: &Router, chapter_id: i64, revision: i64) -> Vec<Value> {
	let deadline = Instant::now() + Duration::from_secs(8);
	loop {
		let (status, pages) = request(
			app,
			"GET",
			&format!("/api/v1/chapter/{chapter_id}/pages"),
			None,
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		if pages[0]["cacheRevision"].as_i64().unwrap_or_default() > revision {
			return pages.as_array().expect("pages").clone();
		}
		assert!(
			Instant::now() < deadline,
			"cache cleanup did not advance revision: {pages}"
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}

async fn wait_downloaded_pages(app: &Router, chapter_id: i64, want: i64) {
	let deadline = Instant::now() + Duration::from_secs(8);
	loop {
		let (status, rows) = request(app, "GET", "/api/v1/downloads", None).await;
		assert_eq!(status, StatusCode::OK);
		if let Some(entry) = rows
			.as_array()
			.and_then(|items| items.iter().find(|item| item["chapterId"] == chapter_id))
		{
			if entry["status"] == "error" {
				panic!("download failed before progress check: {entry}");
			}
			if entry["downloaded"].as_i64().unwrap_or_default() >= want {
				return;
			}
		}
		assert!(
			Instant::now() < deadline,
			"download did not reach {want} completed pages: {rows}"
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}

#[tokio::test]
async fn remote_original_survives_restart_and_clear_advances_revision() {
	let (t, chapter_id, upstream) = remote_chapter().await;
	let page = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, headers, bytes) = request_full(&t.app, "GET", &page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, upstream.first);
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 1);
	let zip_page = format!("/api/v1/chapter/{chapter_id}/page/1");
	let (status, headers, bytes) = request_full(&t.app, "GET", &zip_page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(
		bytes, upstream.second,
		"Zip entry bytes are cached after extraction"
	);
	assert_eq!(upstream.second_hits.load(Ordering::SeqCst), 1);

	let (status, pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let before_revision = pages[0]["cacheRevision"].as_i64().expect("cache revision");
	let dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	let (status, stats) = request(&t.app, "GET", "/api/v1/cache/chapters", None).await;
	assert_eq!(status, StatusCode::OK);
	let expected = page_json_bytes(&t.db_path, chapter_id).await as u64 + tree_bytes(&dir);
	assert_eq!(
		stats["usedBytes"].as_u64(),
		Some(expected),
		"stats must equal page JSON plus actual cache files"
	);

	// A fresh app over the same temp database proves reuse comes from disk, not an in-memory response cache.
	let tmp = t.tmp;
	let db_path = t.db_path;
	drop(t.app);
	let app = build_app(base_config(&tmp)).await.expect("restart app");
	let (status, headers, bytes) = request_full(&app, "GET", &page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, upstream.first);
	let (status, _, zipped) = request_full(&app, "GET", &zip_page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(zipped, upstream.second);
	assert_eq!(
		upstream.first_hits.load(Ordering::SeqCst),
		1,
		"restart must reuse URL original bytes"
	);
	assert_eq!(
		upstream.second_hits.load(Ordering::SeqCst),
		1,
		"restart must reuse Zip originals"
	);

	let (status, cleared) = request(&app, "POST", "/api/v1/cache/chapters/clear", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		cleared["usedBytes"],
		page_json_bytes(&db_path, chapter_id).await
	);
	assert!(cleared["cacheRevision"].as_i64().expect("revision") > before_revision);
	rewrite_urls(&db_path, chapter_id, upstream.port).await;
	let (status, _, bytes) = request_full(&app, "GET", &page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, upstream.first);
	assert_eq!(
		upstream.first_hits.load(Ordering::SeqCst),
		2,
		"clear must force one refetch"
	);
	let (status, _, bytes) = request_full(&app, "GET", &zip_page, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, upstream.second);
	assert_eq!(
		upstream.second_hits.load(Ordering::SeqCst),
		2,
		"clear also invalidates Zip originals"
	);
}

#[tokio::test]
async fn download_reuses_cached_original_fetches_only_misses_and_cleans_derived_cache() {
	let (t, chapter_id, upstream) = remote_chapter().await;
	let (status, pages_before) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let revision_before_download = pages_before[0]["cacheRevision"]
		.as_i64()
		.expect("initial revision");
	let original_url = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, _, bytes) = request_full(&t.app, "GET", &original_url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, upstream.first);
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 1);
	let (status, _) = request(
		&t.app,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let product = wait_download(&t.app, chapter_id).await;
	let product_pages = wait_revision_after(&t.app, chapter_id, revision_before_download).await;
	let revision_after_download = product_pages[0]["cacheRevision"]
		.as_i64()
		.expect("post-download revision");
	assert!(
		revision_after_download > revision_before_download,
		"finished product cleanup advances cache revision"
	);
	assert_eq!(
		upstream.first_hits.load(Ordering::SeqCst),
		1,
		"download should consume cached original"
	);
	assert_eq!(
		upstream.second_hits.load(Ordering::SeqCst),
		1,
		"only uncached page should be fetched"
	);

	let pool = connect_db(&t.db_path).await;
	let (rows, checked, width, height): (i64, i64, Option<i64>, Option<i64>) = sqlx::query_as(
		"SELECT count(*), sum(dimensions_checked), max(width), max(height) FROM page WHERE chapter_id = ?",
	).bind(chapter_id).fetch_one(&pool).await.expect("retained pages");
	assert_eq!(rows, 2, "source page rows survive product completion");
	assert_eq!(
		checked, 2,
		"download measurements survive product completion"
	);
	assert!(
		width.is_some() && height.is_some(),
		"measured dimensions survive"
	);
	let page_json = page_json_bytes(&t.db_path, chapter_id).await;
	let (status, stats) = request(&t.app, "GET", "/api/v1/cache/chapters", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		stats["usedBytes"], page_json,
		"originals and variants are removed; page rows remain charged"
	);
	let blob_dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	assert_eq!(
		tree_bytes(&blob_dir),
		0,
		"finished product cleanup removes all page cache files"
	);

	let (status, _, bytes) = request_full(&t.app, "GET", &original_url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		bytes, upstream.first,
		"reader prefers the completed product"
	);
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 1);
	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/clear", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, list) = request(&t.app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		list.as_array()
			.expect("downloads")
			.iter()
			.all(|row| row["chapterId"] != chapter_id),
		"dismissed product is hidden"
	);
	let (status, pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages?refresh=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(pages.as_array().expect("product pages").len(), 2);
	assert_eq!(
		upstream.first_hits.load(Ordering::SeqCst),
		1,
		"force refresh still prefers a finished hidden product"
	);

	let _path = product["path"].as_str().expect("artifact path");
	let (status, _, product_page) = request_full(&t.app, "GET", &original_url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(!product_page.is_empty());
	let (status, _) = request(
		&t.app,
		"DELETE",
		&format!("/api/v1/downloads/{chapter_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, fallback_pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		fallback_pages[0]["cacheRevision"]
			.as_i64()
			.expect("delete revision")
			> revision_after_download,
		"product deletion advances cache revision"
	);
	assert_eq!(
		tree_bytes(&blob_dir),
		0,
		"product deletion leaves no page cache files behind"
	);
	let (status, _, bytes) = request_full(&t.app, "GET", &original_url, None).await;
	assert_eq!(
		status,
		StatusCode::OK,
		"deleted product falls back to remote page source"
	);
	assert_eq!(bytes, upstream.first);
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn hidden_folder_product_stays_offline_and_wins_force_refresh() {
	let (t, chapter_id, upstream) = remote_chapter().await;
	let folder = t.tmp.path().join("downloads").join("offline-folder");
	std::fs::create_dir_all(&folder).expect("product folder");
	std::fs::write(folder.join("000.png"), &upstream.first).expect("folder page 0");
	std::fs::write(folder.join("001.png"), &upstream.second).expect("folder page 1");
	let pool = connect_db(&t.db_path).await;
	sqlx::query("INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', 'folder', 'offline-folder')")
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("finished folder product");

	let (status, body) = request(&t.app, "POST", "/api/v1/downloads/clear", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cleared"], 1);
	let (status, list) = request(&t.app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		list.as_array()
			.expect("download list")
			.iter()
			.all(|row| row["chapterId"] != chapter_id)
	);

	let (status, pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages?refresh=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(pages.as_array().expect("folder pages").len(), 2);
	let (status, headers, bytes) = request_full(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, upstream.first);
	assert_eq!(
		upstream.first_hits.load(Ordering::SeqCst),
		0,
		"hidden folder remains the offline reader source"
	);
	assert_eq!(
		upstream.second_hits.load(Ordering::SeqCst),
		0,
		"force refresh must not fetch source pages"
	);
}

#[tokio::test]
async fn cold_download_does_not_fill_persistent_original_cache() {
	let upstream = gated_upstream().await;
	let (t, chapter_id) = remote_chapter_at(upstream.port).await;
	let blob_dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	assert_eq!(
		tree_bytes(&blob_dir),
		0,
		"cold chapter starts with no image cache files"
	);
	let (status, pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let revision_before = pages[0]["cacheRevision"]
		.as_i64()
		.expect("initial revision");
	let started = upstream.second_started.clone();
	let waiting = started.notified();
	let (status, _) = request(
		&t.app,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	tokio::time::timeout(Duration::from_secs(8), waiting)
		.await
		.expect("download reached its gated second page");
	wait_downloaded_pages(&t.app, chapter_id, 1).await;
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 1);
	assert_eq!(upstream.second_hits.load(Ordering::SeqCst), 1);
	assert_eq!(
		tree_bytes(&blob_dir),
		0,
		"download-only page misses must not persist originals while packaging is active"
	);
	upstream.release_second.add_permits(1);
	let product = wait_download(&t.app, chapter_id).await;
	let pages = wait_revision_after(&t.app, chapter_id, revision_before).await;
	assert_eq!(pages.len(), 2);
	assert_eq!(
		tree_bytes(&blob_dir),
		0,
		"download completion leaves no page cache files"
	);
	for (index, expected) in [(0, &upstream.first), (1, &upstream.second)] {
		let (status, _, bytes) = request_full(
			&t.app,
			"GET",
			&format!("/api/v1/chapter/{chapter_id}/page/{index}"),
			None,
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(&bytes, expected, "cold downloaded page {index}");
	}
	assert_eq!(product["status"], "finished");
}

#[tokio::test]
async fn url_and_zip_reads_refetch_missing_original_bytes_and_backfill_dimensions() {
	let (t, chapter_id, upstream) = remote_chapter().await;
	let pool = connect_db(&t.db_path).await;
	for (index, hits, original) in [
		(0, &upstream.first_hits, &upstream.first),
		(1, &upstream.second_hits, &upstream.second),
	] {
		let url = format!("/api/v1/chapter/{chapter_id}/page/{index}");
		let (status, _, first) = request_full(&t.app, "GET", &url, None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(&first, original);
		assert_eq!(hits.load(Ordering::SeqCst), 1);
		let bytes_path = t.tmp.path().join(format!("page-blobs/{chapter_id}/originals/{index}/bytes"));
		std::fs::remove_file(&bytes_path).expect("remove original bytes but keep its published version metadata");
		sqlx::query("UPDATE page SET width=NULL,height=NULL,dimensions_checked=0 WHERE chapter_id=? AND idx=?")
			.bind(chapter_id).bind(index).execute(&pool).await.expect("retry dimension backfill");
		let (status, _, second) = request_full(&t.app, "GET", &url, None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(&second, original, "a missing original re-fetches from the source");
		assert_eq!(hits.load(Ordering::SeqCst), 2);
		let dimensions: (Option<i64>, Option<i64>, i64) = sqlx::query_as("SELECT width,height,dimensions_checked FROM page WHERE chapter_id=? AND idx=?")
			.bind(chapter_id).bind(index).fetch_one(&pool).await.expect("original dimensions");
		assert_eq!(dimensions, (Some(640), Some(640), 1), "the re-fetched original backfills page dimensions");
	}
}

#[tokio::test]
async fn repaired_original_at_same_url_serves_the_new_bytes() {
	let first = payload(5);
	let second = payload(6);
	let current = Arc::new(std::sync::Mutex::new(first));
	let hits = Arc::new(AtomicUsize::new(0));
	let app = Router::new().route("/mutable.png", get({
		let current = current.clone();
		let hits = hits.clone();
		move || {
			let current = current.clone();
			let hits = hits.clone();
			async move {
				hits.fetch_add(1, Ordering::SeqCst);
				([(axum::http::header::CONTENT_TYPE, "image/png")], current.lock().expect("image lock").clone())
			}
		}
	}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("local mutable upstream");
	let port = listener.local_addr().expect("local addr").port();
	let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("mutable upstream"); });
	let (t, chapter_id) = remote_chapter_at(port).await;
	let pool = connect_db(&t.db_path).await;
	let content = json!({"content": {"Url": [format!("http://127.0.0.1:{port}/mutable.png"), null]}});
	sqlx::query("UPDATE page SET content=? WHERE chapter_id=? AND idx=0")
		.bind(content.to_string()).bind(chapter_id).execute(&pool).await.expect("mutable page");
	let url = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, _, old) = request_full(&t.app, "GET", &url, None).await;
	assert_eq!(status, StatusCode::OK);
	*current.lock().expect("replace image") = second.clone();
	std::fs::write(t.tmp.path().join(format!("page-blobs/{chapter_id}/originals/0/bytes")), b"corrupted").expect("damage original");
	let (status, _, repaired) = request_full(&t.app, "GET", &url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(repaired, second, "a full original read must repair corruption");
	let (status, _, reread) = request_full(&t.app, "GET", &url, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(reread, second, "the repaired original keeps serving the new bytes");
	assert_ne!(reread, old);
	assert_eq!(hits.load(Ordering::SeqCst), 2);
	server.abort();
}

#[tokio::test]
async fn zero_budget_bypasses_originals_then_positive_budget_restores_reuse() {
	let (t, chapter_id, upstream) = remote_chapter().await;
	let (status, settings) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"cache.chapterMaxBytes": 0}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(settings["cache"], json!({"chapterMaxBytes": 0}));
	let dir = t.tmp.path().join("page-blobs").join(chapter_id.to_string());
	for _ in 0..2 {
		for index in 0..2 {
			let (status, _, _) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
			assert_eq!(status, StatusCode::OK);
		}
	}
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 2);
	assert_eq!(upstream.second_hits.load(Ordering::SeqCst), 2);
	assert!(!dir.join("originals").exists(), "zero budget must not persist remote originals");
	let (status, stats) = request(&t.app, "GET", "/api/v1/cache/chapters", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(stats["enabled"], false);
	assert_eq!(stats["maxBytes"], 0);
	let (status, _) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"cache.chapterMaxBytes": 8 * 1024 * 1024}))).await;
	assert_eq!(status, StatusCode::OK);
	let url = format!("/api/v1/chapter/{chapter_id}/page/0");
	for _ in 0..2 { assert_eq!(request_full(&t.app, "GET", &url, None).await.0, StatusCode::OK); }
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 3, "a positive budget restores caching");
	assert!(dir.join("originals/0/bytes").exists());
	let (status, _) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"cache.chapterMaxBytes": 0}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(request_full(&t.app, "GET", &url, None).await.0, StatusCode::OK);
	assert_eq!(upstream.first_hits.load(Ordering::SeqCst), 4, "zero budget bypasses already-persisted originals too");
}

#[tokio::test]
async fn cache_budget_boundaries_and_removed_switch_are_consistent_for_both_settings_shapes() {
	let t = test_app().await;
	let pool = connect_db(&t.db_path).await;
	sqlx::query("INSERT INTO setting(key,value) VALUES ('cache.chapterLruEnabled','false')")
		.execute(&pool).await.expect("legacy stored switch");
	for value in [0, 1, 256 * 1024 * 1024 - 1, i64::MAX] {
		let (status, body) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"cache.chapterMaxBytes": value, "cache.chapterLruEnabled": false}))).await;
		assert_eq!(status, StatusCode::OK, "flat budget {value}: {body}");
		assert_eq!(body["cache"], json!({"chapterMaxBytes": value}));
		assert!(body.get("cache.chapterLruEnabled").is_none());
		let mut structured = body;
		structured["cache"] = json!({"chapterMaxBytes": value, "chapterLruEnabled": false});
		let (status, body) = request(&t.app, "PUT", "/api/v1/settings", Some(structured)).await;
		assert_eq!(status, StatusCode::OK, "structured budget {value}: {body}");
		assert_eq!(body["cache"], json!({"chapterMaxBytes": value}));
		let (_, stats) = request(&t.app, "GET", "/api/v1/cache/chapters", None).await;
		assert_eq!(stats["enabled"], value > 0, "legacy false cannot disable cache or LRU");
	}
	for value in [json!(-1), json!(1.5), json!(i64::MAX as u64 + 1), json!("0"), Value::Null] {
		let (status, body) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"cache.chapterMaxBytes": value}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "invalid flat budget {value}: {body}");
		let (_, mut structured) = request(&t.app, "GET", "/api/v1/settings", None).await;
		structured["cache"]["chapterMaxBytes"] = value.clone();
		assert_eq!(request(&t.app, "PUT", "/api/v1/settings", Some(structured)).await.0, StatusCode::BAD_REQUEST, "invalid structured budget {value}");
	}
}
