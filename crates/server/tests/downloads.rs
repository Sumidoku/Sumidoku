mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{
	TestApp, base_config, connect_db, connect_ws, fixture_dir, point_page_at_local, read_ws_frame, read_ws_kind,
	register_example_manga, request, request_full, serve_app, serve_bytes,
};
use common::PNG;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use sumidoku_server::download::sanitize;

/// Download tests need downloads_dir inside tmp (base_config doesn't set it; the default ./downloads would pollute the repo)
async fn downloads_app(configure: impl FnOnce(&mut Config)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	configure(&mut config);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn first_chapter_id(app: &Router, manga_id: i64) -> i64 {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("chapter id")
}

async fn wait_download_status(app: &Router, chapter_id: i64, want: &str) -> Value {
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (status, body) = request(app, "GET", "/api/v1/downloads", None).await;
		assert_eq!(status, StatusCode::OK);
		if let Some(entry) = body
			.as_array()
			.and_then(|rows| rows.iter().find(|row| row["chapterId"] == chapter_id))
			.filter(|entry| entry["status"] == want)
		{
			return entry.clone();
		}
		assert!(Instant::now() < deadline, "download status did not become {want}: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

fn assert_cbz(bytes: &[u8], source_key: &str, manga_key: &str, chapter_key: &str, pages: usize) {
	use std::io::Read;
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("open cbz");
	assert_eq!(archive.len(), pages + 1);
	{
		let mut info = archive.by_index(0).expect("first entry");
		assert_eq!(info.name().expect("entry name"), "ComicInfo.xml");
		let mut text = String::new();
		info.read_to_string(&mut text).expect("read ComicInfo");
		assert!(text.contains("!SUMIDOKUDATA"), "{text}");
		assert!(text.contains(&format!("\"sourceKey\":\"{source_key}\"")), "{text}");
		assert!(text.contains(&format!("\"mangaKey\":\"{manga_key}\"")), "{text}");
		assert!(text.contains(&format!("\"chapterKey\":\"{chapter_key}\"")), "{text}");
		assert!(text.contains(&format!("<PageCount>{pages}</PageCount>")), "{text}");
	}
	let names: Vec<String> = (1..archive.len())
		.map(|i| archive.by_index(i).expect("entry").name().expect("entry name").into_owned())
		.collect();
	assert!(names.contains(&"000.png".to_string()), "{names:?}");
	assert!(names.contains(&"001.txt".to_string()), "{names:?}");
	{
		let mut image = archive.by_name("000.png").expect("image entry");
		let mut data = Vec::new();
		image.read_to_end(&mut data).expect("read image entry");
		assert_eq!(data, PNG);
	}
	for i in 0..archive.len() {
		let entry = archive.by_index(i).expect("entry");
		let modified = entry.last_modified().expect("entry timestamp");
		assert_eq!(
			(
				modified.year(),
				modified.month(),
				modified.day(),
				modified.hour(),
				modified.minute(),
				modified.second()
			),
			(1980, 1, 1, 0, 0, 0),
			"entry {} timestamp",
			entry.name().expect("entry name")
		);
	}
}

#[tokio::test]
async fn download_full_flow() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let downloads_dir = t.tmp.path().join("downloads");
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");

	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let (manga_key,): (String,) = sqlx::query_as("SELECT key FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("manga key");
	let (chapter_key,): (String,) = sqlx::query_as("SELECT key FROM chapter WHERE id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("chapter key");

	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	// a duplicate enqueue is skipped
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));

	let entry = wait_download_status(&app, chapter_id, "finished").await;
	let rel_path = entry["path"].as_str().expect("download path").to_string();
	let cbz_path = downloads_dir.join(&rel_path);
	assert!(cbz_path.is_file(), "cbz should exist at {}", cbz_path.display());
	assert_eq!(entry["downloaded"], Value::Null);
	assert_eq!(entry["total"], Value::Null);

	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(body[0]["isDownloaded"], true);
	assert_eq!(body[1]["isDownloaded"], false);

	let bytes = std::fs::read(&cbz_path).expect("read cbz");
	assert_cbz(&bytes, "en.example-source", &manga_key, &chapter_key, 2);

	// delete and re-download: the artifact bytes are identical
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!cbz_path.exists());
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(body[0]["isDownloaded"], false);

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&app, chapter_id, "finished").await;
	let bytes_again = std::fs::read(&cbz_path).expect("read cbz again");
	assert_eq!(bytes, bytes_again, "re-download should be byte-identical");

	let (status, headers, file_bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "application/zip");
	assert_eq!(file_bytes, bytes_again);

	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!cbz_path.exists());
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_backfills_online_page_dimensions() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let pages_path = format!("/api/v1/chapter/{chapter_id}/pages");
	let (status, _) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;

	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body[0].get("width").is_none());
	assert!(body[0].get("height").is_none());
	assert!(body[1]["content"]["Text"].is_string());

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&app, chapter_id, "finished").await;
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["width"], 1);
	assert_eq!(body[0]["height"], 1);
	assert!(body[1].get("width").is_none());
	assert!(body[1].get("height").is_none());
	let rows: Vec<(bool, Option<i64>, Option<i64>)> = sqlx::query_as(
		"SELECT dimensions_checked, width, height FROM page WHERE chapter_id = ? ORDER BY idx",
	)
	.bind(chapter_id)
	.fetch_all(&pool)
	.await
	.expect("dimension rows");
	assert_eq!(rows, [(true, Some(1), Some(1)), (false, None, None)]);

	sqlx::query("UPDATE page SET width = 99, height = 99, dimensions_checked = 1 WHERE chapter_id = ? AND idx = 0")
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("seed stale dimensions");
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&app, chapter_id, "finished").await;
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["width"], 1);
	assert_eq!(body[0]["height"], 1);
}

/// Metadata mirror for rendering expected paths (chapter_display and the author empty fallback follow the server's rules)
struct ChapterMeta {
	source: String,
	title: String,
	author: String,
	chapter: String,
}

async fn chapter_meta(pool: &sqlx::SqlitePool, chapter_id: i64) -> ChapterMeta {
	let (title, authors, number, chapter_title, chapter_key, source): (
		String,
		String,
		Option<f64>,
		Option<String>,
		String,
		String,
	) = sqlx::query_as(
		"SELECT m.title, m.authors, c.number, c.title, c.key, s.name FROM chapter c
		 JOIN manga m ON m.id = c.manga_id JOIN source s ON s.id = m.source_id WHERE c.id = ?",
	)
	.bind(chapter_id)
	.fetch_one(pool)
	.await
	.expect("chapter meta");
	let authors: Vec<String> = serde_json::from_str(&authors).expect("authors json");
	let chapter = match (number, chapter_title) {
		(Some(n), _) => format!("第 {n} 话"),
		(None, Some(t)) => t,
		(None, None) => chapter_key,
	};
	ChapterMeta {
		source,
		title,
		author: if authors.is_empty() { "Unknown".into() } else { authors.join(", ") },
		chapter,
	}
}

fn default_rel_path(meta: &ChapterMeta) -> String {
	format!(
		"{}/{}/{}.cbz",
		sanitize(&meta.source),
		sanitize(&meta.title),
		sanitize(&meta.chapter)
	)
}

/// Mirror of a template variable: empty/missing falls back to Unknown, the rest goes through sanitize (same rules as the server's move_var)
fn template_var(raw: &str) -> String {
	if raw.trim().is_empty() { "Unknown".into() } else { sanitize(raw) }
}

#[tokio::test]
async fn download_move_to_absolute_template_target() {
	let t = downloads_app(|config| {
		let root = config.downloads_dir.parent().expect("tmp root").join("library");
		config.download_move_path = Some(format!("{}/{{author}}/{{manga}}/{{filename}}", root.display()));
	})
	.await;
	let app = t.app.clone();
	let move_root = t.tmp.path().join("library");
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let meta = chapter_meta(&pool, chapter_id).await;

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	let stored = entry["path"].as_str().expect("download path").to_string();
	let target = move_root
		.join(template_var(&meta.author))
		.join(template_var(&meta.title))
		.join(format!("{}.cbz", template_var(&meta.chapter)));
	assert_eq!(stored, target.to_str().expect("utf8 target"), "moved download stores the absolute target");
	let (db_path,): (String,) = sqlx::query_as("SELECT path FROM download WHERE chapter_id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("db path");
	assert_eq!(db_path, stored);
	assert!(target.is_file(), "product moved to {}", target.display());
	assert!(
		!t.tmp.path().join("downloads").join(default_rel_path(&meta)).exists(),
		"default location must be empty after the move"
	);

	// file reads use the recorded absolute path verbatim
	let bytes = std::fs::read(&target).expect("read moved cbz");
	let (status, _, file_bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(file_bytes, bytes);

	// deletion uses the recorded absolute path
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!target.exists());
}

#[tokio::test]
async fn download_move_via_setting_and_overwrites_target() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let move_root = t.tmp.path().join("library");
	let template = format!("{}/{{manga}}", move_root.display());
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"download.move_path": template})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["download"]["movePath"], json!(template));

	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let meta = chapter_meta(&pool, chapter_id).await;
	let (manga_key,): (String,) = sqlx::query_as("SELECT key FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("manga key");
	let (chapter_key,): (String,) = sqlx::query_as("SELECT key FROM chapter WHERE id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("chapter key");

	// template without {filename} → directory semantics: the default artifact name is appended under it
	let target = move_root
		.join(template_var(&meta.title))
		.join(format!("{}.cbz", template_var(&meta.chapter)));
	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	assert_eq!(entry["path"].as_str().expect("path"), target.to_str().expect("utf8 target"));
	assert!(target.is_file());

	// re-download overwrite: after deleting the row (deletion must remove the recorded absolute-path
	// artifact), plant a junk file at the target; the re-download must replace it with a real cbz, not leave a duplicate
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!target.exists(), "delete must remove the moved product");
	std::fs::create_dir_all(target.parent().expect("target parent")).expect("mkdir target parent");
	std::fs::write(&target, b"junk").expect("write junk");
	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&app, chapter_id, "finished").await;
	let bytes = std::fs::read(&target).expect("read replaced cbz");
	assert_cbz(&bytes, "en.example-source", &manga_key, &chapter_key, 2);
}

#[tokio::test]
async fn download_move_failure_keeps_product_in_place() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	// the move target's parent is blocked by a same-named file: mkdir must fail → the artifact stays in place and the download still finishes
	let blocker = t.tmp.path().join("blocker");
	std::fs::write(&blocker, b"x").expect("write blocker");
	let template = format!("{}/sub/{{filename}}", blocker.display());
	let (status, _) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"download.move_path": template})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let meta = chapter_meta(&pool, chapter_id).await;

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	let stored = entry["path"].as_str().expect("download path").to_string();
	assert!(
		!std::path::Path::new(&stored).is_absolute(),
		"failed move keeps the relative path: {stored}"
	);
	assert_eq!(stored, default_rel_path(&meta));
	let product = t.tmp.path().join("downloads").join(&stored);
	assert!(product.is_file(), "product kept at {}", product.display());
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["width"], 1);
	assert_eq!(body[0]["height"], 1);
}

#[tokio::test]
async fn download_move_folder_format_moves_directory() {
	let t = downloads_app(|config| {
		config.download_format = "folder".into();
		let root = config.downloads_dir.parent().expect("tmp root").join("library");
		config.download_move_path = Some(format!("{}/{{manga}}/{{filename}}", root.display()));
	})
	.await;
	let app = t.app.clone();
	let move_root = t.tmp.path().join("library");
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let meta = chapter_meta(&pool, chapter_id).await;

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	let stored = entry["path"].as_str().expect("download path").to_string();
	let target = move_root.join(template_var(&meta.title)).join(template_var(&meta.chapter));
	assert_eq!(stored, target.to_str().expect("utf8 target"));
	assert!(target.is_dir(), "folder product moved to {}", target.display());
	assert!(target.join("ComicInfo.xml").is_file());
	assert!(target.join("000.png").is_file());

	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!target.exists());
}

#[tokio::test]
async fn download_pause_resume() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;

	// stop first, then enqueue, avoiding a race with the worker; stop once more so the newly queued row lands as paused
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body[0]["chapterId"], chapter_id);
	assert_eq!(body[0]["status"], "paused");

	let (status, _) = request(&app, "POST", "/api/v1/downloads/start", None).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	assert!(entry["path"].is_string());
}

async fn wait_update_complete(app: &Router, want_total: i64) {
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (status, body) = request(app, "GET", "/api/v1/update/summary", None).await;
		assert_eq!(status, StatusCode::OK);
		if body["running"] == false
			&& body["total"].as_i64() == Some(want_total)
			&& body["complete"].as_i64() == Some(want_total)
		{
			return;
		}
		assert!(Instant::now() < deadline, "update did not complete: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

#[tokio::test]
async fn update_fetch_reconciles_and_recent() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let deleted = sqlx::query("DELETE FROM chapter WHERE key = '8'")
		.execute(&pool)
		.await
		.expect("delete chapter")
		.rows_affected();
	assert_eq!(deleted, 1);

	let (status, _) = request(&app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["started"], true);
	wait_update_complete(&app, 1).await;

	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ? AND key = '8'")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("count chapter");
	assert_eq!(count, 1);

	let (status, body) = request(&app, "GET", "/api/v1/update/recent", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["hasNextPage"], false);
	assert_eq!(body["entries"][0]["chapter"]["key"], "8");
	assert_eq!(body["entries"][0]["manga"]["id"], manga_id);

	// auto_download off: the download table is empty
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM download")
		.fetch_one(&pool)
		.await
		.expect("count download");
	assert_eq!(count, 0);
}

#[tokio::test]
async fn update_auto_download_enqueues_new_chapters() {
	let t = downloads_app(|config| {
		config.auto_download_new_chapters = true;
	})
	.await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let deleted = sqlx::query("DELETE FROM chapter WHERE key = '8'")
		.execute(&pool)
		.await
		.expect("delete chapter")
		.rows_affected();
	assert_eq!(deleted, 1);

	let (status, _) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.skip_titles": [], "downloads.auto_skip_manga_with_unread": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["started"], true);
	wait_update_complete(&app, 1).await;

	// the new chapter is auto-enqueued (the worker's later success or failure doesn't change the enqueue fact)
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let row: Option<(String,)> = sqlx::query_as(
			"SELECT d.status FROM download d JOIN chapter c ON c.id = d.chapter_id WHERE c.key = '8'",
		)
		.fetch_optional(&pool)
		.await
		.expect("query download");
		if row.is_some() {
			break;
		}
		assert!(Instant::now() < deadline, "auto download did not enqueue chapter 8");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

#[tokio::test]
async fn downloads_ws_pushes_snapshot() {
	let t = downloads_app(|_| {}).await;
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/downloads/ws", None).await;
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let value: Value = serde_json::from_slice(&payload).expect("snapshot json");
	assert!(value["type"].is_string(), "{value}");
	assert_eq!(value["type"], "snapshot");
	assert!(value["downloads"].is_array(), "{value}");
	assert_eq!(value["running"], true, "{value}");

	// stop → running event frame (chapterId=0 placeholder, running=false)
	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let event: Value = serde_json::from_slice(&payload).expect("running event json");
	assert_eq!(event["type"], "running", "{event}");
	assert_eq!(event["chapterId"], 0);
	assert_eq!(event["downloaded"], 0);
	assert_eq!(event["total"], 0);
	assert_eq!(event["running"], false);

	// start → running=true
	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/start", None).await;
	assert_eq!(status, StatusCode::OK);
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let event: Value = serde_json::from_slice(&payload).expect("running event json");
	assert_eq!(event["type"], "running", "{event}");
	assert_eq!(event["running"], true);
}

#[tokio::test]
async fn downloads_ws_pushes_queued_event() {
	let t = downloads_app(|_| {}).await;
	let manga_id = register_example_manga(&t.app).await;
	let chapter_id = first_chapter_id(&t.app, manga_id).await;
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/downloads/ws", None).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut stream, &mut rest).await).expect("snapshot");
	assert_eq!(snapshot["type"], "snapshot");

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut stream, &mut rest, "queued").await;
	assert_eq!(event["chapterId"], chapter_id);
}

#[tokio::test]
async fn update_ws_pushes_job_and_completed_summary() {
	let t = downloads_app(|_| {}).await;
	let manga_id = register_example_manga(&t.app).await;
	let (status, _) = request(&t.app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/update/ws", None).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut stream, &mut rest).await).expect("snapshot");
	assert_eq!(snapshot["type"], "summary");

	let (status, _) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&t.app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut stream, &mut rest, "job").await;
	assert_eq!(event["mangaId"], manga_id);
	wait_update_complete(&t.app, 1).await;

	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/update/ws", None).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut stream, &mut rest).await).expect("snapshot");
	assert_eq!(snapshot["type"], "summary");
	assert_eq!(snapshot["running"], false);
	assert_eq!(snapshot["total"], 1);
	assert_eq!(snapshot["complete"], 1);
}

#[tokio::test]
async fn download_cancel_in_flight_removes_the_row() {
	let t = downloads_app(|_| {}).await;
	let manga_id = register_example_manga(&t.app).await;
	let chapter_id = first_chapter_id(&t.app, manga_id).await;
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let handler = || async {
		tokio::time::sleep(Duration::from_secs(2)).await;
		([(axum::http::header::CONTENT_TYPE, "image/png".to_string())], PNG.to_vec())
	};
	let router = Router::new().route("/img.png", axum::routing::get(handler));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let _upstream = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&t.app, chapter_id, "downloading").await;
	let (status, _) = request(&t.app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&t.app, "GET", "/api/v1/downloads", None).await;
	assert!(body.as_array().expect("downloads").is_empty());
	let (_, body) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["isDownloaded"], false);
}

#[tokio::test]
async fn download_delete_while_worker_finishes_cleans_artifact_and_allows_requeue() {
	let t = downloads_app(|_| {}).await;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let base = format!("http://127.0.0.1:{port}");
	let feed_url = format!("{base}/feed");
	let feed = format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Race</title><id>race</id>
  <entry><title>First</title><id>first</id><updated>2024-03-01T00:00:00Z</updated>
    <link rel="http://opds-spec.org/acquisition" type="application/zip" href="{base}/empty.cbz"/>
  </entry>
  <entry><title>Probe</title><id>probe</id><updated>2024-03-01T00:00:00Z</updated>
    <link rel="http://opds-spec.org/acquisition" type="application/zip" href="{base}/empty.cbz"/>
  </entry>
</feed>"#
	);
	let release = std::sync::Arc::new(tokio::sync::Notify::new());
	let first_request = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
	let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
	let feed_release = release.clone();
	let feed_handler = move || {
		let release = feed_release.clone();
		let first_request = first_request.clone();
		let started_tx = started_tx.clone();
		let feed = feed.clone();
		async move {
			if first_request.swap(false, std::sync::atomic::Ordering::SeqCst) {
				let _ = started_tx.send(());
				release.notified().await;
			}
			(
				[(axum::http::header::CONTENT_TYPE, "application/atom+xml;profile=opds-catalog".to_string())],
				feed,
			)
		}
	};
	let mut empty = std::io::Cursor::new(Vec::new());
	zip::ZipWriter::new(&mut empty).finish().expect("empty cbz");
	let empty = empty.into_inner();
	let empty_handler = move || {
		let empty = empty.clone();
		async move { ([(axum::http::header::CONTENT_TYPE, "application/zip".to_string())], empty) }
	};
	let router = Router::new()
		.route("/feed", axum::routing::get(feed_handler))
		.route("/empty.cbz", axum::routing::get(empty_handler));
	let _upstream = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});

	let pool = connect_db(&t.db_path).await;
	let source_id: i64 = sqlx::query_scalar(
		"INSERT INTO source (key, name, kind, base_url, wasm_path)
		 VALUES ('opds.race', 'Race OPDS', 'opds', ?, '') RETURNING id",
	)
	.bind(&feed_url)
	.fetch_one(&pool)
	.await
	.expect("opds source");
	let manga_id: i64 = sqlx::query_scalar(
		"INSERT INTO manga (source_id, key, title, url) VALUES (?, 'race', 'Race Manga', ?) RETURNING id",
	)
	.bind(source_id)
	.bind(&feed_url)
	.fetch_one(&pool)
	.await
	.expect("opds manga");
	let chapter_id: i64 = sqlx::query_scalar(
		"INSERT INTO chapter (manga_id, key, title, number, source_order)
		 VALUES (?, 'first', 'First', 1, 0) RETURNING id",
	)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("first chapter");
	let probe_id: i64 = sqlx::query_scalar(
		"INSERT INTO chapter (manga_id, key, title, number, source_order)
		 VALUES (?, 'probe', 'Probe', 2, 1) RETURNING id",
	)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("probe chapter");
	let meta = chapter_meta(&pool, chapter_id).await;
	let artifact = t.tmp.path().join("downloads").join(default_rel_path(&meta));
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/downloads/ws", None).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut stream, &mut rest).await).expect("snapshot");
	assert_eq!(snapshot["type"], "snapshot");

	let (status, body) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
		.await
		.expect("feed started")
		.expect("feed start signal");
	wait_download_status(&t.app, chapter_id, "downloading").await;
	let (status, _) = request(&t.app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	release.notify_one();

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [probe_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let finished = read_ws_kind(&mut stream, &mut rest, "finished").await;
	assert_eq!(finished["chapterId"], probe_id, "deleted attempt must not emit finished");
	let status: Option<String> = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id = ? AND user_id = 1")
		.bind(chapter_id)
		.fetch_optional(&pool)
		.await
		.expect("deleted row");
	assert_eq!(status, None);
	let downloaded: Option<i64> = sqlx::query_scalar(
		"SELECT is_downloaded FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_optional(&pool)
	.await
	.expect("download flag");
	assert_eq!(downloaded, Some(0));
	assert!(!artifact.exists(), "stale worker recreated {}", artifact.display());
	assert!(!t.tmp.path().join("downloads/.tmp").join(chapter_id.to_string()).exists());

	let (status, body) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	wait_download_status(&t.app, chapter_id, "finished").await;
	assert!(artifact.is_file());
}
