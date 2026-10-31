mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{
	TestApp, base_config, connect_db, connect_sse, fixture_dir, point_page_at_local, read_sse_data, read_sse_kind,
	register_example_manga, request, request_full, serve_app, serve_bytes, source_by_key,
};
use common::PNG;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use sumidoku_server::download::sanitize;

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
async fn download_numbers_serialize_cleanly() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let pool = connect_db(&t.db_path).await;
	sqlx::query("UPDATE chapter SET number = ?, volume = ? WHERE id = ?")
		.bind(3.1_f32 as f64)
		.bind(4.2_f32 as f64)
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("fractional chapter numbers");

	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	let (status, body) = request(&app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["chapterNumber"], json!(3.1));
	assert_eq!(body[0]["volumeNumber"], json!(4.2));
}

#[tokio::test]
async fn download_full_flow() {
	let (logs, _guard) = common::capture_logs();
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
	sqlx::query("UPDATE chapter SET title = NULL, number = 50 WHERE id = ?")
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("numbered chapter without a separate title");
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
	let text = common::wait_for_log(&logs, "download finished").await;
	assert!(text.contains("downloads queued"), "enqueue log missing: {text}");
	assert!(text.contains("download started"), "start log missing: {text}");
	let line = text
		.lines()
		.find(|line| line.contains("download finished"))
		.expect("finish log");
	for field in ["INFO", "pages=2", "attempt=1", "dur_ms=", "source_id="] {
		assert!(line.contains(field), "missing {field:?} in {line}");
	}
	assert!(line.contains(&format!("chapter_id={chapter_id}")), "chapter id missing: {line}");
	let rel_path = entry["path"].as_str().expect("download path").to_string();
	let cbz_path = downloads_dir.join(&rel_path);
	assert!(cbz_path.is_file(), "cbz should exist at {}", cbz_path.display());
	assert_eq!(entry["downloaded"], Value::Null);
	assert_eq!(entry["total"], Value::Null);
	assert_eq!(entry["chapterTitle"], Value::Null);
	assert_eq!(entry["chapterNumber"], json!(50.0));
	assert_eq!(entry["volumeNumber"], Value::Null);

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

/// Two chapters with the same display number (other group, language or volume) used to package to the same
/// path: the second download overwrote the first while the first row still pointed at it
#[tokio::test]
async fn same_numbered_chapters_get_distinct_artifacts() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let downloads_dir = t.tmp.path().join("downloads");
	let manga_id = register_example_manga(&app).await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let ids: Vec<i64> = body.as_array().expect("chapters").iter().map(|c| c["id"].as_i64().expect("id")).collect();
	let (first, second) = (ids[0], ids[1]);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	let (manga_key,): (String,) = sqlx::query_as("SELECT key FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("manga key");
	let mut keys = Vec::new();
	for chapter_id in [first, second] {
		let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
		assert_eq!(status, StatusCode::OK);
		point_page_at_local(&pool, chapter_id, port).await;
		sqlx::query("UPDATE chapter SET title = NULL, number = 7 WHERE id = ?")
			.bind(chapter_id)
			.execute(&pool)
			.await
			.expect("same display number");
		let (key,): (String,) = sqlx::query_as("SELECT key FROM chapter WHERE id = ?")
			.bind(chapter_id)
			.fetch_one(&pool)
			.await
			.expect("chapter key");
		keys.push(key);
	}

	let mut paths = Vec::new();
	for chapter_id in [first, second] {
		let (status, _) =
			request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
		assert_eq!(status, StatusCode::OK);
		let entry = wait_download_status(&app, chapter_id, "finished").await;
		paths.push(entry["path"].as_str().expect("download path").to_string());
	}
	assert_ne!(paths[0], paths[1], "each chapter needs its own artifact");
	assert!(paths[1].contains(&format!("[{second}]")), "the later chapter is disambiguated: {}", paths[1]);
	for (path, key) in paths.iter().zip(&keys) {
		let bytes = std::fs::read(downloads_dir.join(path)).expect("read cbz");
		assert_cbz(&bytes, "en.example-source", &manga_key, key, 2);
	}
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
	let (logs, _guard) = common::capture_logs();
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let chapter_id: i64 = sqlx::query_scalar("SELECT id FROM chapter WHERE key = '8'")
		.fetch_one(&pool)
		.await
		.expect("chapter id");
	common::delete_chapter(&pool, chapter_id).await;

	let (status, _) = request(&app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["started"], true);
	wait_update_complete(&app, 1).await;
	let text = common::wait_for_log(&logs, "library update finished").await;
	assert!(text.contains("library update started"), "update start log missing: {text}");
	let line = text
		.lines()
		.find(|line| line.contains("library update finished"))
		.expect("update finish log");
	for field in ["INFO", "scheduled=false", "total=1", "complete=1", "failed=0", "skipped=0", "dur_ms="] {
		assert!(line.contains(field), "missing {field:?} in {line}");
	}

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
	let chapter_id: i64 = sqlx::query_scalar("SELECT id FROM chapter WHERE key = '8'")
		.fetch_one(&pool)
		.await
		.expect("chapter id");
	common::delete_chapter(&pool, chapter_id).await;
	let addr = serve_app(app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["type"], "snapshot");

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
	let queued = read_sse_data(&mut stream, &mut rest, "queued").await;
	let chapter_key: String = sqlx::query_scalar("SELECT key FROM chapter WHERE id = ?")
		.bind(queued["chapterId"].as_i64().expect("queued chapter id"))
		.fetch_one(&pool)
		.await
		.expect("queued chapter");
	assert_eq!(chapter_key, "8");
	let completed = read_sse_kind(&mut stream, &mut rest, "updateComplete").await;
	assert_eq!(completed["data"]["running"], false);
	assert_eq!(completed["data"]["total"], 1);
	assert_eq!(completed["data"]["complete"], 1);
}

#[tokio::test]
async fn legacy_websocket_routes_are_not_registered() {
	let t = downloads_app(|_| {}).await;
	for path in ["/api/v1/downloads/ws", "/api/v1/update/ws"] {
		let request = axum::http::Request::get(path)
			.body(axum::body::Body::empty())
			.expect("legacy route request");
		let response = tower::ServiceExt::oneshot(t.app.clone(), request).await.expect("legacy route response");
		let status = response.status();
		assert!(
			status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
			"unexpected legacy WebSocket response for {path}: {status}"
		);
	}
}

#[tokio::test]
async fn downloads_sse_pushes_snapshot() {
	let t = downloads_app(|_| {}).await;
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let value = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert!(value["type"].is_string(), "{value}");
	assert_eq!(value["type"], "snapshot");
	assert!(value["downloads"].is_array(), "{value}");
	assert_eq!(value["running"], true, "{value}");

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_sse_kind(&mut stream, &mut rest, "running").await;
	assert_eq!(event["data"]["running"], false, "{event}");

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/start", None).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_sse_kind(&mut stream, &mut rest, "running").await;
	assert_eq!(event["data"]["running"], true, "{event}");
}

#[tokio::test]
async fn downloads_sse_pushes_queued_event() {
	let t = downloads_app(|_| {}).await;
	let manga_id = register_example_manga(&t.app).await;
	let chapter_id = first_chapter_id(&t.app, manga_id).await;
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["type"], "snapshot");

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_sse_data(&mut stream, &mut rest, "queued").await;
	assert_eq!(event["chapterId"], chapter_id);
}

#[tokio::test]
async fn update_sse_pushes_job_and_completed_summary() {
	let t = downloads_app(|_| {}).await;
	let manga_id = register_example_manga(&t.app).await;
	let (status, _) = request(&t.app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["type"], "snapshot");
	assert_eq!(snapshot["updateSummary"]["running"], false);

	let (status, _) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&t.app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_sse_data(&mut stream, &mut rest, "job").await;
	assert_eq!(event["mangaId"], manga_id);
	let completed = read_sse_kind(&mut stream, &mut rest, "updateComplete").await;
	assert_eq!(completed["data"]["running"], false);
	assert_eq!(completed["data"]["total"], 1);
	assert_eq!(completed["data"]["complete"], 1);

	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["type"], "snapshot");
	assert_eq!(snapshot["updateSummary"]["running"], false);
	assert_eq!(snapshot["updateSummary"]["total"], 1);
	assert_eq!(snapshot["updateSummary"]["complete"], 1);
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
	let source_blob = t.tmp.path().join("page-blobs").join(chapter_id.to_string()).join("0.png");
	std::fs::create_dir_all(source_blob.parent().expect("blob parent")).expect("blob dir");
	std::fs::write(&source_blob, b"materialized page").expect("materialized page");
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
	std::fs::create_dir_all(artifact.parent().expect("artifact parent")).expect("artifact dir");
	std::fs::write(&artifact, b"orphaned prior artifact").expect("prior artifact");
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_data(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["type"], "snapshot");

	let (status, body) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	sqlx::query("UPDATE download SET path = ? WHERE chapter_id = ? AND user_id = 1")
		.bind(default_rel_path(&meta))
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("attach old artifact path to active row");
	tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
		.await
		.expect("feed started")
		.expect("feed start signal");
	wait_download_status(&t.app, chapter_id, "downloading").await;
	let (status, _) = tokio::time::timeout(
		Duration::from_secs(10),
		request(&t.app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None),
	)
	.await
	.expect("active download deletion must not wait for blocked upstream I/O");
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(artifact.exists(), "the active worker must finish before its old artifact is removed");
	assert!(source_blob.exists(), "canceling an active attempt preserves source page blobs");
	release.notify_one();

	let (status, _) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [probe_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let finished = read_sse_data(&mut stream, &mut rest, "finished").await;
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

/// Registers the fixture search entry at `index` (register_example_manga always takes the first)
async fn register_manga_entry(app: &Router, index: usize) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][index]["key"].as_str().expect("manga key").to_string();
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

/// A mangaIds enqueue expands to the manga's untrashed chapters and skips local sources;
/// chapterIds merge in and active rows dedupe as usual.
#[tokio::test]
async fn enqueue_by_manga_skips_local_trashed_and_duplicates() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	let ids: Vec<i64> = body
		.as_array()
		.expect("chapters")
		.iter()
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect();
	assert_eq!(ids.len(), 8);
	let pool = connect_db(&t.db_path).await;
	// ids[0] sits in the trash; a second manga lives on the built-in local source
	sqlx::query("UPDATE chapter SET trashed_at = 1 WHERE id = ?")
		.bind(ids[0])
		.execute(&pool)
		.await
		.expect("trash chapter");
	let local_source: i64 = sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
		.fetch_one(&pool)
		.await
		.expect("local source");
	let local_manga: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key) VALUES (?, 'local-manga') RETURNING id")
		.bind(local_source)
		.fetch_one(&pool)
		.await
		.expect("local manga");
	sqlx::query("INSERT INTO chapter (manga_id, key) VALUES (?, 'local-chapter')")
		.bind(local_manga)
		.execute(&pool)
		.await
		.expect("local chapter");

	// stop the worker so the queued rows stay put for the duplicate check
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [ids[1]], "mangaIds": [manga_id, local_manga, 999]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let mut queued: Vec<i64> = body["queued"]
		.as_array()
		.expect("queued")
		.iter()
		.map(|v| v.as_i64().expect("chapter id"))
		.collect();
	queued.sort_unstable();
	let mut want: Vec<i64> = ids[1..].to_vec();
	want.sort_unstable();
	assert_eq!(queued, want);

	// a repeat over the same manga dedupes against the queued rows
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"mangaIds": [manga_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
}

/// Batch delete removes every download of the given manga through the shared removal routine:
/// rows go, download flags reset, artifacts leave the disk; other manga stay untouched.
#[tokio::test]
async fn delete_by_manga_removes_downloads_and_resets_flags() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let other_id = register_manga_entry(&app, 1).await;
	assert_ne!(manga_id, other_id);
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let first = body[0]["id"].as_i64().expect("first chapter id");
	let second = body[1]["id"].as_i64().expect("second chapter id");
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	for chapter_id in [first, second] {
		let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
		assert_eq!(status, StatusCode::OK);
		point_page_at_local(&pool, chapter_id, port).await;
	}

	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [first, second]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([first, second]));
	for chapter_id in [first, second] {
		wait_download_status(&app, chapter_id, "finished").await;
	}
	let artifacts: Vec<String> = sqlx::query_scalar("SELECT path FROM download WHERE chapter_id IN (?, ?) AND user_id = 1")
		.bind(first)
		.bind(second)
		.fetch_all(&pool)
		.await
		.expect("artifact paths");
	assert_eq!(artifacts.len(), 2);

	// a queued row of another manga must survive the batch delete
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{other_id}/chapters"), None).await;
	let other_chapter = body[0]["id"].as_i64().expect("other chapter id");
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [other_chapter]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([other_chapter]));

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/downloads/delete",
		Some(json!({"mangaIds": [manga_id, 999]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["deleted"], 2);

	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(body[0]["isDownloaded"], false);
	assert_eq!(body[1]["isDownloaded"], false);
	for path in &artifacts {
		assert!(
			!t.tmp.path().join("downloads").join(path).exists(),
			"artifact left behind: {path}"
		);
	}
	let remaining: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM download d JOIN chapter c ON c.id = d.chapter_id WHERE c.manga_id = ?",
	)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("remaining downloads");
	assert_eq!(remaining, 0);
	let surviving: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM download d JOIN chapter c ON c.id = d.chapter_id WHERE c.manga_id = ?",
	)
	.bind(other_id)
	.fetch_one(&pool)
	.await
	.expect("surviving downloads");
	assert_eq!(surviving, 1);

	// a repeat is a no-op
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/downloads/delete",
		Some(json!({"mangaIds": [manga_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["deleted"], 0);
}

/// A selection larger than SQLite's variable cap (32766 with the bundled build) is expanded in
/// chunks: the endpoint answers instead of failing with "too many SQL variables".
#[tokio::test]
async fn enqueue_manga_expansion_beyond_sql_variable_limit() {
	let t = downloads_app(|_| {}).await;
	let manga_ids: Vec<i64> = (900_001..=940_000).collect();
	let (status, body) = request(&t.app, "POST", "/api/v1/downloads", Some(json!({"mangaIds": manga_ids}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
}

/// Same chunked expansion for batch delete: a huge selection answers 0 instead of a 500.
#[tokio::test]
async fn delete_manga_expansion_beyond_sql_variable_limit() {
	let t = downloads_app(|_| {}).await;
	let manga_ids: Vec<i64> = (900_001..=940_000).collect();
	let (status, body) = request(
		&t.app,
		"POST",
		"/api/v1/downloads/delete",
		Some(json!({"mangaIds": manga_ids})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["deleted"], 0);
}
