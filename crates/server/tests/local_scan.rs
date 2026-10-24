//! Scheduled local scan + trash bin (#256 phase 2): the scan indexes discovered manga (cheap, initialized=0),
//! trashes vanished rows instead of deleting them (failed-directory protection, empty-scan guard, root
//! availability gate, exclusion protection), restores on reappearance, and the derived analysis pass fills
//! metadata and page rows. Browse reads the index only, never the filesystem.
mod common;

use axum::http::StatusCode;
use common::{TestApp, base_config, request, source_by_key};
use serde_json::{Value, json};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;

fn write_page(dir: &std::path::Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

fn write_cbz(path: &std::path::Path, entries: &[(&str, &[u8])]) {
	std::fs::create_dir_all(path.parent().expect("cbz parent")).expect("mkdir cbz parent");
	let file = std::fs::File::create(path).expect("create cbz");
	let mut writer = zip::ZipWriter::new(file);
	for (name, bytes) in entries {
		writer
			.start_file(*name, zip::write::SimpleFileOptions::default())
			.expect("start entry");
		std::io::Write::write_all(&mut writer, bytes).expect("write entry");
	}
	writer.finish().expect("finish cbz");
}

const COMIC_INFO_SERIES_ONLY: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Indexed Series</Series>
</ComicInfo>"#;

const COMIC_INFO_CHAPTER: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>Chapter One Title</Title>
  <Writer>Chapter Writer</Writer>
  <Genre>ChapterTag</Genre>
</ComicInfo>"#;

fn seed_tree(root: &std::path::Path) {
	write_page(&root.join("Alpha/ch 1"), "1.png", b"alpha-1-1");
	write_page(&root.join("Alpha/ch 2"), "1.png", b"alpha-2-1");
	// Beta's chain hit (ch 2, the newest first) carries only the series title: the manga authors/tags must
	// then aggregate from ch 1's in-archive ComicInfo
	write_cbz(
		&root.join("Beta/ch 1.cbz"),
		&[("1.png", b"beta-1-1"), ("ComicInfo.xml", COMIC_INFO_CHAPTER.as_bytes())],
	);
	write_cbz(
		&root.join("Beta/ch 2.cbz"),
		&[("1.png", b"beta-2-1"), ("ComicInfo.xml", COMIC_INFO_SERIES_ONLY.as_bytes())],
	);
}

async fn scan_app() -> TestApp {
	scan_app_with(|_| {}).await
}

async fn scan_app_with(configure: impl FnOnce(&mut Config)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_tree(&tmp.path().join("local"));
	let mut config = base_config(&tmp);
	config.local_dirs = Some(vec![tmp.path().join("local")]);
	configure(&mut config);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn local_source_id(app: &axum::Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "local.local")["id"]
		.as_i64()
		.expect("local source id")
}

/// POST the scan and wait until every queued task (scan + derived analysis) reaches a terminal state
async fn scan_and_settle(app: &axum::Router) -> Vec<Value> {
	let (status, body) = request(app, "POST", "/api/v1/local/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let started = std::time::Instant::now();
	loop {
		let (status, body) = request(app, "GET", "/api/v1/tasks", None).await;
		assert_eq!(status, StatusCode::OK);
		let tasks = body.as_array().expect("tasks array");
		let live = tasks
			.iter()
			.filter(|task| task["status"] == "pending" || task["status"] == "running")
			.count();
		if live == 0 {
			return tasks.clone();
		}
		assert!(started.elapsed() < std::time::Duration::from_secs(15), "tasks never settled: {body}");
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
	}
}

async fn manga_row(pool: &sqlx::SqlitePool, key: &str) -> Option<(i64, String, bool, Option<i64>, Option<i64>)> {
	sqlx::query_as("SELECT id, title, initialized, trashed_at, scan_mtime FROM manga WHERE key = ?")
		.bind(key)
		.fetch_optional(pool)
		.await
		.expect("manga row")
}

#[tokio::test]
async fn scan_indexes_then_trashes_restores_and_purges() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;

	// scan indexes everything discovered, cheap: initialized = 0, nothing in the library
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["status"], "done", "{scan_task}");
	let result = &scan_task["result"];
	assert_eq!(result["indexed"], 2, "{result}");
	assert_eq!(result["scanned"], 2, "{result}");
	let (alpha_id, _, initialized, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha indexed");
	assert!(!initialized, "scan-level rows start uninitialized");
	assert_eq!(trashed, None);
	let in_library: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = ? AND in_library = 1")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("library count");
	assert_eq!(in_library, 0, "indexing never shelves");

	// the derived analysis pass filled metadata and page rows with no reader involved
	let (alpha_id, title, initialized, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert!(initialized, "analysis ran: {title}");
	let beta_title: String = sqlx::query_scalar("SELECT title FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta title");
	assert_eq!(beta_title, "Indexed Series", "chapter ComicInfo feeds the manga title");
	let beta_authors: String = sqlx::query_scalar("SELECT authors FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta authors");
	assert_eq!(beta_authors, r#"["Chapter Writer"]"#, "manga authors aggregate from chapters");
	let beta_tags: String = sqlx::query_scalar("SELECT tags FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta tags");
	assert_eq!(beta_tags, r#"["ChapterTag"]"#, "manga tags aggregate from chapters");
	let chapter_title: Option<String> = sqlx::query_scalar(
		"SELECT title FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter title");
	assert_eq!(chapter_title.as_deref(), Some("Chapter One Title"), "ComicInfo Title overrides the filename");
	let chapter_writers: String = sqlx::query_scalar(
		"SELECT scanlators FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter writers");
	assert_eq!(chapter_writers, r#"["Chapter Writer"]"#, "writers land in scanlators");
	let page_count: i64 = sqlx::query_scalar(
		"SELECT page_count FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.key = 'ch 1'",
	)
	.fetch_one(&pool)
	.await
	.expect("page count");
	assert_eq!(page_count, 1, "page analysis filled the count without a reader");

	// browse reads the index: Alpha/Beta listed with their internal ids, #recycle hidden
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries");
	let keys: Vec<&str> = entries.iter().filter_map(|e| e["key"].as_str()).collect();
	assert_eq!(keys, ["Alpha", "Beta"], "{keys:?}");
	assert!(entries.iter().all(|e| e["id"].is_i64()), "index rows carry their ids");

	// delete the folder -> scan trashes, never deletes
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["trashed"], 1, "{scan_task}");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha still persisted");
	assert!(trashed.is_some(), "trashed, not gone");

	// trash API: listed, restore refuses while the directory is missing
	let (status, body) = request(&t.app, "GET", "/api/v1/local/trash", None).await;
	assert_eq!(status, StatusCode::OK);
	let trash = body.as_array().expect("trash array");
	assert_eq!(trash.len(), 1);
	assert_eq!(trash[0]["manga"]["id"], alpha_id);
	assert_eq!(trash[0]["trashedChapters"], 2);
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/local/trash/{alpha_id}/restore"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "gone");

	// the directory reappears -> the next scan restores in place
	write_page(&t.tmp.path().join("local/Alpha/ch 1"), "1.png", b"alpha-1-1");
	write_page(&t.tmp.path().join("local/Alpha/ch 2"), "1.png", b"alpha-2-1");
	scan_and_settle(&t.app).await;
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert_eq!(trashed, None, "reappearance restores without the trash API");
	let (status, body) = request(&t.app, "GET", "/api/v1/local/trash", None).await;
	assert_eq!(body.as_array().expect("trash").len(), 0, "{status}");

	// delete again -> trash -> purge -> gone for good
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha again");
	scan_and_settle(&t.app).await;
	let (status, _) = request(&t.app, "DELETE", &format!("/api/v1/local/trash/{alpha_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(manga_row(&pool, "Alpha").await.is_none(), "purged");

	// a live row cannot be purged through the trash endpoint
	let (beta_id, _, _, _, _) = manga_row(&pool, "Beta").await.expect("Beta");
	let (status, _) = request(&t.app, "DELETE", &format!("/api/v1/local/trash/{beta_id}"), None).await;
	assert_eq!(status, StatusCode::CONFLICT);
	pool.close().await;
}

#[tokio::test]
async fn manual_restore_brings_back_a_trashed_manga() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	scan_and_settle(&t.app).await;
	write_page(&t.tmp.path().join("local/Alpha/ch 1"), "1.png", b"alpha-1-1");
	let (status, _) = request(
		&t.app,
		"POST",
		&format!("/api/v1/local/trash/{alpha_id}/restore"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert_eq!(trashed, None);
	pool.close().await;
}

#[tokio::test]
async fn a_regular_file_in_the_manga_dirs_place_never_trashes() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	// replace the manga directory with a regular file: the entry is not a directory, so discovery skips it —
	// but it is not *gone*, and the diff must protect the row instead of trashing it
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	std::fs::write(t.tmp.path().join("local/Alpha"), b"not a directory").expect("write placeholder file");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["status"], "done", "{scan_task}");
	assert_eq!(scan_task["result"]["trashed"], 0, "{scan_task}");
	assert_eq!(scan_task["result"]["protected"], 1, "{scan_task}");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha still persisted");
	assert_eq!(trashed, None);
	pool.close().await;
}

#[tokio::test]
async fn a_missing_root_fails_the_scan_and_trashes_nothing() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	std::fs::remove_dir_all(t.tmp.path().join("local")).expect("remove root");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["status"], "failed", "{scan_task}");
	assert!(scan_task["error"].as_str().is_some_and(|e| e.contains("root")), "{scan_task}");
	let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE trashed_at IS NOT NULL")
		.fetch_one(&pool)
		.await
		.expect("trash count");
	assert_eq!(count, 0, "an unavailable root trashes nothing");
	pool.close().await;
}

#[tokio::test]
async fn exclusions_hide_from_browse_and_protect_rows() {
	let t = scan_app_with(|config| config.local_scan_exclusions = vec!["#recycle".to_string()]).await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	write_page(&t.tmp.path().join("local/#recycle/Trashed/ch 1"), "1.png", b"recycle-1-1");
	scan_and_settle(&t.app).await;
	assert!(manga_row(&pool, "#recycle").await.is_none(), "excluded subtrees never index");
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<&str> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.filter_map(|e| e["key"].as_str())
		.collect();
	assert!(!keys.iter().any(|key| key.contains("#recycle")), "browse hides exclusions too: {keys:?}");

	// a row indexed before the exclusion appeared is protected, not trashed
	let config_row: i64 = sqlx::query_scalar(
		"INSERT INTO manga (source_id, key, title) VALUES (?, '#recycle/Trashed', 'Trashed') RETURNING id",
	)
	.bind(source_id)
	.fetch_one(&pool)
	.await
	.expect("seed pre-exclusion row");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["trashed"], 0, "{scan_task}");
	assert!(scan_task["result"]["protected"].as_i64().expect("protected") >= 1, "{scan_task}");
	let trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM manga WHERE id = ?")
		.bind(config_row)
		.fetch_one(&pool)
		.await
		.expect("row");
	assert_eq!(trashed, None);
	pool.close().await;
}

#[tokio::test]
async fn unchanged_manga_skip_reconciliation_and_mtime_modes() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["unchanged"], 2, "second scan is all fast-path: {scan_task}");
	assert_eq!(scan_task["result"]["reconciled"], 0, "{scan_task}");

	// a deep addition bumps a descendant directory's mtime: caught without forceModifiedTime
	write_page(&t.tmp.path().join("local/Alpha/deep/ch 3"), "1.png", b"alpha-3-1");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["reconciled"], 1, "{scan_task}");
	let chapter: Option<(String,)> =
		sqlx::query_as("SELECT c.key FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.key = 'deep/ch 3'")
			.fetch_optional(&pool)
			.await
			.expect("deep chapter");
	assert!(chapter.is_some(), "nested chapter reconciled");
	pool.close().await;
}

#[tokio::test]
async fn force_modified_time_catches_in_place_archive_edits() {
	// force off (default): rewriting a chapter archive in place bumps no directory mtime, so the manga is
	// skipped; with the instance toggle on, the file mtime joins the effective mtime and it reconciles
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	scan_and_settle(&t.app).await;
	write_cbz(&t.tmp.path().join("local/Beta/ch 1.cbz"), &[("1.png", b"beta-1-1-edited")]);
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["unchanged"], 2, "in-place edit invisible without the toggle: {scan_task}");

	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"forceModifiedTime": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	// the settings write enqueued a rescan (kmrs's check_library_should_rescan)
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["reconciled"], 1, "the pending in-place edit reconciles: {scan_task}");
	write_cbz(&t.tmp.path().join("local/Beta/ch 1.cbz"), &[("1.png", b"beta-1-1-third")]);
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["reconciled"], 1, "in-place edit caught with the toggle on: {scan_task}");
	pool.close().await;
}

#[tokio::test]
async fn analysis_switches_gate_the_two_passes() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (_, _, initialized, _, _) = manga_row(&pool, "Beta").await.expect("Beta");
	assert!(!initialized, "metadata analysis off: no chain runs");
	let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("title");
	assert_eq!(title, "Beta", "leaf-name title stands while analysis is off");
	let page_count: i64 = sqlx::query_scalar(
		"SELECT page_count FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("page count");
	assert_eq!(page_count, 0, "page analysis off: counts stay zero");

	// the lazy reader path still works with both passes off
	let chapter_id: i64 = sqlx::query_scalar(
		"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter id");
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body.as_array().expect("pages array").len(), 1);
	pool.close().await;
}

#[tokio::test]
async fn empty_trash_purges_everything_trashed() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Beta")).expect("remove Beta");
	scan_and_settle(&t.app).await;
	let trashed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE trashed_at IS NOT NULL")
		.fetch_one(&pool)
		.await
		.expect("trashed");
	assert_eq!(trashed, 2);
	let (status, body) = request(&t.app, "DELETE", "/api/v1/local/trash", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let tasks = scan_and_settle_quiet(&t.app).await;
	let empty = tasks.iter().find(|task| task["kind"] == "empty_trash").expect("empty task");
	assert_eq!(empty["status"], "done", "{empty}");
	assert_eq!(empty["result"]["purged"], 2, "{empty}");
	let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga")
		.fetch_one(&pool)
		.await
		.expect("remaining");
	assert_eq!(remaining, 0);
	pool.close().await;
}

/// Waits for tasks already in the queue (used after empty_trash, which enqueues its own)
async fn scan_and_settle_quiet(app: &axum::Router) -> Vec<Value> {
	let started = std::time::Instant::now();
	loop {
		let (status, body) = request(app, "GET", "/api/v1/tasks", None).await;
		assert_eq!(status, StatusCode::OK);
		let tasks = body.as_array().expect("tasks array");
		if !tasks.is_empty()
			&& tasks.iter().all(|task| task["status"] != "pending" && task["status"] != "running")
		{
			return tasks.clone();
		}
		assert!(started.elapsed() < std::time::Duration::from_secs(15), "tasks never settled: {body}");
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
	}
}
