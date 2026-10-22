//! Multiple local directories for the built-in local source: config layer (SUMIDOKU_LOCAL_DIRS >
//! SUMIDOKU_LOCAL_DIR > <config_dir>/local) with the runtime local.dirs setting override / browse
//! unions the roots and the first directory wins a mirrored relative path / directory-indexed keys
//! (`N:<path>`, bare = directory 0) with old bare-key rows still resolving / settings validation
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, request, request_full};
use serde_json::json;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;

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

fn seed_dir_a(root: &std::path::Path) {
	write_page(&root.join("Alpha/ch 1"), "P1.png", b"alpha-1-p1");
	write_page(&root.join("Alpha/ch 1"), "p2.png", b"alpha-1-p2");
	write_page(&root.join("Shared/ch 1"), "1.png", b"shared-from-a");
}

fn seed_dir_b(root: &std::path::Path) {
	write_cbz(
		&root.join("Beta/ch 1.cbz"),
		&[("a.png", b"beta-1-a"), ("b.png", b"beta-1-b")],
	);
	write_page(&root.join("Beta/ch 2"), "001.png", b"beta-2-1");
	write_page(&root.join("Shared/ch 1"), "1.png", b"shared-from-b");
	write_cbz(&root.join("pub/series3/ch 9.cbz"), &[("x.png", b"s3-9-x")]);
}

/// The two roots live outside the config dir so their tempdirs must stay bound for the app's lifetime
struct TwoDirApp {
	app: TestApp,
	dir_a: tempfile::TempDir,
	dir_b: tempfile::TempDir,
}

/// Boots with the config layer pointing at directory A only (the single-dir form); tests opt into the
/// multi-root view through the local.dirs setting
async fn two_dir_app() -> TwoDirApp {
	let dir_a = tempfile::tempdir().expect("dir a");
	let dir_b = tempfile::tempdir().expect("dir b");
	seed_dir_a(dir_a.path());
	seed_dir_b(dir_b.path());
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dirs = Some(vec![dir_a.path().to_path_buf()]);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TwoDirApp {
		app: TestApp { app, tmp, db_path },
		dir_a,
		dir_b,
	}
}

/// Points the runtime override at both roots; paths go in as strings exactly like a settings-page save
async fn enable_both_dirs(t: &TwoDirApp) {
	let (status, body) = request(
		&t.app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"local.dirs": [
			t.dir_a.path().to_string_lossy(),
			t.dir_b.path().to_string_lossy(),
		]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["readOnly"]["localDirs"], json!([
		t.dir_a.path().to_string_lossy(),
		t.dir_b.path().to_string_lossy(),
	]));
}

async fn local_source_id(app: &Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	common::source_by_key(&body, "local.local")["id"]
		.as_i64()
		.expect("local source id")
}

async fn browse_keys(app: &Router, source_id: i64) -> Vec<serde_json::Value> {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["entries"].as_array().expect("entries array").clone()
}

async fn register_local(app: &Router, manga_key: &str) -> i64 {
	let source_id = local_source_id(app).await;
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "register {manga_key}: {body}");
	body["id"].as_i64().expect("manga id")
}

async fn register_status(app: &Router, manga_key: &str) -> StatusCode {
	let source_id = local_source_id(app).await;
	let (status, _) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	status
}

async fn chapter_keys(app: &Router, manga_id: i64) -> Vec<(i64, String)> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters array")
		.iter()
		.map(|c| {
			(
				c["id"].as_i64().expect("chapter id"),
				c["key"].as_str().expect("chapter key").to_string(),
			)
		})
		.collect()
}

#[tokio::test]
async fn browse_unions_directories_and_first_directory_wins() {
	let t = two_dir_app().await;
	enable_both_dirs(&t).await;
	let source_id = local_source_id(&t.app.app).await;

	let entries = browse_keys(&t.app.app, source_id).await;
	let mut keys: Vec<&str> = entries.iter().map(|e| e["key"].as_str().unwrap()).collect();
	keys.sort();
	assert_eq!(keys, ["1:Beta", "1:pub/series3", "Alpha", "Shared"]);

	// the mirrored relative path lists once, from the first directory (bare key form)
	let shared: Vec<&serde_json::Value> = entries.iter().filter(|e| e["key"] == "Shared").collect();
	assert_eq!(shared.len(), 1, "mirror must be deduped to one listing: {entries:?}");
	assert!(!keys.contains(&"1:Shared"), "the shadowed mirror never lists: {keys:?}");

	// titles come from the relative path's leaf, never carrying the directory prefix
	let beta = entries.iter().find(|e| e["key"] == "1:Beta").expect("1:Beta entry");
	assert_eq!(beta["title"], "Beta");
	let nested = entries.iter().find(|e| e["key"] == "1:pub/series3").expect("1:pub/series3 entry");
	assert_eq!(nested["title"], "series3");
}

#[tokio::test]
async fn mirrored_relative_path_registers_from_first_directory() {
	let t = two_dir_app().await;
	enable_both_dirs(&t).await;

	let shared_id = register_local(&t.app.app, "Shared").await;
	let chapters = chapter_keys(&t.app.app, shared_id).await;
	let (chapter_id, _) = chapters.iter().find(|(_, k)| k == "ch 1").expect("ch 1");
	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"shared-from-a", "the bare key resolves to directory 0");

	// the shadowed copy stays addressable by its numbered key (deep links and restores stay total)
	let mirror_id = register_local(&t.app.app, "1:Shared").await;
	assert_ne!(mirror_id, shared_id);
	let chapters = chapter_keys(&t.app.app, mirror_id).await;
	let (chapter_id, _) = chapters.iter().find(|(_, k)| k == "ch 1").expect("ch 1");
	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"shared-from-b");
}

#[tokio::test]
async fn numbered_directory_key_reads_pages_and_covers() {
	let t = two_dir_app().await;
	enable_both_dirs(&t).await;

	let beta_id = register_local(&t.app.app, "1:Beta").await;
	let chapters = chapter_keys(&t.app.app, beta_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k)| k.as_str()).collect();
	assert_eq!(keys, ["ch 2", "ch 1.cbz"], "descending by parsed chapter number");

	let (chapter_id, _) = chapters.iter().find(|(_, k)| k == "ch 1.cbz").expect("ch 1.cbz");
	let (status, body) = request(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["a.png", "b.png"]);

	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"beta-1-a");

	// cover fallback: the last chapter in the descending list (ch 1.cbz), its first page
	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/manga/{beta_id}/cover"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"beta-1-a");
}

#[tokio::test]
async fn old_bare_key_rows_still_resolve_after_adding_directories() {
	let t = two_dir_app().await;
	// persisted before the second directory exists: the pre-multi-dir rows are all bare keys
	let alpha_id = register_local(&t.app.app, "Alpha").await;
	let source_id = local_source_id(&t.app.app).await;

	enable_both_dirs(&t).await;
	let entries = browse_keys(&t.app.app, source_id).await;
	assert!(
		entries.iter().any(|e| e["key"] == "1:Beta"),
		"the override widens the union without a restart: {entries:?}"
	);
	let alpha = entries.iter().find(|e| e["key"] == "Alpha").expect("Alpha entry");
	assert_eq!(alpha["id"], alpha_id, "the old row matches back by its bare key");

	// refresh and page reads on the old row keep resolving to directory 0
	let (status, _) = request(
		&t.app.app,
		"GET",
		&format!("/api/v1/manga/{alpha_id}/chapters?refresh=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let chapters = chapter_keys(&t.app.app, alpha_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k)| k.as_str()).collect();
	assert_eq!(keys, ["ch 1"]);
	let (chapter_id, _) = chapters[0];
	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"alpha-1-p1");

	// 0:Alpha is not a second spelling of the same manga, and Alpha is not in directory 1
	assert_eq!(register_status(&t.app.app, "0:Alpha").await, StatusCode::BAD_REQUEST);
	assert_eq!(register_status(&t.app.app, "1:Alpha").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn directory_prefix_validation() {
	let t = two_dir_app().await;
	enable_both_dirs(&t).await;

	// only the canonical `N:<path>` form (N >= 1, no leading zeros, non-empty path) is a legal prefix
	for key in ["0:Alpha", "00:Alpha", "01:Alpha", "1:", "1:/Alpha"] {
		assert_eq!(
			register_status(&t.app.app, key).await,
			StatusCode::BAD_REQUEST,
			"{key} must be rejected as malformed"
		);
	}
	// an in-range-form but out-of-range index resolves to no directory
	assert_eq!(register_status(&t.app.app, "9:Alpha").await, StatusCode::NOT_FOUND);
	// a directory-0 name containing a colon keeps working as a plain name
	write_page(&t.dir_a.path().join("Note: 1/ch 1"), "1.png", b"note-1-1");
	let note_id = register_local(&t.app.app, "Note: 1").await;
	let chapters = chapter_keys(&t.app.app, note_id).await;
	assert_eq!(chapters.len(), 1);
}

#[tokio::test]
async fn settings_local_dirs_put_validation_and_config_fallback() {
	let t = two_dir_app().await;

	for value in [
		json!("not-an-array"),
		json!(123),
		json!(["/manga", 1]),
		json!([""]),
		json!(["/manga", "   "]),
	] {
		let (status, body) = request(
			&t.app.app,
			"PUT",
			"/api/v1/settings",
			Some(json!({"local.dirs": value})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}

	// a valid override takes effect immediately and reads back through the injected localDirs
	enable_both_dirs(&t).await;

	// an empty array clears the override: resolution falls back to the config layer (directory A)
	let (status, body) = request(
		&t.app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"local.dirs": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(
		body["readOnly"]["localDirs"],
		json!([t.dir_a.path().to_string_lossy()]),
		"the cleared override exposes the config layer again"
	);
	let source_id = local_source_id(&t.app.app).await;
	let entries = browse_keys(&t.app.app, source_id).await;
	let mut keys: Vec<&str> = entries.iter().map(|e| e["key"].as_str().unwrap()).collect();
	keys.sort();
	assert_eq!(keys, ["Alpha", "Shared"]);
}

#[tokio::test]
async fn settings_get_exposes_local_dirs_array() {
	let t = two_dir_app().await;

	// no override stored: the injected value is the config layer, and the retired localDir key is gone
	let (status, body) = request(&t.app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["readOnly"]["localDirs"], json!([t.dir_a.path().to_string_lossy()]));
	assert!(body.get("localDir").is_none(), "localDir is replaced by localDirs: {body}");

	enable_both_dirs(&t).await;
	let (status, body) = request(&t.app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["readOnly"]["localDirs"], json!([
		t.dir_a.path().to_string_lossy(),
		t.dir_b.path().to_string_lossy(),
	]));
}

#[tokio::test]
async fn update_scheduler_rescans_numbered_directory() {
	let t = two_dir_app().await;
	enable_both_dirs(&t).await;
	let beta_id = register_local(&t.app.app, "1:Beta").await;
	let (status, _) = request(
		&t.app.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": beta_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	write_page(&t.dir_b.path().join("Beta/ch 3"), "1.png", b"beta-3-1");
	let (status, _) = request(&t.app.app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&t.app.app, "POST", "/api/v1/update/fetch", None).await;
	assert_eq!(status, StatusCode::OK);
	// the fetch runs asynchronously: wait for the behavioral result (ch 3 appears), then check the summary
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let keys: Vec<String> = chapter_keys(&t.app.app, beta_id)
			.await
			.into_iter()
			.map(|(_, k)| k)
			.collect();
		if keys.iter().any(|k| k == "ch 3") {
			break;
		}
		assert!(Instant::now() < deadline, "the update never rescanned directory 1: {keys:?}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	let (status, body) = request(&t.app.app, "GET", "/api/v1/update/summary", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["failed"], 0, "{body}");
}

#[tokio::test]
async fn oneshots_archives_under_numbered_root_resolve() {
	let t = two_dir_app().await;
	write_cbz(
		&t.dir_b.path().join("oneshots/x.cbz"),
		&[("p2.png", b"os-x-p2"), ("p1.png", b"os-x-p1")],
	);
	let (status, body) = request(
		&t.app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({
			"local.dirs": [
				t.dir_a.path().to_string_lossy(),
				t.dir_b.path().to_string_lossy(),
			],
			"local.oneshots_dir": "oneshots",
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let source_id = local_source_id(&t.app.app).await;
	let entries = browse_keys(&t.app.app, source_id).await;
	assert!(
		entries.iter().any(|e| e["key"] == "1:oneshots/x.cbz"),
		"the archive is its own manga under the numbered root: {entries:?}"
	);
	assert!(
		!entries.iter().any(|e| e["key"] == "1:oneshots"),
		"the oneshots directory itself is never claimed: {entries:?}"
	);

	let manga_id = register_local(&t.app.app, "1:oneshots/x.cbz").await;
	let chapters = chapter_keys(&t.app.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k)| k.as_str()).collect();
	assert_eq!(keys, ["x.cbz"], "an oneshot has exactly one chapter: the archive itself");

	let (chapter_id, _) = chapters[0];
	let (status, body) = request(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/pages"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["p1.png", "p2.png"]);
	let (status, _, bytes) = request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"os-x-p1");
}
