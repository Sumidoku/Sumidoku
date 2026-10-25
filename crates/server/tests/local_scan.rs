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

fn bump_mtime(path: &std::path::Path) {
	let previous = std::fs::metadata(path).expect("archive metadata").modified().expect("archive mtime");
	let file = std::fs::OpenOptions::new().write(true).open(path).expect("open archive mtime");
	file.set_modified(previous + std::time::Duration::from_secs(2)).expect("advance archive mtime");
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

async fn entry_kind(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
	sqlx::query_scalar("SELECT local_entry_kind FROM manga WHERE key = ?")
		.bind(key)
		.fetch_one(pool)
		.await
		.expect("entry kind")
}

#[tokio::test]
async fn scan_indexes_then_trashes_restores_and_purges() {
	// phase A runs with both analysis passes off: scan-level indexing stays uninitialized
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
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

	// phase B: turn both passes on for the instance; the next scan's derived analysis fills metadata and
	// page rows with no reader involved
	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"analyzePages": true, "analyzeMetadata": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle(&t.app).await;
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
		"SELECT c.title FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter title");
	assert_eq!(chapter_title.as_deref(), Some("Chapter One Title"), "ComicInfo Title overrides the filename");
	let chapter_writers: String = sqlx::query_scalar(
		"SELECT c.scanlators FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
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
	// the previous index stays intact and keeps serving: nothing was trashed, nothing deleted
	let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE trashed_at IS NULL")
		.fetch_one(&pool)
		.await
		.expect("live count");
	assert_eq!(live, 2, "the failed scan leaves the previous index intact");
	let (_, _, _, alpha_trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha survives");
	assert_eq!(alpha_trashed, None, "Alpha stays live");
	let (_, _, _, beta_trashed, _) = manga_row(&pool, "Beta").await.expect("Beta survives");
	assert_eq!(beta_trashed, None, "Beta stays live");
	let source_id = local_source_id(&t.app).await;
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
	assert_eq!(keys, ["Alpha", "Beta"], "browse keeps serving the intact index: {keys:?}");
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
	// the new entry is absent from the index before the scan: the scan is what brings it in
	let pre_scan: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.key = 'deep/ch 3'",
	)
	.fetch_one(&pool)
	.await
	.expect("pre-scan count");
	assert_eq!(pre_scan, 0, "a file added after the last scan is not in the index before the next scan");
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
	let archive = t.tmp.path().join("local/Beta/ch 1.cbz");
	scan_and_settle(&t.app).await;
	write_cbz(&archive, &[("1.png", b"beta-1-1-edited")]);
	bump_mtime(&archive);
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["unchanged"], 2, "in-place edit invisible without the toggle: {scan_task}");
	let beta_before: i64 = sqlx::query_scalar("SELECT scan_mtime FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta mtime before toggle");

	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"forceModifiedTime": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle(&t.app).await;
	let beta_after: i64 = sqlx::query_scalar("SELECT scan_mtime FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta mtime after toggle");
	assert!(beta_after > beta_before, "the pending edit must be observed: {beta_before} -> {beta_after}");
	write_cbz(&archive, &[("1.png", b"beta-1-1-third")]);
	bump_mtime(&archive);
	scan_and_settle(&t.app).await;
	let beta_third: i64 = sqlx::query_scalar("SELECT scan_mtime FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta mtime after second edit");
	assert!(beta_third > beta_after, "the second edit must be observed: {beta_after} -> {beta_third}");
	pool.close().await;
}

/// POST /manga/{id}/analyze runs the pass for one manga only
#[tokio::test]
async fn manga_level_manual_analysis() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (_, _, initialized, _, _) = manga_row(&pool, "Beta").await.expect("Beta");
	assert!(!initialized);
	let (beta_id, _, _, _, _) = manga_row(&pool, "Beta").await.expect("Beta");

	// manga-level analysis fills exactly that manga, even with the instance switches off... no: the
	// switches gate the pass; re-enable them per instance, then analyze
	let source_id = local_source_id(&t.app).await;
	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"analyzePages": true, "analyzeMetadata": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/manga/{beta_id}/analyze"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["kind"], "local_analyze");
	assert_eq!(body["payload"]["mangaId"], beta_id);
	scan_and_settle_quiet(&t.app).await;

	let (_, title, initialized, _, _) = manga_row(&pool, "Beta").await.expect("Beta");
	assert!(initialized);
	assert_eq!(title, "Indexed Series");
	// Alpha was never analyzed by the manga-scoped task
	let (_, _, alpha_initialized, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert!(!alpha_initialized, "manga-scoped analysis must not spill to other manga");

	// a non-local manga is rejected
	let (status, _) = request(&t.app, "POST", "/api/v1/manga/9999/analyze", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
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

#[tokio::test]
async fn fresh_browse_returns_empty_then_the_kicked_scan_indexes() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;

	// a fresh instance serves no rows and kicks its first scan (manual priority) — the first response is
	// empty, the index arrives with the task
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0, "{body}");
	let (status, body) = request(&t.app, "GET", "/api/v1/tasks", None).await;
	assert_eq!(status, StatusCode::OK);
	let tasks = body.as_array().expect("tasks array");
	assert!(
		tasks.iter().any(|task| task["kind"] == "local_scan"),
		"the empty first response kicked the scan: {tasks:?}"
	);

	// once it settles, browse serves the index
	scan_and_settle_quiet(&t.app).await;
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
	assert_eq!(keys, ["Alpha", "Beta"], "{keys:?}");
	pool.close().await;
}

#[tokio::test]
async fn existing_rows_browse_before_the_scan_and_manual_scan_backfills() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	// Gamma exists on disk and in the index: a pre-upgrade install that was opened/shelved but never scanned
	write_page(&t.tmp.path().join("local/Gamma/ch 1"), "1.png", b"gamma-1-1");
	sqlx::query("INSERT INTO manga (source_id, key, title) VALUES (?, 'Gamma', 'Gamma')")
		.bind(source_id)
		.execute(&pool)
		.await
		.expect("seed pre-upgrade row");

	// browse shows the persisted subset (rows > 0, so no scan is kicked), not the on-disk Alpha/Beta
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
	assert_eq!(
		keys,
		["Gamma"],
		"an upgrade with existing rows shows the persisted subset until a scan: {keys:?}"
	);

	// the manual scan backfills the rest of the library
	scan_and_settle(&t.app).await;
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
	assert_eq!(keys, ["Alpha", "Beta", "Gamma"], "the manual scan backfills the index: {keys:?}");
	pool.close().await;
}

#[tokio::test]
async fn nested_chapters_under_an_excluded_subtree_stay_protected() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	write_page(&t.tmp.path().join("local/Alpha/deep/ch 3"), "1.png", b"alpha-3-1");
	scan_and_settle(&t.app).await;
	let deep_id: i64 = sqlx::query_scalar(
		"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.key = 'deep/ch 3'",
	)
	.fetch_one(&pool)
	.await
	.expect("deep chapter indexed");

	// the subtree becomes excluded: the rescan skips it for content, but its container lands in the
	// protection set — persisted chapters under it must not look vanished and get trashed
	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"exclusions": ["deep"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["chaptersTrashed"], 0, "{scan_task}");
	let trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM chapter WHERE id = ?")
		.bind(deep_id)
		.fetch_one(&pool)
		.await
		.expect("deep chapter");
	assert_eq!(trashed, None, "an excluded subtree's persisted chapters are protected, not trashed");
	let alpha_trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM manga WHERE key = 'Alpha'")
		.fetch_one(&pool)
		.await
		.expect("alpha");
	assert_eq!(alpha_trashed, None, "the manga row itself stays live");
	pool.close().await;
}

#[tokio::test]
async fn settings_reset_rescans_and_restores_the_excluded_rows() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	scan_and_settle(&t.app).await;

	// exclude Alpha through the instance settings: the write enqueues a rescan that hides and protects it
	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"exclusions": ["Alpha"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle(&t.app).await;
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
	assert!(!keys.contains(&"Alpha"), "the excluded row is hidden: {keys:?}");

	// reset clears the overrides and enqueues another rescan: the index converges back to the config rules
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/settings/reset"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle(&t.app).await;
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
	assert_eq!(keys, ["Alpha", "Beta"], "reset re-serves the previously excluded rows: {keys:?}");
	let alpha_trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM manga WHERE key = 'Alpha'")
		.fetch_one(&pool)
		.await
		.expect("alpha");
	assert_eq!(alpha_trashed, None, "exclusion protection never trashed the row");
	pool.close().await;
}

#[tokio::test]
async fn restore_untrashes_only_the_chapters_back_on_disk() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	scan_and_settle(&t.app).await;
	// only ch 1 returns: restore reconciles against disk instead of blanket-clearing the flags
	write_page(&t.tmp.path().join("local/Alpha/ch 1"), "1.png", b"alpha-1-1");
	let (status, _) = request(
		&t.app,
		"POST",
		&format!("/api/v1/local/trash/{alpha_id}/restore"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	scan_and_settle_quiet(&t.app).await;

	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert_eq!(trashed, None, "the manga untrashes once its directory is back");
	let ch1_trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM chapter WHERE manga_id = ? AND key = 'ch 1'")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("ch 1");
	assert_eq!(ch1_trashed, None, "ch 1 untrashes");
	let ch2_trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM chapter WHERE manga_id = ? AND key = 'ch 2'")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("ch 2");
	assert!(ch2_trashed.is_some(), "ch 2 stays trashed — its files are still missing");
	pool.close().await;
}

#[tokio::test]
async fn empty_trash_never_deletes_a_restored_manga() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Beta")).expect("remove Beta");
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");

	// Alpha's directory is back and the row is restored before the trash is emptied: each purge is
	// guarded by the row's current trash state, so a restored manga survives emptying
	write_page(&t.tmp.path().join("local/Alpha/ch 1"), "1.png", b"alpha-1-1");
	let (status, _) = request(
		&t.app,
		"POST",
		&format!("/api/v1/local/trash/{alpha_id}/restore"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&t.app, "DELETE", "/api/v1/local/trash", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle_quiet(&t.app).await;

	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha survives empty-trash");
	assert_eq!(trashed, None, "a restored manga is not purged by empty-trash");
	assert!(manga_row(&pool, "Beta").await.is_none(), "a still-trashed manga is purged");
	pool.close().await;
}

#[tokio::test]
async fn local_scan_clock_is_server_owned() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	sqlx::query("INSERT INTO setting (key, value) VALUES ('local.last_scan_at', '456')")
		.execute(&pool)
		.await
		.expect("seed scan clock");

	let (status, mut settings) = request(&t.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(settings.get("local.last_scan_at").is_none(), "{settings}");
	settings["local.last_scan_at"] = json!(123);
	let (status, body) = request(&t.app, "PUT", "/api/v1/settings", Some(settings)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, body) = request(&t.app, "PUT", "/api/v1/settings", Some(json!({"local.last_scan_at": 123}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let stored: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'local.last_scan_at'")
		.fetch_one(&pool)
		.await
		.expect("scan clock must survive both settings write forms");
	assert_eq!(stored, "456");
	pool.close().await;
}

#[tokio::test]
async fn local_scan_clock_stays_out_of_backups() {
	let t = scan_app().await;
	let pool = common::connect_db(&t.db_path).await;
	sqlx::query("INSERT INTO setting (key, value) VALUES ('local.last_scan_at', '456')")
		.execute(&pool)
		.await
		.expect("seed scan clock");

	let (status, mut doc) = request(&t.app, "GET", "/api/v1/backups/document", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(doc["settings"].get("local.last_scan_at").is_none(), "{doc}");
	doc["settings"]["local.last_scan_at"] = json!(123);
	let (status, body) = request(&t.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let stored: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'local.last_scan_at'")
		.fetch_one(&pool)
		.await
		.expect("scan clock must survive restoring an old backup");
	assert_eq!(stored, "456");
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

/// The analysis upsert must not clobber scheduler-owned fields: a title the user marked Never (or with a
/// pending next update) must keep that scheduling after the derived metadata pass, not re-enter scheduled
/// updates with the default Manga's Always/NULL
#[tokio::test]
async fn analysis_preserves_user_update_strategy_and_next_update() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (beta_id, _, _, _, _) = manga_row(&pool, "Beta").await.expect("Beta");
	// user-owned scheduling: Never (1) with a pending next update
	sqlx::query("UPDATE manga SET update_strategy = 1, next_update_time = 1234567890 WHERE id = ?")
		.bind(beta_id)
		.execute(&pool)
		.await
		.expect("set strategy");
	let source_id = local_source_id(&t.app).await;
	let (status, body) = request(
		&t.app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"analyzePages": true, "analyzeMetadata": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/manga/{beta_id}/analyze"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	scan_and_settle_quiet(&t.app).await;
	let (strategy, next_at): (i64, Option<i64>) = sqlx::query_as(
		"SELECT update_strategy, next_update_time FROM manga WHERE id = ?",
	)
	.bind(beta_id)
	.fetch_one(&pool)
	.await
	.expect("strategy row");
	assert_eq!(strategy, 1, "analysis must not reset a user's Never strategy");
	assert_eq!(next_at, Some(1234567890), "analysis must keep the pending next update");
	pool.close().await;
}

/// A restore racing a purge must not recreate the row through the ordinary insert path: the purge's cascade
/// has already removed progress and library links, so restore has to refuse instead of reporting success
/// on a fresh id
#[tokio::test]
async fn restore_after_purge_does_not_recreate_the_row() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	// trash Alpha by removing it from disk, then purge it
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	scan_and_settle(&t.app).await;
	let (status, _) = request(&t.app, "DELETE", &format!("/api/v1/local/trash/{alpha_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key = 'Alpha'")
		.fetch_one(&pool)
		.await
		.expect("alpha count");
	assert_eq!(count, 0, "purge removed the row");
	// restore must refuse (the row is gone) and must not re-create it
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{alpha_id}/restore"), None).await;
	assert!(!status.is_success(), "restore after purge must fail: {status} {body}");
	let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key = 'Alpha'")
		.fetch_one(&pool)
		.await
		.expect("alpha count");
	assert_eq!(count, 0, "restore never re-creates a purged row");
	pool.close().await;
}

/// Restore must validate the on-disk entry's shape, not just its existence: an arbitrary regular file in
/// the manga's place would otherwise manufacture a fake oneshot chapter that later scans then protect
#[tokio::test]
async fn restore_rejects_a_wrong_typed_replacement() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	scan_and_settle(&t.app).await;
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha trashed");
	assert!(trashed.is_some(), "Alpha sits in the trash");
	// replace the directory with a regular text file: not a manga dir, not an archive
	std::fs::write(t.tmp.path().join("local/Alpha"), b"not a manga").expect("write impostor");
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{alpha_id}/restore"), None).await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "gone", "{body}");
	// no pseudo-chapter materialized and restored under the fake entry (the pre-existing trashed chapter
	// rows stay trashed; restore must not have re-created any live one)
	let count: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.trashed_at IS NULL",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter count");
	assert_eq!(count, 0, "no live pseudo-chapter from a wrong-typed restore");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha still trashed");
	assert!(trashed.is_some(), "the row stays trashed after the refused restore");
	pool.close().await;
}

#[tokio::test]
async fn restore_rejects_an_archive_suffixed_file_replacing_a_directory() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	write_page(&t.tmp.path().join("local/Alpha.cbz/ch 1"), "1.png", b"alpha-1-1");
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha.cbz").await.expect("Alpha.cbz");
	assert_eq!(entry_kind(&pool, "Alpha.cbz").await.as_deref(), Some("directory"));
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha.cbz")).expect("remove Alpha.cbz");
	scan_and_settle(&t.app).await;
	std::fs::write(t.tmp.path().join("local/Alpha.cbz"), b"not a directory").expect("write impostor");

	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{alpha_id}/restore"), None).await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "gone", "{body}");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha.cbz").await.expect("Alpha.cbz still trashed");
	assert!(trashed.is_some());
	let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter WHERE manga_id = ? AND trashed_at IS NULL")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("live chapter count");
	assert_eq!(live, 0, "no pseudo-oneshot may be restored for the original directory");
	pool.close().await;
}

#[tokio::test]
async fn restore_requires_an_archive_and_restores_a_real_archive() {
	let t = scan_app_with(|config| {
		config.local_oneshots_dir = Some("oneshots".into());
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	let archive = t.tmp.path().join("local/oneshots/Solo.cbz");
	write_cbz(&archive, &[("1.png", b"solo-1")]);
	scan_and_settle(&t.app).await;
	let (solo_id, _, _, _, _) = manga_row(&pool, "oneshots/Solo.cbz").await.expect("Solo");
	assert_eq!(entry_kind(&pool, "oneshots/Solo.cbz").await.as_deref(), Some("archive"));
	std::fs::remove_file(&archive).expect("remove Solo");
	scan_and_settle(&t.app).await;
	std::fs::create_dir_all(&archive).expect("replace Solo with a directory");

	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{solo_id}/restore"), None).await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "gone", "{body}");
	let (_, _, _, trashed, _) = manga_row(&pool, "oneshots/Solo.cbz").await.expect("Solo still trashed");
	assert!(trashed.is_some());

	std::fs::remove_dir_all(&archive).expect("remove directory impostor");
	write_cbz(&archive, &[("1.png", b"solo-1-back")]);
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{solo_id}/restore"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (_, _, _, trashed, _) = manga_row(&pool, "oneshots/Solo.cbz").await.expect("Solo restored");
	assert_eq!(trashed, None);
	let live_chapters: Vec<String> = sqlx::query_scalar("SELECT key FROM chapter WHERE manga_id = ? AND trashed_at IS NULL")
		.bind(solo_id)
		.fetch_all(&pool)
		.await
		.expect("live chapters");
	assert_eq!(live_chapters, vec!["Solo.cbz".to_string()]);
	pool.close().await;
}

#[tokio::test]
async fn restore_with_unknown_entry_kind_waits_for_a_scan() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	scan_and_settle(&t.app).await;
	sqlx::query("UPDATE manga SET local_entry_kind = NULL WHERE id = ?")
		.bind(alpha_id)
		.execute(&pool)
		.await
		.expect("seed unknown history");
	write_page(&t.tmp.path().join("local/Alpha/ch 1"), "1.png", b"alpha-1-1-back");

	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{alpha_id}/restore"), None).await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "gone", "{body}");
	assert_eq!(entry_kind(&pool, "Alpha").await, None, "restore must not infer the missing kind");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha still trashed");
	assert!(trashed.is_some());

	scan_and_settle(&t.app).await;
	assert_eq!(entry_kind(&pool, "Alpha").await.as_deref(), Some("directory"));
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha restored by scan");
	assert_eq!(trashed, None);
	pool.close().await;
}

#[tokio::test]
async fn scans_backfill_missing_entry_kind_on_unchanged_fast_paths() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	assert_eq!(entry_kind(&pool, "Alpha").await.as_deref(), Some("directory"));
	sqlx::query("UPDATE manga SET local_entry_kind = NULL, initialized = TRUE WHERE id = ?")
		.bind(alpha_id)
		.execute(&pool)
		.await
		.expect("seed unknown kind");

	let tasks = scan_and_settle(&t.app).await;
	let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
	assert_eq!(scan_task["result"]["unchanged"], 2, "{scan_task}");
	let full: (Option<String>, bool) = sqlx::query_as("SELECT local_entry_kind, initialized FROM manga WHERE id = ?")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("full scan backfill");
	assert_eq!(full, (Some("directory".to_string()), true), "the fast path must not reconcile metadata");

	sqlx::query("UPDATE manga SET local_entry_kind = NULL WHERE id = ?")
		.bind(alpha_id)
		.execute(&pool)
		.await
		.expect("reset unknown kind");
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{alpha_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entry: (Option<String>, bool) = sqlx::query_as("SELECT local_entry_kind, initialized FROM manga WHERE id = ?")
		.bind(alpha_id)
		.fetch_one(&pool)
		.await
		.expect("entry scan backfill");
	assert_eq!(entry, (Some("directory".to_string()), true), "the entry fast path must not reconcile metadata");
	pool.close().await;
}

/// The batch progress endpoint must apply the trashed-chapter guard too, or a batch update would write
/// progress/history for a chapter whose files are gone even though the single-chapter and read paths refuse
#[tokio::test]
async fn batch_progress_skips_trashed_chapters() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let ch1: i64 = sqlx::query_scalar(
		"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Beta' AND c.key = 'ch 1.cbz'",
	)
	.fetch_one(&pool)
	.await
	.expect("chapter id");
	// trash Beta/ch 1 by removing it from disk and rescanning
	std::fs::remove_file(t.tmp.path().join("local/Beta/ch 1.cbz")).expect("remove ch1");
	scan_and_settle(&t.app).await;
	let trashed: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM chapter WHERE id = ?")
		.bind(ch1)
		.fetch_one(&pool)
		.await
		.expect("trashed flag");
	assert!(trashed.is_some(), "chapter sits in the trash");
	let (status, body) = request(
		&t.app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": [ch1], "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["updated"], 0, "a trashed chapter writes no batch progress: {body}");
	let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE chapter_id = ?")
		.bind(ch1)
		.fetch_one(&pool)
		.await
		.expect("progress count");
	assert_eq!(count, 0, "no progress row appears for a trashed chapter");
	pool.close().await;
}

/// Restore clears the manga's trash flag before the entry rescan; when the directory came back with the
/// same mtime (a rename away and back), the unchanged fast path would skip chapter reconciliation and leave
/// every chapter trashed. Restore must force chapter reconciliation and untrash the chapters with the manga
#[tokio::test]
async fn restore_with_unchanged_mtime_still_untrashes_chapters() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	scan_and_settle(&t.app).await;
	let (alpha_id, _, _, _, _) = manga_row(&pool, "Alpha").await.expect("Alpha");
	// move the directory away and back with rename — the same inode keeps its mtime, so the entry-level
	// rescan would see an unchanged subtree and skip chapter reconciliation without the force
	let away = t.tmp.path().join("away");
	std::fs::create_dir_all(&away).expect("away dir");
	std::fs::rename(t.tmp.path().join("local/Alpha"), away.join("Alpha")).expect("move away");
	scan_and_settle(&t.app).await;
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha trashed");
	assert!(trashed.is_some(), "Alpha sits in the trash after the directory left");
	std::fs::rename(away.join("Alpha"), t.tmp.path().join("local/Alpha")).expect("move back");
	let (status, body) = request(&t.app, "POST", &format!("/api/v1/local/trash/{alpha_id}/restore"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	// the chapters must come out of the trash with the manga, not stay trashed behind a cleared manga flag
	let live: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.key = 'Alpha' AND c.trashed_at IS NULL",
	)
	.fetch_one(&pool)
	.await
	.expect("live chapters");
	assert_eq!(live, 2, "all chapters untrashed with the restored manga");
	let (_, _, _, trashed, _) = manga_row(&pool, "Alpha").await.expect("Alpha restored");
	assert_eq!(trashed, None);
	pool.close().await;
}

/// The first-scan kick answers a genuinely empty instance (no rows at all), not an all-trashed one: once
/// every row sits in the trash, browsing must serve an empty visible list without re-triggering a full
/// filesystem scan on every call
#[tokio::test]
async fn an_all_trashed_instance_does_not_rekick_a_scan_on_browse() {
	let t = scan_app_with(|config| {
		config.local_analyze_pages = false;
		config.local_analyze_metadata = false;
	})
	.await;
	let pool = common::connect_db(&t.db_path).await;
	let source_id = local_source_id(&t.app).await;
	scan_and_settle(&t.app).await;
	// trash everything: remove both manga directories and rescan
	std::fs::remove_dir_all(t.tmp.path().join("local/Alpha")).expect("remove Alpha");
	std::fs::remove_dir_all(t.tmp.path().join("local/Beta")).expect("remove Beta");
	scan_and_settle(&t.app).await;
	let trashed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE trashed_at IS NOT NULL")
		.fetch_one(&pool)
		.await
		.expect("trashed count");
	assert_eq!(trashed, 2, "every row is in the trash");
	// browse serves the empty visible list but must not kick another scan
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0, "{body}");
	let (status, body) = request(&t.app, "GET", "/api/v1/tasks", None).await;
	assert_eq!(status, StatusCode::OK);
	let tasks = body.as_array().expect("tasks array");
	assert!(
		!tasks.iter().any(|task| {
			task["kind"] == "local_scan" && (task["status"] == "pending" || task["status"] == "running")
		}),
		"an all-trashed browse must not re-kick the scan: {tasks:?}"
	);
	pool.close().await;
}

/// An empty instance (no manga at all) scans and settles cleanly: the inputs checkpoint still runs after
/// the persisted pass (there are no rows to checkpoint against), so the handler converges without looping
#[tokio::test]
async fn empty_instance_scan_settles_cleanly() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let empty_dir = tmp.path().join("empty");
	std::fs::create_dir_all(&empty_dir).expect("empty dir");
	let mut config = base_config(&tmp);
	config.local_dirs = Some(vec![empty_dir]);
	config.local_analyze_pages = false;
	config.local_analyze_metadata = false;
	let app = build_app(config).await.expect("build app");
	let (status, body) = request(&app, "POST", "/api/v1/local/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let started = std::time::Instant::now();
	loop {
		let (status, body) = request(&app, "GET", "/api/v1/tasks", None).await;
		assert_eq!(status, StatusCode::OK);
		let tasks = body.as_array().expect("tasks array");
		if !tasks.is_empty()
			&& tasks.iter().all(|task| task["status"] != "pending" && task["status"] != "running")
		{
			let scan_task = tasks.iter().find(|task| task["kind"] == "local_scan").expect("scan task");
			assert_eq!(scan_task["status"], "done", "{scan_task}");
			assert_eq!(scan_task["result"]["indexed"], 0, "an empty root indexes nothing: {scan_task}");
			break;
		}
		assert!(
			started.elapsed() < std::time::Duration::from_secs(15),
			"tasks never settled: {body}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
	}
}

/// A manual scan requested while the browse-kicked first scan is live must not be acknowledged and lost:
/// the one-outstanding-execution contract folds the newest payload into the running task, and the executed
/// scan carries the manual trigger's refreshMetadata = true
#[tokio::test]
async fn live_payload_moves_to_the_newest_trigger() {
	let t = scan_app().await;
	let source_id = local_source_id(&t.app).await;
	// browse kicks the first scan with refresh_metadata = false
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0, "{body}");
	// an immediate manual scan lands on the same live task; its payload must replace the kick's
	let (status, _) = request(&t.app, "POST", "/api/v1/local/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	scan_and_settle_quiet(&t.app).await;
	let (status, body) = request(&t.app, "GET", "/api/v1/tasks", None).await;
	assert_eq!(status, StatusCode::OK);
	let tasks = body.as_array().expect("tasks array");
	let scan_tasks: Vec<&Value> = tasks.iter().filter(|task| task["kind"] == "local_scan").collect();
	let last = scan_tasks.last().expect("scan task");
	assert_eq!(last["status"], "done", "{last}");
	assert_eq!(
		last["payload"]["refreshMetadata"], true,
		"the executed scan carried the newest (manual) trigger's payload: {last}"
	);
}
