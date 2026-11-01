mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{
	TestApp, base_config, connect_db, fixture_dir, install_fixture_source, register_example_manga,
	request, request_full, source_by_key, test_app,
};
use serde_json::{Value, json};
use sumidoku_server::build_app;

// 1x1 transparent PNG
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[tokio::test]
async fn library_category_progress_history() {
	let TestApp { app, db_path, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;

	// shelve
	let (status, body) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["inLibrary"], true);

	// library list
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("library entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["id"], manga_id);
	assert_eq!(entries[0]["inLibrary"], true);
	assert!(entries[0]["inLibraryAt"].is_i64());
	assert_eq!(body["hasNextPage"], false);

	// not shelved means uncategorized
	let (status, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);

	// categories
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_a = body["id"].as_i64().expect("category id");
	assert_eq!(body["sortOrder"], 0);
	assert_eq!(body["mangaCount"], 0);
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "duplicate");
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Drama"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_b = body["id"].as_i64().expect("category id");
	assert_eq!(body["sortOrder"], 1);

	// assign to the category
	let manga_categories = format!("/api/v1/manga/{manga_id}/categories");
	let (status, _) = request(&app, "PUT", &manga_categories, Some(json!({"categoryIds": [cat_a]}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &format!("/api/v1/library?categoryId={cat_a}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);
	let (_, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);

	// the category list carries mangaCount
	let (status, body) = request(&app, "GET", "/api/v1/categories", None).await;
	assert_eq!(status, StatusCode::OK);
	let cats = body.as_array().expect("categories");
	assert_eq!(cats.len(), 2);
	assert_eq!(cats[0]["name"], "Action");
	assert_eq!(cats[0]["mangaCount"], 1);
	assert_eq!(cats[1]["mangaCount"], 0);

	// reorder
	let (status, body) = request(
		&app,
		"PATCH",
		"/api/v1/categories/reorder",
		Some(json!({"ids": [cat_b, cat_a]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let cats = body.as_array().expect("reordered categories");
	assert_eq!(cats[0]["id"], cat_b);
	assert_eq!(cats[0]["sortOrder"], 0);
	assert_eq!(cats[1]["id"], cat_a);
	assert_eq!(cats[1]["sortOrder"], 1);

	// rename
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/categories/{cat_b}"),
		Some(json!({"name": "Romance"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["name"], "Romance");

	// chapters carry default user fields
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0]["lastPageRead"], 0);
	assert_eq!(chapters[0]["isRead"], false);
	assert_eq!(chapters[0]["isBookmarked"], false);
	assert!(chapters[0]["lastReadAt"].is_null());
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");

	// progress
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 3})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["chapterId"], chapter_id);
	assert_eq!(body["lastPageRead"], 3);
	assert_eq!(body["isRead"], false);
	assert_eq!(body["isBookmarked"], false);
	assert!(body["lastReadAt"].is_i64());

	// chapters read back user fields: read ones have values, unread ones defaults
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let chapters = body.as_array().expect("chapters");
	assert_eq!(chapters[0]["lastPageRead"], 3);
	assert!(chapters[0]["lastReadAt"].is_i64());
	assert_eq!(chapters[1]["lastPageRead"], 0);
	assert!(chapters[1]["lastReadAt"].is_null());

	// history
	let (status, body) = request(&app, "GET", "/api/v1/history", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("history entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["chapter"]["id"], chapter_id);
	assert_eq!(entries[0]["chapter"]["lastPageRead"], 3);
	assert_eq!(entries[0]["manga"]["id"], manga_id);
	assert_eq!(entries[0]["lastPageRead"], 3);
	assert_eq!(entries[0]["isRead"], false);
	assert!(entries[0]["lastReadAt"].is_i64());
	assert_eq!(body["hasNextPage"], false);

	// batch mark read
	let ids: Vec<i64> = chapters.iter().map(|c| c["id"].as_i64().expect("id")).collect();
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": ids, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 8);
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert!(
		body.as_array()
			.expect("chapters")
			.iter()
			.all(|c| c["isRead"] == true)
	);

	// deleting a category: category_manga cascades away, the manga falls back to uncategorized
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/categories/{cat_a}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);

	// unshelve
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);
	let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(body["inLibrary"], false);
	let (status, body) = request(&app, "PUT", &manga_categories, Some(json!({"categoryIds": []}))).await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "not_in_library");

	// the category_manga rows are wiped too
	let pool = connect_db(&db_path).await;
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM category_manga WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("count category_manga");
	assert_eq!(count, 0);
}

/// Marking unread (isRead: false) is Aidoku's mark-unread: the whole reading trace goes -
/// progress, resume stamp, history entry - while the bookmark survives; the batch endpoint
/// runs the same wipe, and marking read still never touches the stamp by itself.
#[tokio::test]
async fn mark_unread_wipes_the_reading_trace() {
	let TestApp { app, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");

	// finish the chapter at page 3 and bookmark it
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 3, "isRead": true, "isBookmarked": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/history", None).await;
	assert_eq!(body["entries"].as_array().expect("history entries").len(), 1);

	// single mark-unread: progress, stamp and history entry wiped, bookmark kept, no new stamp
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"isRead": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["lastPageRead"], 0);
	assert_eq!(body["isRead"], false);
	assert_eq!(body["isBookmarked"], true);
	assert!(body["lastReadAt"].is_null());
	let (_, body) = request(&app, "GET", "/api/v1/history", None).await;
	assert_eq!(body["entries"].as_array().expect("history entries").len(), 0);

	// the batch endpoint applies the same wipe; the chapters read-back shows the wiped defaults
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": [chapter_id], "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 1);
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": [chapter_id], "isRead": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 1);
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let chapter = &body[0];
	assert_eq!(chapter["lastPageRead"], 0);
	assert_eq!(chapter["isRead"], false);
	assert_eq!(chapter["isBookmarked"], true);
	assert!(chapter["lastReadAt"].is_null());
}

/// Starts a minimal static server returning bytes; returns the port and handle (aborting shuts it down, simulating the upstream disappearing)
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

async fn update_page_content(pool: &sqlx::SqlitePool, chapter_id: i64, idx: i64, content: serde_json::Value) {
	let stored = json!({
		"content": content,
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	});
	sqlx::query("UPDATE page SET content = ? WHERE chapter_id = ? AND idx = ?")
		.bind(stored.to_string())
		.bind(chapter_id)
		.bind(idx)
		.execute(pool)
		.await
		.expect("update page content");
}

#[tokio::test]
async fn cover_and_page_proxy() {
	let TestApp { app, tmp, db_path } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let pool = connect_db(&db_path).await;

	// cover: upstream PNG → 200 into the cache
	let (port, upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?")
		.bind(format!("http://127.0.0.1:{port}/img.png"))
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update cover url");
	let cover_path = format!("/api/v1/manga/{manga_id}/cover");
	let (status, headers, bytes) = request_full(&app, "GET", &cover_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=86400");
	// the example fixture exports process_cover_image, so the cover is always processed into a 200x300 PNG
	let cover = image::ImageReader::new(std::io::Cursor::new(&bytes))
		.with_guessed_format()
		.expect("cover format")
		.decode()
		.expect("cover decode");
	assert_eq!((cover.width(), cover.height()), (200, 300));

	// still 200 after the upstream disappears: served from cache
	upstream.abort();
	let (status, _, cached) = request_full(&app, "GET", &cover_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(cached, bytes);

	// ?refresh bypasses the cache → upstream is dead, 502
	let (status, body) = request(&app, "GET", &format!("{cover_path}?refresh"), None).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);
	assert_eq!(body["error"]["code"], "bad_gateway");

	// page image proxy: cache the pages first
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _source_pages) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let source_content: String = sqlx::query_scalar("SELECT content FROM page WHERE chapter_id = ? AND idx = 0")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("source page content");
	let page_path = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, _, source_bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK, "fixture page should be readable");

	// Url page
	let (port, _pages_upstream) = serve_bytes(vec![
		("/img.png", "image/png", PNG.to_vec()),
		("/img.zip", "application/zip", make_zip("page/1.jpg", PNG)),
	])
	.await;
	update_page_content(
		&pool,
		chapter_id,
		0,
		json!({"Url": [format!("http://127.0.0.1:{port}/img.png"), null]}),
	)
	.await;
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=3600");
	assert_eq!(bytes, PNG);

	// Text page
	update_page_content(&pool, chapter_id, 0, json!({"Text": "# Hi"})).await;
	let (status, body) = request(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!({"type": "text", "text": "# Hi"}));

	// Zip page: the in-memory zip yields the entry
	update_page_content(
		&pool,
		chapter_id,
		0,
		json!({"Zip": [format!("http://127.0.0.1:{port}/img.zip"), "page/1.jpg"]}),
	)
	.await;
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/jpeg");
	assert_eq!(bytes, PNG);

	// Image page: a missing materialized file triggers source page rematerialization.
	update_page_content(&pool, chapter_id, 0, json!({"Image": "0.png"})).await;
	let blob_dir = tmp.path().join("page-blobs").join(chapter_id.to_string());
	std::fs::create_dir_all(&blob_dir).expect("mkdir blob dir");
	std::fs::write(blob_dir.join("0.png"), PNG).expect("write blob");
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, PNG);

	update_page_content(&pool, chapter_id, 0, json!({"Image": "9.png"})).await;
	let (status, _, rebuilt_bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK, "missing Image blobs are rebuilt from the source");
	assert_eq!(rebuilt_bytes, source_bytes, "the refreshed page serves actual source bytes");
	let rebuilt_content: String = sqlx::query_scalar("SELECT content FROM page WHERE chapter_id = ? AND idx = 0")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("rebuilt page content");
	assert_eq!(rebuilt_content, source_content, "source page metadata is restored");

	// out of range 404
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/99"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

fn make_zip(entry_path: &str, data: &[u8]) -> Vec<u8> {
	use std::io::Write;
	let mut cursor = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut cursor);
		writer
			.start_file(entry_path, zip::write::SimpleFileOptions::default())
			.expect("start file");
		writer.write_all(data).expect("write entry");
		writer.finish().expect("finish zip");
	}
	cursor.into_inner()
}

#[tokio::test]
async fn static_webui() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let webui = tmp.path().join("webui");
	std::fs::create_dir_all(webui.join("assets")).expect("mkdir assets");
	std::fs::write(webui.join("index.html"), "<html><body>sumidoku-webui</body></html>").expect("index");
	std::fs::write(webui.join("assets/app.js"), "console.log(1)").expect("asset");
	std::fs::write(webui.join("sw.js"), "// service worker").expect("service worker");
	std::fs::write(webui.join("manifest.webmanifest"), "{}").expect("manifest");
	let mut config = base_config(&tmp);
	config.webui_dir = webui;
	let app = build_app(config).await.expect("build app");

	let (status, _, bytes) = request_full(&app, "GET", "/", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(String::from_utf8(bytes).expect("utf8").contains("sumidoku-webui"));

	let (status, _, bytes) = request_full(&app, "GET", "/any/spa/route", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(String::from_utf8(bytes).expect("utf8").contains("sumidoku-webui"));

	let (status, headers, bytes) = request_full(&app, "GET", "/assets/app.js", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(!headers.contains_key("cache-control"));
	assert_eq!(bytes, b"console.log(1)");

	for (path, expected) in [
		("/sw.js", b"// service worker".as_slice()),
		("/manifest.webmanifest", b"{}".as_slice()),
	] {
		let (status, headers, bytes) = request_full(&app, "GET", path, None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "no-cache");
		assert_eq!(bytes.as_slice(), expected);
	}

	// the API is not swallowed by the SPA; the built-in local source is persisted by the migration, so even an empty DB has one row
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("sources array").len(), 1);
	assert_eq!(body[0]["key"], "local.local");
	assert_eq!(body[0]["kind"], "local");
	let (status, body) = request(&app, "GET", "/api/v1/nope", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn deleting_category_clears_exclusions_before_id_reuse() {
	let TestApp { app, db_path, .. } = test_app().await;
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Old"}))).await;
	assert_eq!(status, StatusCode::OK);
	let category_id = body["id"].as_i64().expect("category id");
	let pool = connect_db(&db_path).await;
	for key in [
		"library.excluded_update_categories",
		"library.excluded_download_categories",
	] {
		sqlx::query("INSERT INTO setting (key, value) VALUES (?, ?)")
			.bind(key)
			.bind(format!("[{category_id}, 999]"))
			.execute(&pool)
			.await
			.expect("seed category exclusions");
	}
	pool.close().await;

	let (status, _) = request(&app, "DELETE", &format!("/api/v1/categories/{category_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Replacement"}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], category_id);

	let pool = connect_db(&db_path).await;
	for key in [
		"library.excluded_update_categories",
		"library.excluded_download_categories",
	] {
		let raw: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
			.bind(key)
			.fetch_one(&pool)
			.await
			.expect("category exclusions");
		let ids: Vec<i64> = serde_json::from_str(&raw).expect("exclusion ids");
		assert_eq!(ids, vec![999], "{key}");
	}
}

#[tokio::test]
async fn category_include_in_download_patch_is_rejected() {
	let TestApp { app, db_path, .. } = test_app().await;
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Flags"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat = body["id"].as_i64().expect("category id");
	assert!(!body.as_object().unwrap().contains_key("includeInUpdate"));
	assert!(!body.as_object().unwrap().contains_key("includeInDownload"));
	let cat_path = format!("/api/v1/categories/{cat}");
	let pool = connect_db(&db_path).await;
	sqlx::query("UPDATE category SET include_in_download = 1 WHERE id = ?")
		.bind(cat)
		.execute(&pool)
		.await
		.expect("seed legacy download flag");
	pool.close().await;

	for patch in [
		json!({"includeInDownload": 0}),
		json!({"includeInDownload": 1}),
		json!({"includeInDownload": 2}),
		json!({"includeInDownload": null}),
		json!({"includeInDownload": "x"}),
		json!({"name": "Changed", "includeInDownload": 2}),
	] {
		let (status, body) = request(&app, "PATCH", &cat_path, Some(patch.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{patch}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
		assert!(
			body["error"]["message"]
				.as_str()
				.expect("error message")
				.contains("library.excluded_download_categories"),
			"{body}"
		);
	}

	let (_, body) = request(&app, "GET", "/api/v1/categories", None).await;
	assert_eq!(body[0]["name"], "Flags");
	assert!(!body[0].as_object().unwrap().contains_key("includeInDownload"));
	let (status, body) = request(&app, "PATCH", &cat_path, Some(json!({"name": "Flags2"}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["name"], "Flags2");
	assert!(!body.as_object().unwrap().contains_key("includeInDownload"));
	for body in [Some(json!({})), None] {
		let (status, response) = request(&app, "PATCH", &cat_path, body).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(response["name"], "Flags2");
	}
	let pool = connect_db(&db_path).await;
	let legacy: i64 = sqlx::query_scalar("SELECT include_in_download FROM category WHERE id = ?")
		.bind(cat)
		.fetch_one(&pool)
		.await
		.expect("legacy flag is untouched");
	assert_eq!(legacy, 1);
	pool.close().await;

	let (status, _) = request(&app, "PATCH", "/api/v1/categories/999", Some(json!({"includeInDownload": 1}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn category_include_in_update_patch_is_rejected() {
	let TestApp { app, .. } = test_app().await;
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Flags"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat = body["id"].as_i64().expect("category id");
	let cat_path = format!("/api/v1/categories/{cat}");

	for patch in [
		json!({"includeInUpdate": 0}),
		json!({"includeInUpdate": 1}),
		json!({"includeInUpdate": 2}),
		json!({"includeInUpdate": null}),
		json!({"name": "Changed", "includeInUpdate": 2}),
	] {
		let (status, body) = request(&app, "PATCH", &cat_path, Some(patch.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{patch}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
		assert!(
			body["error"]["message"]
				.as_str()
				.expect("error message")
				.contains("library.excluded_update_categories"),
			"{body}"
		);
	}

	let (_, body) = request(&app, "GET", "/api/v1/categories", None).await;
	assert_eq!(body[0]["name"], "Flags");
	assert!(!body[0].as_object().unwrap().contains_key("includeInDownload"));
}

/// Registers the first three search results (full entries, titles persisted at registration), returning (id, title)
async fn register_three_manga(app: &Router) -> Vec<(i64, String)> {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entries = body["entries"].as_array().expect("entries");
	let mut ids = Vec::new();
	for entry in entries.iter().take(3) {
		let title = entry["title"].as_str().expect("manga title").to_string();
		let (status, body) = request(
			app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": entry})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		ids.push((body["id"].as_i64().expect("manga id"), title));
	}
	ids
}

async fn first_chapters(app: &Router, manga_id: i64, count: usize) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters")
		.iter()
		.take(count)
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect()
}

async fn put_chapter_user(
	pool: &sqlx::SqlitePool,
	chapter_id: i64,
	is_read: bool,
	is_bookmarked: bool,
	is_downloaded: bool,
	last_read_at: Option<i64>,
) {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_read, is_bookmarked, is_downloaded, last_read_at)
		 VALUES (?, 1, ?, ?, ?, ?)",
	)
	.bind(chapter_id)
	.bind(is_read)
	.bind(is_bookmarked)
	.bind(is_downloaded)
	.bind(last_read_at)
	.execute(pool)
	.await
	.expect("insert chapter_user");
}

fn entry_ids(body: &Value) -> Vec<i64> {
	body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["id"].as_i64().expect("id"))
		.collect()
}

#[tokio::test]
async fn library_unread_filters_include_missing_chapter_user_rows() {
	let TestApp { app, db_path, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	// No chapter_user row exists yet: unread is the default, not an explicit flag.
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(entry_ids(&body), vec![manga_id]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filterExclude=unread", None).await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());

	let pool = connect_db(&db_path).await;
	for chapter_id in first_chapters(&app, manga_id, usize::MAX).await {
		put_chapter_user(&pool, chapter_id, true, false, false, None).await;
	}
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	let (_, body) = request(&app, "GET", "/api/v1/library?filterExclude=unread", None).await;
	assert_eq!(entry_ids(&body), vec![manga_id]);
}

#[tokio::test]
async fn library_tracked_filter_reads_the_real_schema() {
	let TestApp { app, db_path, .. } = test_app().await;
	let ids = register_three_manga(&app).await;
	let (a, b) = (ids[0].0, ids[1].0);
	for id in [a, b] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let pool = connect_db(&db_path).await;
	sqlx::query(
		"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, created_at, updated_at)
		 SELECT ?, id, 'anilist', '1', 0, 0 FROM user",
	)
	.bind(a)
	.execute(&pool)
	.await
	.expect("link");

	let (status, body) = request(&app, "GET", "/api/v1/library?filter=tracked", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![a]);
	let (status, body) = request(&app, "GET", "/api/v1/library?filterExclude=tracked", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![b]);
	// Later binds and the page window sit after the filter's: a miscount shifts them.
	let (status, body) = request(&app, "GET", "/api/v1/library?filter=tracked,unread&sort=unread", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![a]);
}

#[tokio::test]
async fn library_search_sort_filter() {
	let TestApp { app, db_path, .. } = test_app().await;
	let ids = register_three_manga(&app).await;
	let (a, b, c) = (ids[0].0, ids[1].0, ids[2].0);
	let (ta, tb, tc) = (&ids[0].1, &ids[1].1, &ids[2].1);
	// the search/sort assertions all build on the titles persisted at registration, so read them back to reconcile first
	for (id, title) in [(a, ta), (b, tb), (c, tc)] {
		let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{id}"), None).await;
		assert_eq!(&body["title"], title);
	}
	for id in [a, b, c] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let pool = connect_db(&db_path).await;
	sqlx::query("UPDATE manga SET status = CASE WHEN id = ? THEN 2 WHEN id = ? THEN 0 ELSE 1 END")
		.bind(a)
		.bind(c)
		.execute(&pool)
		.await
		.expect("set publication status");
	// anchor for added sorting: shelve times explicitly spread apart to avoid same-second ties
	for (id, at) in [(a, 1000), (b, 1001), (c, 1002)] {
		sqlx::query("UPDATE manga_user SET in_library_at = ? WHERE manga_id = ?")
			.bind(at)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set in_library_at");
	}
	let a_ch = first_chapters(&app, a, 2).await;
	let b_ch = first_chapters(&app, b, 1).await;
	// A: 2 unread (one bookmarked, read @100); B: 1 unread downloaded, read @300; C: all read, no read time
	put_chapter_user(&pool, a_ch[0], false, true, false, Some(100)).await;
	put_chapter_user(&pool, a_ch[1], false, false, false, None).await;
	put_chapter_user(&pool, b_ch[0], false, false, true, Some(300)).await;
	for chapter_id in first_chapters(&app, c, usize::MAX).await {
		put_chapter_user(&pool, chapter_id, true, false, false, None).await;
	}

	// default added ordering + unreadCount
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![c, b, a]);
	assert_eq!(body["hasNextPage"], false);
	let mut totals = std::collections::HashMap::new();
	for id in [a, b, c] {
		let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{id}/chapters"), None).await;
		totals.insert(id, body.as_array().expect("chapters").len() as i64);
	}
	let counts: std::collections::HashMap<i64, i64> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| {
			(
				e["id"].as_i64().expect("id"),
				e["unreadCount"].as_i64().expect("unreadCount"),
			)
		})
		.collect();
	// chapters with an is_read=1 row: C has all chapters read, A and B none — every other chapter counts as unread
	assert_eq!(
		counts,
		std::collections::HashMap::from([(a, totals[&a]), (b, totals[&b]), (c, 0)])
	);

	// search: B's full title hits only B
	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(tb)),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![b]);
	// LIKE wildcards are treated literally: replacing the title's 2nd character with _ or % would hit if they were treated as wildcards
	let mut wildcard: String = tb.chars().enumerate().map(|(i, ch)| if i == 1 { '_' } else { ch }).collect();
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(&wildcard)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	wildcard.insert(1, '%');
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(&wildcard)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	// an empty search counts as not passed
	let (_, body) = request(&app, "GET", "/api/v1/library?search=", None).await;
	assert_eq!(entry_ids(&body).len(), 3);

	// sorting
	let mut by_title = [(a, ta), (b, tb), (c, tc)];
	by_title.sort_by_key(|x| x.1.to_lowercase());
	let title_order: Vec<i64> = by_title.iter().map(|(id, _)| *id).collect();
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=title", None).await;
	assert_eq!(entry_ids(&body), title_order);
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=lastRead", None).await;
	assert_eq!(entry_ids(&body), vec![b, a, c]);
	// the badge's default-unread count desc, ties broken by id asc
	let mut by_unread = [(a, totals[&a]), (b, totals[&b]), (c, 0)];
	by_unread.sort_by_key(|&(id, count)| (-count, id));
	let unread_order: Vec<i64> = by_unread.iter().map(|&(id, _)| id).collect();
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=unread", None).await;
	assert_eq!(entry_ids(&body), unread_order);
	let (status, _) = request(&app, "GET", "/api/v1/library?sort=bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// filtering (multiple values are ANDed)
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(entry_ids(&body), vec![b, a]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=downloaded", None).await;
	assert_eq!(entry_ids(&body), vec![b]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=bookmarked", None).await;
	assert_eq!(entry_ids(&body), vec![a]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread,downloaded", None).await;
	assert_eq!(entry_ids(&body), vec![b]);
	let (status, body) = request(&app, "GET", "/api/v1/library?filter=completed", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![a]);
	let (status, body) = request(&app, "GET", "/api/v1/library?filterExclude=completed", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![c, b]);
	// Publication status has no user bind; the following filter and sort do.
	let (status, body) = request(&app, "GET", "/api/v1/library?filter=completed,unread&sort=unread", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![a]);
	let (status, _) = request(&app, "GET", "/api/v1/library?filter=unread,bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// exclusion semantics: excluding unread = everything read; combined with inclusion as an intersection (b is downloaded but has unread, so it's excluded)
	let (_, body) = request(&app, "GET", "/api/v1/library?filterExclude=unread", None).await;
	assert_eq!(entry_ids(&body), vec![c]);
	let (_, body) = request(
		&app,
		"GET",
		"/api/v1/library?filter=downloaded&filterExclude=unread",
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	let (status, _) = request(&app, "GET", "/api/v1/library?filterExclude=bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// combination: search + title sort
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}&sort=title", urlencoding(ta)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), vec![a]);
}

#[tokio::test]
async fn library_latest_chapter_sort() {
	let TestApp { app, db_path, .. } = test_app().await;
	let ids = register_three_manga(&app).await;
	let (a, b, c) = (ids[0].0, ids[1].0, ids[2].0);
	for id in [a, b, c] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let pool = connect_db(&db_path).await;
	for id in [a, b] {
		sqlx::query("UPDATE chapter SET fetched_at = 100, date_uploaded = 100 WHERE manga_id = ?")
			.bind(id)
			.execute(&pool)
			.await
			.expect("reset chapter timestamps");
	}
	let a_chapters = first_chapters(&app, a, 2).await;
	let b_chapters = first_chapters(&app, b, 2).await;
	for (chapter_id, fetched_at, date_uploaded) in [
		(a_chapters[0], 100, 900),
		(a_chapters[1], 200, 900),
		(b_chapters[0], 300, 100),
		(b_chapters[1], 100, 100),
	] {
		sqlx::query("UPDATE chapter SET fetched_at = ?, date_uploaded = ? WHERE id = ?")
			.bind(fetched_at)
			.bind(date_uploaded)
			.bind(chapter_id)
			.execute(&pool)
			.await
			.expect("set chapter timestamps");
	}
	for (id, last_fetch) in [(a, 900), (b, 100), (c, 1000)] {
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(last_fetch)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set chapter refresh timestamp");
	}
	sqlx::query("DELETE FROM chapter WHERE manga_id = ?")
		.bind(c)
		.execute(&pool)
		.await
		.expect("remove chapters");

	let (status, body) = request(&app, "GET", "/api/v1/library?sort=latestChapter", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(entry_ids(&body), vec![b, a, c]);

	sqlx::query("UPDATE chapter SET fetched_at = 300 WHERE id = ?")
		.bind(a_chapters[1])
		.execute(&pool)
		.await
		.expect("tie latest chapter timestamp");
	let (status, body) = request(&app, "GET", "/api/v1/library?sort=latestChapter", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(entry_ids(&body), vec![a, b, c]);
}

#[tokio::test]
async fn library_latest_chapter_refresh_keeps_first_seen_order() {
	let TestApp { app, db_path, .. } = test_app().await;
	let ids = register_three_manga(&app).await;
	let (a, b) = (ids[0].0, ids[1].0);
	for id in [a, b] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let pool = connect_db(&db_path).await;
	for (id, fetched_at, last_fetch) in [(a, 100, 1), (b, 200, 200)] {
		sqlx::query("UPDATE chapter SET fetched_at = ? WHERE manga_id = ?")
			.bind(fetched_at)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set first-seen timestamps");
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(last_fetch)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set refresh timestamps");
	}
	let before: Vec<(i64, i64)> = sqlx::query_as("SELECT id, fetched_at FROM chapter WHERE manga_id = ? ORDER BY id")
		.bind(a)
		.fetch_all(&pool)
		.await
		.expect("chapters before refresh");
	assert!(!before.is_empty());

	let (status, body) = request(&app, "GET", "/api/v1/library?sort=latestChapter", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(entry_ids(&body), vec![b, a]);
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{a}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body.as_array().expect("chapters").len(), before.len());
	let after: Vec<(i64, i64)> = sqlx::query_as("SELECT id, fetched_at FROM chapter WHERE manga_id = ? ORDER BY id")
		.bind(a)
		.fetch_all(&pool)
		.await
		.expect("chapters after refresh");
	assert_eq!(after, before);
	let last_fetch: Option<i64> = sqlx::query_scalar("SELECT chapters_last_fetched_at FROM manga WHERE id = ?")
		.bind(a)
		.fetch_one(&pool)
		.await
		.expect("refresh timestamp");
	assert!(last_fetch.expect("chapter refresh timestamp") > 200);
	let (status, body) = request(&app, "GET", "/api/v1/library?sort=latestChapter", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(entry_ids(&body), vec![b, a]);
}

#[tokio::test]
async fn library_source_counts() {
	let TestApp { app, .. } = test_app().await;
	let (_, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(body.as_array().expect("array").len(), 0, "{body}");

	let ids = register_three_manga(&app).await;
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	for (id, _) in ids.iter().take(2) {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}

	let (status, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!([{ "sourceId": source_id, "count": 2 }]), "{body}");

	// the count follows after removing one
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{}", ids[0].0), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(body, json!([{ "sourceId": source_id, "count": 1 }]), "{body}");
}

/// Library id set (order-agnostic assertion: sort before comparing)
async fn library_ids(app: &Router, qs: &str) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/library?{qs}"), None).await;
	assert_eq!(status, StatusCode::OK, "{qs}");
	let mut ids = entry_ids(&body);
	ids.sort_unstable();
	ids
}

#[tokio::test]
async fn library_rating_source_filter() {
	let TestApp { app, tmp, db_path } = test_app().await;
	// the second source uses the same fixture payload; a different key makes it another source
	install_fixture_source(&tmp, "en.example-two", "Example Two", &fixture_dir().join("main.wasm"));
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let sources = body.as_array().expect("sources").clone();
	let source_id_of = |key: &str| {
		sources
			.iter()
			.find(|s| s["key"].as_str() == Some(key))
			.unwrap_or_else(|| panic!("source id for {key}"))["id"]
			.as_i64()
			.expect("source id")
	};
	let one = source_id_of("en.example-source");
	let two = source_id_of("en.example-two");

	// register 3 on source one, 1 on source two, all shelved; ratings are set directly in the DB (the fixture doesn't produce them)
	let mut registered: Vec<(i64, String)> = Vec::new();
	for (source, take) in [(one, 3), (two, 1)] {
		let (_, body) = request(
			&app,
			"POST",
			&format!("/api/v1/sources/{source}/search"),
			Some(json!({"page": 1})),
		)
		.await;
		for entry in body["entries"].as_array().expect("entries").iter().take(take) {
			let title = entry["title"].as_str().expect("manga title").to_string();
			let (status, body) = request(
				&app,
				"POST",
				"/api/v1/manga",
				Some(json!({"sourceId": source, "manga": entry})),
			)
			.await;
			assert_eq!(status, StatusCode::OK);
			let id = body["id"].as_i64().expect("manga id");
			let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
			assert_eq!(status, StatusCode::OK);
			registered.push((id, title));
		}
	}
	let (a, b, c, d) = (
		registered[0].0,
		registered[1].0,
		registered[2].0,
		registered[3].0,
	);
	let tb = registered[1].1.clone();
	let pool = connect_db(&db_path).await;
	for (id, rating) in [(a, 1), (b, 2), (c, 3), (d, 3)] {
		sqlx::query("UPDATE manga SET content_rating = ? WHERE id = ?")
			.bind(rating)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set content_rating");
	}

	// rating: OR within the include set, NOT-OR for the exclude set
	assert_eq!(library_ids(&app, "contentRating=1,2").await, vec![a, b]);
	assert_eq!(library_ids(&app, "contentRating=3").await, vec![c, d]);
	assert_eq!(library_ids(&app, "contentRatingExclude=3").await, vec![a, b]);
	assert_eq!(
		library_ids(&app, "contentRating=1,2,3&contentRatingExclude=2").await,
		vec![a, c, d]
	);
	// source: OR within the include set, NOT-OR for the exclude set
	assert_eq!(library_ids(&app, &format!("source={one}")).await, vec![a, b, c]);
	assert_eq!(library_ids(&app, &format!("sourceExclude={one}")).await, vec![d]);
	assert_eq!(
		library_ids(&app, &format!("source={one},{two}&sourceExclude={two}")).await,
		vec![a, b, c]
	);
	// AND across dimensions
	assert_eq!(
		library_ids(&app, &format!("contentRating=3&source={one}")).await,
		vec![c]
	);
	assert_eq!(
		library_ids(&app, &format!("contentRatingExclude=3&sourceExclude={two}")).await,
		vec![a, b]
	);
	// empty / pure whitespace counts as absent
	let mut all = vec![a, b, c, d];
	all.sort_unstable();
	assert_eq!(library_ids(&app, "contentRating=").await, all);
	assert_eq!(library_ids(&app, "contentRating=%20").await, all);
	assert_eq!(library_ids(&app, "source=%20%20").await, all.clone());
	assert_eq!(library_ids(&app, "sourceExclude=").await, all);
	// illegal tokens 400: out-of-range rating, non-integer
	for qs in [
		"contentRating=0",
		"contentRating=4",
		"contentRating=x",
		"contentRatingExclude=9",
		"source=abc",
		"sourceExclude=1.5",
	] {
		let (status, body) = request(&app, "GET", &format!("/api/v1/library?{qs}"), None).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{qs}");
		assert_eq!(body["error"]["code"], "bad_request", "{qs}");
	}

	// bind position reconciliation: search + categoryId + the category/rating/source tri-state parameters
	// fired together; any swap breaks it
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Fav"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat = body["id"].as_i64().expect("category id");
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{b}/categories"),
		Some(json!({"categoryIds": [cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let qs = format!(
		"search={}&categoryId={cat}&category={cat}&contentRating=2&contentRatingExclude=3&source={one},{two}&sourceExclude={two}",
		urlencoding(&tb)
	);
	assert_eq!(library_ids(&app, &qs).await, vec![b]);
}

/// Category tri-state (`category`/`categoryExclude`, 0 = uncategorized): OR within a set, NOT-OR for the exclude set, AND across
#[tokio::test]
async fn library_category_tri_state_filter() {
	let TestApp { app, .. } = test_app().await;
	// the fixture source directory exists but is only registered by an explicit scan
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	// four shelved manga: a → cat1, b → cat1+cat2, c → cat2, d → uncategorized
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entries = body["entries"].as_array().expect("entries").clone();
	assert!(entries.len() >= 4, "fixture yields at least 4 manga");
	let mut registered = Vec::new();
	for entry in entries.iter().take(4) {
		let (status, body) = request(
			&app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": entry})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		let id = body["id"].as_i64().expect("manga id");
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
		registered.push(id);
	}
	let (a, b, c, d) = (registered[0], registered[1], registered[2], registered[3]);
	let cat = |name: &str| {
		let name = name.to_string();
		let app = &app;
		async move {
			let (status, body) = request(app, "POST", "/api/v1/categories", Some(json!({"name": name}))).await;
			assert_eq!(status, StatusCode::OK);
			body["id"].as_i64().expect("category id")
		}
	};
	let cat1 = cat("Cat One").await;
	let cat2 = cat("Cat Two").await;
	let shelve = |manga: i64, cats: Vec<i64>| {
		let app = &app;
		async move {
			let (status, _) = request(
				app,
				"PUT",
				&format!("/api/v1/manga/{manga}/categories"),
				Some(json!({"categoryIds": cats})),
			)
			.await;
			assert_eq!(status, StatusCode::OK);
		}
	};
	shelve(a, vec![cat1]).await;
	shelve(b, vec![cat1, cat2]).await;
	shelve(c, vec![cat2]).await;

	// include: OR within the set, 0 is the uncategorized arm
	assert_eq!(library_ids(&app, &format!("category={cat1}")).await, vec![a, b]);
	assert_eq!(
		library_ids(&app, &format!("category={cat1},{cat2}")).await,
		vec![a, b, c]
	);
	assert_eq!(library_ids(&app, "category=0").await, vec![d]);
	assert_eq!(
		library_ids(&app, &format!("category={cat1},0")).await,
		vec![a, b, d]
	);
	// exclude: NOT-OR, exclude-0 keeps only categorized manga
	assert_eq!(
		library_ids(&app, &format!("categoryExclude={cat1}")).await,
		vec![c, d]
	);
	assert_eq!(library_ids(&app, "categoryExclude=0").await, vec![a, b, c]);
	// AND across the two sets: b matches the include but lives in an excluded category
	assert_eq!(
		library_ids(&app, &format!("category={cat1},{cat2}&categoryExclude={cat2}")).await,
		vec![a]
	);
	// empty / pure whitespace counts as absent
	let mut all = vec![a, b, c, d];
	all.sort_unstable();
	assert_eq!(library_ids(&app, "category=").await, all);
	assert_eq!(library_ids(&app, "categoryExclude=%20").await, all);
	// illegal tokens 400: negative and non-integer ids
	for qs in ["category=-1", "category=x", "categoryExclude=1.5"] {
		let (status, body) = request(&app, "GET", &format!("/api/v1/library?{qs}"), None).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{qs}");
		assert_eq!(body["error"]["code"], "bad_request", "{qs}");
	}
}

/// Author/tag tri-state (`author`/`authorExclude`/`tag`/`tagExclude`): exact case-insensitive
/// match against the manga's JSON arrays, OR within a set, NOT-OR for the exclude set, AND across
#[tokio::test]
async fn library_author_tag_filter() {
	let TestApp { app, db_path, .. } = test_app().await;
	let ids = register_four_shelved(&app).await;
	let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
	let pool = connect_db(&db_path).await;
	// the fixture doesn't produce authors/tags; b deliberately varies the case to prove NOCASE
	set_manga_lists(
		&pool,
		&[
			(a, r#"["Asanagi","Team X"]"#, r#"["action","romance"]"#),
			(b, r#"["asanagi"]"#, r#"["Action"]"#),
			(c, r#"["Murasaki"]"#, r#"["drama"]"#),
			// d stays uninitialized: empty arrays, the way a manga without metadata looks
		],
	)
	.await;
	let mut all = vec![a, b, c, d];
	all.sort_unstable();

	// include: OR within the set, exact but case-insensitive; names with spaces pass percent-encoded
	assert_eq!(library_ids(&app, "author=Asanagi").await, vec![a, b]);
	assert_eq!(library_ids(&app, "author=asanagi").await, vec![a, b]);
	assert_eq!(library_ids(&app, "author=Team%20X").await, vec![a]);
	assert_eq!(library_ids(&app, "author=Asanagi,Murasaki").await, vec![a, b, c]);
	assert_eq!(library_ids(&app, "author=Nobody").await, Vec::<i64>::new());
	assert_eq!(library_ids(&app, "tag=action").await, vec![a, b]);
	assert_eq!(library_ids(&app, "tag=romance,drama").await, vec![a, c]);
	// exclude: NOT-OR; the uninitialized d has nothing to match, so excludes keep it
	assert_eq!(library_ids(&app, "authorExclude=Asanagi").await, vec![c, d]);
	assert_eq!(library_ids(&app, "tagExclude=drama").await, vec![a, b, d]);
	assert_eq!(
		library_ids(&app, "author=Asanagi,Murasaki&authorExclude=Murasaki").await,
		vec![a, b]
	);
	// AND across dimensions: include never matches the uninitialized d
	assert_eq!(library_ids(&app, "author=Asanagi&tag=action").await, vec![a, b]);
	assert_eq!(library_ids(&app, "author=Asanagi&tag=romance").await, vec![a]);
	assert_eq!(library_ids(&app, "tagExclude=action,romance,drama").await, vec![d]);
	// empty / pure whitespace counts as absent
	assert_eq!(library_ids(&app, "author=").await, all);
	assert_eq!(library_ids(&app, "author=%20").await, all);
	assert_eq!(library_ids(&app, "tag=,,").await, all);
	assert_eq!(library_ids(&app, "tagExclude=%20%20").await, all);
}

/// Distinct author/tag lists behind the filter dropdowns: case variants collapse into one entry,
/// counts follow library membership
#[tokio::test]
async fn library_author_tag_counts() {
	let TestApp { app, db_path, .. } = test_app().await;
	for path in ["/api/v1/library/authors", "/api/v1/library/tags"] {
		let (status, body) = request(&app, "GET", path, None).await;
		assert_eq!(status, StatusCode::OK, "{path}");
		assert_eq!(body.as_array().expect("array").len(), 0, "{body}");
	}

	let ids = register_four_shelved(&app).await;
	let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
	let pool = connect_db(&db_path).await;
	set_manga_lists(
		&pool,
		&[
			(a, r#"["Asanagi","Team X"]"#, r#"["action","romance"]"#),
			(b, r#"["asanagi"]"#, r#"["Action"]"#),
			(c, r#"["Murasaki"]"#, r#"["drama"]"#),
			// the fixture registers entry 3 with its own author/tag metadata;
			// zero d out so it stands in for a manga with empty lists
			(d, "[]", "[]"),
		],
	)
	.await;

	// NOCASE grouping collapses the Asanagi/asanagi pair into one entry with count 2; the
	// emptied d contributes nothing
	let name_counts = |body: &Value| -> Vec<(String, i64)> {
		body.as_array()
			.expect("array")
			.iter()
			.map(|e| {
				(
					e["name"].as_str().expect("name").to_lowercase(),
					e["count"].as_i64().expect("count"),
				)
			})
			.collect()
	};
	let (status, body) = request(&app, "GET", "/api/v1/library/authors", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		name_counts(&body),
		vec![
			("asanagi".to_string(), 2),
			("murasaki".to_string(), 1),
			("team x".to_string(), 1)
		],
		"{body}"
	);
	let (_, body) = request(&app, "GET", "/api/v1/library/tags", None).await;
	assert_eq!(
		name_counts(&body),
		vec![
			("action".to_string(), 2),
			("drama".to_string(), 1),
			("romance".to_string(), 1)
		],
		"{body}"
	);

	// removing a from the library drops its names and counts; unshelving d changes nothing
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{a}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library/authors", None).await;
	assert_eq!(
		name_counts(&body),
		vec![("asanagi".to_string(), 1), ("murasaki".to_string(), 1)],
		"{body}"
	);
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{d}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library/authors", None).await;
	assert_eq!(
		name_counts(&body),
		vec![("asanagi".to_string(), 1), ("murasaki".to_string(), 1)],
		"{body}"
	);
}

/// Registers four search results, shelves them all, returns their ids
async fn register_four_shelved(app: &Router) -> Vec<i64> {
	// register_three_manga registers but does not shelve; the fixture yields at least 4 entries,
	// so register and shelve the 4th the same way
	let manga = register_three_manga(app).await;
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entry = &body["entries"].as_array().expect("entries")[3];
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": entry})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let fourth = body["id"].as_i64().expect("manga id");
	let mut ids: Vec<i64> = manga.iter().map(|(id, _)| *id).collect();
	ids.push(fourth);
	for id in &ids {
		let (status, _) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	ids
}

/// Writes the authors/tags JSON array columns directly so the test does not
/// depend on whatever metadata the fixture's search entries carry
async fn set_manga_lists(pool: &sqlx::SqlitePool, rows: &[(i64, &str, &str)]) {
	for (id, authors, tags) in rows {
		sqlx::query("UPDATE manga SET authors = ?, tags = ? WHERE id = ?")
			.bind(authors)
			.bind(tags)
			.bind(id)
			.execute(pool)
			.await
			.expect("set authors/tags");
	}
}

/// Percent-encodes reserved characters in the query value (test titles are ASCII, byte-wise handling suffices)
fn urlencoding(raw: &str) -> String {
	let mut out = String::new();
	for byte in raw.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

#[tokio::test]
async fn embedded_webui() {
	let TestApp { app, tmp, .. } = test_app().await;
	// test_app's webui_dir doesn't exist under tmp → the disk branch is not taken
	assert!(!tmp.path().join("webui").is_dir());
	if sumidoku_server::webui_assets::has_index() {
		let index = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../webui/build/index.html"))
			.expect("repo webui index");
		let (status, headers, bytes) = request_full(&app, "GET", "/", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "no-cache");
		assert_eq!(bytes, index);

		// SPA routes fall back to index.html
		let (status, headers, bytes) = request_full(&app, "GET", "/some/spa/route", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "no-cache");
		assert_eq!(bytes, index);

		for path in ["/sw.js", "/manifest.webmanifest"] {
			let (status, headers, _) = request_full(&app, "GET", path, None).await;
			assert_eq!(status, StatusCode::OK);
			assert_eq!(headers["cache-control"], "no-cache");
		}
		let (status, headers, _) = request_full(&app, "GET", "/icon.svg", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "private, max-age=3600");

		// path traversal is rejected, keeping the JSON 404
		let (status, body) = request(&app, "GET", "/../../secret", None).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
		assert_eq!(body["error"]["code"], "not_found");

		// the API is not swallowed by the SPA
		let (status, body) = request(&app, "GET", "/api/v1/nope", None).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
		assert_eq!(body["error"]["code"], "not_found");
	} else {
		// environments without webui artifacts (clean CI checkout): respond with the "webui not built" text
		let (status, _, bytes) = request_full(&app, "GET", "/", None).await;
		assert_eq!(status, StatusCode::OK);
		assert!(String::from_utf8(bytes).expect("utf8").contains("webui not built"));
	}
}

/// chapter id → isRead map of a chapters-list response
fn read_states(body: &Value) -> std::collections::HashMap<i64, bool> {
	body.as_array()
		.expect("chapters")
		.iter()
		.map(|c| {
			(
				c["id"].as_i64().expect("chapter id"),
				c["isRead"].as_bool().expect("isRead"),
			)
		})
		.collect()
}

/// A mangaIds mark-read expansion targets only readable chapters: untrashed, and either unlocked
/// or already downloaded by the user. chapterIds merge in deduplicated.
#[tokio::test]
async fn batch_mark_read_by_manga_targets_only_readable_chapters() {
	let TestApp { app, db_path, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let ids: Vec<i64> = body
		.as_array()
		.expect("chapters")
		.iter()
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect();
	assert_eq!(ids.len(), 8);
	let pool = connect_db(&db_path).await;
	// ids[1]: locked and not downloaded — unreadable; ids[2]: locked but downloaded — readable
	for chapter_id in [ids[1], ids[2]] {
		sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?")
			.bind(chapter_id)
			.execute(&pool)
			.await
			.expect("lock chapter");
	}
	put_chapter_user(&pool, ids[2], false, false, true, None).await;
	// ids[3]: in the trash
	sqlx::query("UPDATE chapter SET trashed_at = 1 WHERE id = ?")
		.bind(ids[3])
		.execute(&pool)
		.await
		.expect("trash chapter");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": [ids[0]], "mangaIds": [manga_id, 999], "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 6);

	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let states = read_states(&body);
	for (position, chapter_id) in ids.iter().enumerate() {
		let want = !matches!(position, 1 | 3);
		assert_eq!(states[chapter_id], want, "chapter at position {position}");
	}

	// empty selections are a no-op
	for patch in [
		json!({"isRead": true}),
		json!({"mangaIds": [], "isRead": true}),
		json!({"mangaIds": [999], "isRead": true}),
	] {
		let (status, body) = request(&app, "POST", "/api/v1/chapter/batch", Some(patch)).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["updated"], 0, "{body}");
	}
}

/// Mark-unread and other patches expand a manga selection to every untrashed chapter: locked
/// chapters are included there, only the trash is skipped.
#[tokio::test]
async fn batch_mark_unread_by_manga_skips_only_trashed() {
	let TestApp { app, db_path, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let ids: Vec<i64> = body
		.as_array()
		.expect("chapters")
		.iter()
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect();
	assert_eq!(ids.len(), 8);
	let pool = connect_db(&db_path).await;
	for chapter_id in &ids {
		put_chapter_user(&pool, *chapter_id, true, false, false, Some(100)).await;
	}
	// ids[1]: locked, not downloaded; ids[3]: in the trash
	sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?")
		.bind(ids[1])
		.execute(&pool)
		.await
		.expect("lock chapter");
	sqlx::query("UPDATE chapter SET trashed_at = 1 WHERE id = ?")
		.bind(ids[3])
		.execute(&pool)
		.await
		.expect("trash chapter");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"mangaIds": [manga_id], "isRead": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 7);

	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let states = read_states(&body);
	for (position, chapter_id) in ids.iter().enumerate() {
		let want = position == 3;
		assert_eq!(states[chapter_id], want, "chapter at position {position}");
	}
	// the locked chapter's trace is wiped like any other; the trashed one keeps its row untouched
	let chapter = body
		.as_array()
		.expect("chapters")
		.iter()
		.find(|c| c["id"] == ids[1])
		.expect("locked chapter");
	assert_eq!(chapter["lastPageRead"], 0);
	assert!(chapter["lastReadAt"].is_null());
	let chapter = body
		.as_array()
		.expect("chapters")
		.iter()
		.find(|c| c["id"] == ids[3])
		.expect("trashed chapter");
	assert_eq!(chapter["lastReadAt"], 100);

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"mangaIds": [manga_id], "isBookmarked": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 7);
}

/// A selection larger than SQLite's variable cap (32766 with the bundled build) is expanded in
/// chunks: the endpoint answers instead of failing with "too many SQL variables".
#[tokio::test]
async fn batch_manga_expansion_beyond_sql_variable_limit() {
	let TestApp { app, .. } = test_app().await;
	let manga_ids: Vec<i64> = (900_001..=940_000).collect();
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"mangaIds": manga_ids, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 0);
}
