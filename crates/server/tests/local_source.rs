//! Built-in local source contract: seeded by migration, undeletable / real-scan dedup and skip rules /
//! persisted only when opened / page bytes and order (image chapters and CBZ identical) / chapter
//! number sorting / ComicInfo mapping / cover fallback chain / refresh rescan / failed-directory
//! protection / local_dir config override / browse search arm (real scan + substring filter, same
//! shape for single source and cross-source aggregation) / listings 404 unsupported /
//! nested recognition (container-vs-series classification, relative-path keys, subtree absorption,
//! nested failed-entry protection with the container prefix rule)
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, request, request_full, source_by_key, test_app};
use serde_json::json;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;

const COMIC_INFO_ALPHA: &str = r#"<?xml version="1.0"?>
<ComicInfo xmlns:ty="http://www.w3.org/2001/XMLSchema">
  <Series>Alpha Series</Series>
  <Summary>alpha desc</Summary>
  <Writer>w1, w2</Writer>
  <Penciller>p1</Penciller>
  <Genre>Action, Adventure</Genre>
  <ty:PublishingStatusTachiyomi>Completed</ty:PublishingStatusTachiyomi>
</ComicInfo>"#;

const COMIC_INFO_DELTA: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Delta Series</Series>
  <PublishingStatusTachiyomi>Ongoing</PublishingStatusTachiyomi>
</ComicInfo>"#;

/// Real 3000x4000 image: the downsampling branch needs a real decode
fn big_png() -> Vec<u8> {
	let image = image::RgbImage::from_fn(3000, 4000, |x, y| {
		image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 255) as u8])
	});
	let mut out = Vec::new();
	image::DynamicImage::ImageRgb8(image)
		.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
		.expect("encode png");
	out
}

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

/// Builds the local tree and boots the app; local_dir uses the <config_dir>/local default derivation
async fn local_app() -> TestApp {
	local_app_with(|_| {}).await
}

async fn local_app_with(configure: impl FnOnce(&mut Config)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_local_tree(&tmp.path().join("local"));
	let mut config = base_config(&tmp);
	configure(&mut config);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

/// Alpha's ch 2 single-page big image, needed by the downsampling assertions; returns the on-disk original bytes
fn seed_local_tree(root: &std::path::Path) {
	let alpha = root.join("Alpha");
	std::fs::create_dir_all(&alpha).expect("mkdir alpha");
	std::fs::write(alpha.join("ComicInfo.xml"), COMIC_INFO_ALPHA).expect("write comicinfo");
	write_page(&alpha.join("ch 1"), "p2.png", b"alpha-p2");
	write_page(&alpha.join("ch 1"), "P1.png", b"alpha-p1");
	write_page(&alpha.join("ch 1"), "p10.png", b"alpha-p10");
	write_page(&alpha.join("ch 1"), ".hidden.png", b"hidden");
	write_page(&alpha.join("ch 2"), "big.png", &big_png());
	write_page(&alpha.join("第3话"), "001.png", b"alpha-3-1");
	write_cbz(
		&alpha.join("ch 4.cbz"),
		&[
			("b.png", b"alpha-4-b"),
			("a.png", b"alpha-4-a"),
			("ComicInfo.xml", b"<ComicInfo/>"),
			("__MACOSX/x.png", b"junk"),
		],
	);
	write_page(&alpha.join("ch 10"), "1.png", b"alpha-10-1");
	write_page(&alpha.join("extras"), "1.png", b"alpha-extras-1");

	let beta = root.join("Beta");
	write_page(&beta.join("ch 1"), "1.png", b"beta-1-1");
	std::fs::write(beta.join("cover.jpg"), b"beta-sidecar-cover").expect("write sidecar");

	let gamma = root.join("Gamma");
	write_page(&gamma.join("ch 1"), "g1.png", b"gamma-1-g1");
	write_page(&gamma.join("ch 1"), "g2.png", b"gamma-1-g2");
	write_page(&gamma.join("ch 2"), "h1.png", b"gamma-2-h1");

	let delta = root.join("Delta");
	std::fs::create_dir_all(&delta).expect("mkdir delta");
	write_cbz(
		&delta.join("ch 1.cbz"),
		&[("z.png", b"delta-1-z"), ("m.png", b"delta-1-m"), ("ComicInfo.xml", COMIC_INFO_DELTA.as_bytes())],
	);

	std::fs::create_dir_all(root.join("Epsilon")).expect("mkdir epsilon");

	// skip rules: dot-leading directories, @eaDir, and loose files in the root don't count as manga
	std::fs::create_dir_all(root.join(".hidden")).expect("mkdir hidden");
	std::fs::create_dir_all(root.join("@eaDir")).expect("mkdir eaDir");
	std::fs::write(root.join("note.txt"), b"not a manga").expect("write note");
}

async fn local_source_id(app: &Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "local.local")["id"]
		.as_i64()
		.expect("local source id")
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

async fn chapters_of(app: &Router, manga_id: i64) -> Vec<(i64, String, Option<f64>, String)> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters array")
		.iter()
		.map(|c| {
			(
				c["id"].as_i64().expect("chapter id"),
				c["key"].as_str().expect("chapter key").to_string(),
				c["chapter_number"].as_f64(),
				c["title"].as_str().unwrap_or_default().to_string(),
			)
		})
		.collect()
}

async fn chapter_id_of(app: &Router, manga_id: i64, key: &str) -> i64 {
	chapters_of(app, manga_id)
		.await
		.into_iter()
		.find(|(_, k, _, _)| k == key)
		.map(|(id, _, _, _)| id)
		.unwrap_or_else(|| panic!("chapter {key}"))
}

#[tokio::test]
async fn migration_seeds_builtin_local_source() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local = source_by_key(&body, "local.local");
	assert_eq!(local["name"], "Local");
	assert_eq!(local["kind"], "local");
	assert_eq!(source_by_key(&body, "en.example-source")["kind"], "aidoku");
	let local_id = local["id"].as_i64().expect("local id");

	// the built-in source can't be deleted
	let (status, body) = request(&app, "DELETE", &format!("/api/v1/sources/{local_id}"), None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert_eq!(body["error"]["code"], "bad_request");

	// no wasm instance: dynamic capabilities 404 unsupported per the endpoint contract
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{local_id}/filters"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");
}

#[tokio::test]
async fn scan_lists_only_real_manga_dirs() {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_local_tree(&tmp.path().join("local"));
	let dirs = sumidoku_server::local::scan_manga_dirs(&tmp.path().join("local"))
		.await
		.expect("scan");
	assert_eq!(dirs, ["Alpha", "Beta", "Delta", "Epsilon", "Gamma"]);
}

#[tokio::test]
async fn register_persists_scan_metadata_and_chapter_order() {
	let TestApp { app, tmp: _tmp, db_path: _ } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Alpha Series");
	assert_eq!(body["authors"], json!(["w1", "w2"]));
	assert_eq!(body["artists"], json!(["p1"]));
	assert_eq!(body["description"], "alpha desc");
	assert_eq!(body["tags"], json!(["Action", "Adventure"]));
	assert_eq!(body["status"], "Completed");

	// descending chapter numbers, unparsed extras sink to the bottom
	let chapters = chapters_of(&app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["ch 10", "ch 4.cbz", "第3话", "ch 2", "ch 1", "extras"]);
	let numbers: Vec<Option<f64>> = chapters.iter().map(|(_, _, n, _)| *n).collect();
	assert_eq!(
		numbers,
		[Some(10.0), Some(4.0), Some(3.0), Some(2.0), Some(1.0), None]
	);
	// archive chapters' display name drops the extension
	assert_eq!(chapters[1].3, "ch 4");
}

#[tokio::test]
async fn image_chapter_pages_natural_order_and_bytes() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 1").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["P1.png", "p2.png", "p10.png"]);

	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, b"alpha-p2");
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/9"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let page_count = body
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["key"] == "ch 1")
		.expect("ch 1 row")["pageCount"]
		.as_i64()
		.unwrap();
	assert_eq!(page_count, 3);
}

#[tokio::test]
async fn downsample_reuses_local_original_bytes() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 2").await;

	// no width: original bytes pass through
	let original = std::fs::read(tmp.path().join("local/Alpha/ch 2/big.png")).expect("read big.png");
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, original);

	// ?width=512: the same downsampling pipeline as online pages (width quantized to 256 buckets)
	let (status, headers, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0?width=512"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	let image = image::ImageReader::new(std::io::Cursor::new(bytes))
		.with_guessed_format()
		.expect("format")
		.decode()
		.expect("decode");
	assert_eq!((image.width(), image.height()), (512, 682));
}

#[tokio::test]
async fn cbz_chapter_pages_and_bytes_match_image_chapter() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 4.cbz").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["a.png", "b.png"]);

	for (index, want) in [(0, b"alpha-4-a"), (1, b"alpha-4-b")] {
		let (status, headers, bytes) =
			request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["content-type"], "image/png");
		assert_eq!(bytes, want);
	}

	// OPDS-PSE shares the same Local branch
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/opds/v1.2/stream/{chapter_id}/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"alpha-4-a");
}

#[tokio::test]
async fn comic_info_extracted_from_cbz() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Delta").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Delta Series");
	assert_eq!(body["status"], "Ongoing");
}

#[tokio::test]
async fn cover_sidecar_wins() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Beta").await;
	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/jpeg");
	assert_eq!(bytes, b"beta-sidecar-cover");
}

#[tokio::test]
async fn cover_falls_back_to_last_chapter_first_page() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	// the chapter list is descending by number, so the tail = the oldest chapter (ch 1); take its first page
	let gamma_id = register_local(&app, "Gamma").await;
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/manga/{gamma_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"gamma-1-g1");

	// archive chapters fall back the same way: take the first image entry in the archive's natural order
	let delta_id = register_local(&app, "Delta").await;
	let (status, headers, bytes) = request_full(&app, "GET", &format!("/api/v1/manga/{delta_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, b"delta-1-m");

	// the OPDS cover shares the same cache and the same branch
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/opds/v1.2/cover/{delta_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"delta-1-m");
}

#[tokio::test]
async fn cover_missing_answers_404() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Epsilon").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn refresh_rescans_added_and_removed_chapters() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;

	write_page(&tmp.path().join("local/Alpha/ch 11"), "1.png", b"alpha-11-1");
	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<String> = chapters_of(&app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert_eq!(keys[0], "ch 11");

	std::fs::remove_dir_all(tmp.path().join("local/Alpha/ch 2")).expect("remove ch 2");
	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<String> = chapters_of(&app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert!(!keys.iter().any(|k| k == "ch 2"), "真删除的章节应对账掉: {keys:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_manga_dir_never_deletes_rows() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let before = chapters_of(&app, manga_id).await;
	assert_eq!(before.len(), 6);

	// simulates a mount flapping: when the manga directory is unreadable, refresh errors out and existing rows are never treated as deleted
	let alpha = tmp.path().join("local/Alpha");
	let mut perms = std::fs::metadata(&alpha).expect("metadata").permissions();
	use std::os::unix::fs::PermissionsExt;
	perms.set_mode(0o000);
	std::fs::set_permissions(&alpha, perms).expect("chmod 000");

	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

	let mut perms = std::fs::metadata(&alpha).expect("metadata").permissions();
	perms.set_mode(0o755);
	std::fs::set_permissions(&alpha, perms).expect("chmod 755");

	let after = chapters_of(&app, manga_id).await;
	assert_eq!(after.len(), before.len(), "读失败保护：章节行必须原样保留");
	let keys: Vec<&str> = after.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["ch 10", "ch 4.cbz", "第3话", "ch 2", "ch 1", "extras"]);
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_chapter_entry_never_deletes_rows() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let sigma = tmp.path().join("local/Sigma");
	write_page(&sigma.join("ch 1"), "1.png", b"sigma-1-1");
	write_page(&sigma.join("ch 3"), "1.png", b"sigma-3-1");
	// ch 2 is a symlink to a real directory: at registration metadata follows the target successfully, and the chapter persists normally
	let target = tmp.path().join("target-ch2");
	write_page(&target, "1.png", b"sigma-2-1");
	std::os::unix::fs::symlink(&target, sigma.join("ch 2")).expect("symlink ch 2");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Sigma").await;
	let before = chapters_of(&t.app, manga_id).await;
	assert_eq!(before.len(), 3);

	// before refresh: ch 3 is truly deleted (control group, reconciled away as usual); ch 2's target is
	// removed leaving the symlink dangling, entry.metadata() follows the symlink and must fail → entry-level failed → protected reconciliation
	std::fs::remove_dir_all(sigma.join("ch 3")).expect("remove ch 3");
	std::fs::remove_dir_all(&target).expect("remove ch 2 target");

	// an entry-level failure degrades instead of erroring: refresh still returns OK; protected rows skip
	// this round's reordering (keeping their old source_order), so assert on the content set, not positions
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let mut keys: Vec<String> = chapters_of(&t.app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	keys.sort();
	assert_eq!(keys, ["ch 1", "ch 2"], "悬垂 symlink 的章节行必须保留，真删的对账掉: {keys:?}");
}

#[tokio::test]
async fn invalid_or_unknown_keys_rejected() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;
	// nested keys are well-formed relative paths: they pass the guard and fail on the filesystem lookup instead
	for (key, want) in [("../escape", 400), ("a/b", 404), ("Nope", 404)] {
		let (status, body) = request(
			&app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "mangaKey": key})),
		)
		.await;
		assert_eq!(status, want as u16, "{key}: {body}");
	}
}

#[tokio::test]
async fn local_dir_config_override() {
	let outside = tempfile::tempdir().expect("outside dir");
	seed_local_tree(&outside.path().join("manga"));
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dir = Some(outside.path().join("manga"));
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Beta").await;
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"beta-sidecar-cover");
	drop(outside);
}

#[tokio::test]
async fn shelving_progress_and_library_filters_behave_like_online() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 1").await;

	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 2})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("library entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["id"], manga_id);

	let source_id = local_source_id(&app).await;
	let (status, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(status, StatusCode::OK);
	let entry = body
		.as_array()
		.expect("counts array")
		.iter()
		.find(|e| e["sourceId"] == source_id)
		.expect("local count");
	assert_eq!(entry["count"], 1);

	// the read-filter tri-state runs the same query as online sources
	let (status, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().unwrap().len(), 1);
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let ch1 = body
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["key"] == "ch 1")
		.expect("ch 1");
	assert_eq!(ch1["lastPageRead"], 2);
	assert!(ch1["lastReadAt"].as_i64().is_some());
}

#[tokio::test]
async fn update_scheduler_rescans_local_library() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	write_page(&tmp.path().join("local/Alpha/ch 11"), "1.png", b"alpha-11-1");
	let (status, _) = request(&app, "POST", "/api/v1/update/fetch", None).await;
	assert_eq!(status, StatusCode::OK);
	// fetch runs asynchronously: wait for the behavioral result (the new chapter appears), then reconcile the scheduler counts
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let keys: Vec<String> = chapters_of(&app, manga_id)
			.await
			.into_iter()
			.map(|(_, k, _, _)| k)
			.collect();
		if keys[0] == "ch 11" {
			break;
		}
		assert!(Instant::now() < deadline, "调度器重扫没看到新章: {keys:?}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	let (status, body) = request(&app, "GET", "/api/v1/update/summary", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["failed"], 0, "{body}");
	assert_eq!(body["complete"], 1, "{body}");
}

#[tokio::test]
async fn browse_search_scans_dirs_and_filters() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// empty query (absent or pure whitespace) returns everything: browse = real scan, entries in the online search shape (sourceId attached)
	for body in [json!({}), json!({"query": "  "})] {
		let (status, body) = request(
			&app,
			"POST",
			&format!("/api/v1/sources/{source_id}/search"),
			Some(body),
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{body}");
		assert_eq!(body["hasNextPage"], false);
		let entries = body["entries"].as_array().expect("entries array");
		let titles: Vec<&str> = entries.iter().map(|e| e["title"].as_str().unwrap()).collect();
		assert_eq!(titles, ["Alpha", "Beta", "Delta", "Epsilon", "Gamma"]);
		for entry in entries {
			assert_eq!(entry["sourceId"], source_id);
			assert_eq!(entry["key"], entry["title"]);
			assert!(entry["id"].is_null(), "未入库的条目不带内部 id: {entry}");
		}
	}

	// substring filtering is case-insensitive
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "aLP"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["title"], "Alpha");

	// shelved entries carry the internal id, which the browse page uses to jump straight to the detail page
	let manga_id = register_local(&app, "Beta").await;
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "beta"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"][0]["id"], manga_id);

	// a directory name nothing scans to: an empty set is not an error
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "nope"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"], json!([]));
}

#[tokio::test]
async fn aggregate_search_includes_local_source() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// cross-source aggregation with the local source selected yields a result entry, not an unsupported error entry
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"query": "gamma", "sourceIds": [source_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let sources = body["sources"].as_array().expect("sources array");
	assert_eq!(sources.len(), 1);
	let entry = &sources[0];
	assert_eq!(entry["sourceId"], source_id);
	assert!(entry.get("error").is_none(), "{entry}");
	assert_eq!(entry["hasNextPage"], false);
	let entries = entry["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["title"], "Gamma");
	assert_eq!(entries[0]["sourceId"], source_id);
}

#[tokio::test]
async fn settings_expose_local_dir_read_only() {
	let TestApp { app, tmp, db_path } = local_app().await;
	let effective = tmp.path().join("local").to_string_lossy().into_owned();

	// GET injects the effective local directory (the config-layer value, default <config_dir>/local)
	let (status, body) = request(&app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["localDir"], json!(effective));

	// a PUT to the same key is skipped: the read-back is still the config value, other keys persist as usual
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"localDir": "/elsewhere", "incognito.enabled": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["localDir"], json!(effective));
	assert_eq!(body["incognito.enabled"], true);

	// skipped is not shadowed: there is no localDir row in the setting table
	let pool = common::connect_db(&db_path).await;
	let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'localDir'")
		.fetch_optional(&pool)
		.await
		.expect("query setting");
	assert!(row.is_none(), "localDir 不应落 setting 表");
}

#[tokio::test]
async fn local_listings_answer_unsupported() {	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// the local source has no listings concept: same contract as a wasm source lacking the capability, 404 unsupported
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");
}

/// Komga-style nested fixture: two archive series under the pure container `publisher`, and one
/// archive-free manga whose chapters live in image-leaf directories two levels down
fn seed_nested_tree(root: &std::path::Path) {
	write_cbz(
		&root.join("publisher/series1/a.cbz"),
		&[("001.png", b"s1-a-1"), ("002.png", b"s1-a-2")],
	);
	write_cbz(&root.join("publisher/series2/b.cbz"), &[("001.png", b"s2-b-1")]);

	write_page(&root.join("My Series/vol1/ch01"), "p2.png", b"ms-1-p2");
	write_page(&root.join("My Series/vol1/ch01"), "P1.png", b"ms-1-p1");
	write_page(&root.join("My Series/vol1/ch01"), "p10.png", b"ms-1-p10");
	write_page(&root.join("My Series/vol1/ch02"), "001.png", b"ms-2-1");
}

async fn nested_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_nested_tree(&tmp.path().join("local"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

#[tokio::test]
async fn nested_scan_discovers_series_under_containers() {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_nested_tree(&tmp.path().join("local"));
	let dirs = sumidoku_server::local::scan_manga_dirs(&tmp.path().join("local"))
		.await
		.expect("scan");
	// the publisher grouping directory itself is never a manga
	assert_eq!(dirs, ["My Series", "publisher/series1", "publisher/series2"]);
}

#[tokio::test]
async fn nested_browse_lists_leaf_titles_and_filters() {
	let TestApp { app, tmp: _tmp, .. } = nested_app().await;
	let source_id = local_source_id(&app).await;

	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	let keys: Vec<&str> = entries.iter().map(|e| e["key"].as_str().unwrap()).collect();
	assert_eq!(keys, ["My Series", "publisher/series1", "publisher/series2"]);
	let titles: Vec<&str> = entries.iter().map(|e| e["title"].as_str().unwrap()).collect();
	assert_eq!(titles, ["My Series", "series1", "series2"], "browse shows the leaf title, not the container");

	// the substring filter matches the leaf title, not the container path
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "series"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().unwrap().len(), 3);
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "publisher"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"], json!([]));

	// a shelved nested manga is matched back by its full relative key
	let manga_id = register_local(&app, "publisher/series1").await;
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "series1"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"][0]["id"], manga_id);
	assert_eq!(body["entries"][0]["key"], "publisher/series1");
}

#[tokio::test]
async fn nested_image_leaf_chapter_reads_pages_in_order() {
	let TestApp { app, tmp: _tmp, .. } = nested_app().await;
	let manga_id = register_local(&app, "My Series").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "My Series");

	let chapters = chapters_of(&app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["vol1/ch02", "vol1/ch01"]);
	let numbers: Vec<Option<f64>> = chapters.iter().map(|(_, _, n, _)| *n).collect();
	assert_eq!(numbers, [Some(2.0), Some(1.0)]);

	let chapter_id = chapter_id_of(&app, manga_id, "vol1/ch01").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["P1.png", "p2.png", "p10.png"]);

	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, b"ms-1-p2");
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/9"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn manga_absorbs_nested_archives() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let series = tmp.path().join("local/Absorbed");
	write_cbz(&series.join("ch5.cbz"), &[("1.png", b"abs-5-1")]);
	write_cbz(
		&series.join("nested/x.cbz"),
		&[("p2.png", b"abs-x-p2"), ("p1.png", b"abs-x-p1")],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	// the claimed manga absorbs its whole subtree: nested is never a manga of its own
	let dirs = sumidoku_server::local::scan_manga_dirs(&t.tmp.path().join("local"))
		.await
		.expect("scan");
	assert_eq!(dirs, ["Absorbed"]);

	let manga_id = register_local(&t.app, "Absorbed").await;
	let chapters = chapters_of(&t.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["ch5.cbz", "nested/x.cbz"]);
	let titles: Vec<&str> = chapters.iter().map(|(_, _, _, title)| title.as_str()).collect();
	assert_eq!(titles, ["ch5", "nested/x"], "archive display names drop the extension but keep the path");
	let numbers: Vec<Option<f64>> = chapters.iter().map(|(_, _, n, _)| *n).collect();
	assert_eq!(numbers, [Some(5.0), None]);

	let chapter_id = chapter_id_of(&t.app, manga_id, "nested/x.cbz").await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["p1.png", "p2.png"]);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"abs-x-p1");
}

#[tokio::test]
async fn same_name_dirs_at_different_depths_do_not_collide() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_page(&tmp.path().join("local/Same/a/ch01"), "1.png", b"same-a-1");
	write_page(&tmp.path().join("local/Same/b/ch01"), "1.png", b"same-b-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let manga_id = register_local(&t.app, "Same").await;
	let chapters = chapters_of(&t.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["b/ch01", "a/ch01"], "equal chapter numbers tie-break on the descending natural path");

	// the keys carry the path, so each chapter reads its own directory
	for (key, want) in [("a/ch01", b"same-a-1"), ("b/ch01", b"same-b-1")] {
		let chapter_id = chapter_id_of(&t.app, manga_id, key).await;
		let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(bytes, want, "{key} must read its own directory");
	}
}

#[tokio::test]
async fn path_guard_rejects_traversal_absolute_and_drive() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;
	for key in ["../escape", "a/../b", "a/./b", "a//b", "a/b/", "/abs", "a\\b", "C:/x", "c:/y"] {
		let (status, body) = request(
			&app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "mangaKey": key})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {body}");
	}
	// well-formed relative paths pass the guard and fail only on the filesystem lookup
	for key in ["a/b", "a/b/c"] {
		let (status, body) = request(
			&app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "mangaKey": key})),
		)
		.await;
		assert_eq!(status, StatusCode::NOT_FOUND, "{key}: {body}");
	}
}

#[tokio::test]
async fn nested_comic_info_and_cover_resolve() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	// top-level ComicInfo wins and overrides the leaf-name title; the sidecar sits directly in the manga directory
	let series_a = root.join("Pub/Series A");
	std::fs::create_dir_all(&series_a).expect("mkdir series a");
	std::fs::write(series_a.join("ComicInfo.xml"), COMIC_INFO_ALPHA).expect("write comicinfo");
	std::fs::write(series_a.join("cover.jpg"), b"sa-sidecar-cover").expect("write sidecar");
	write_cbz(&series_a.join("vol1/ch1.cbz"), &[("1.png", b"sa-1-1")]);
	// no top-level file: the first archive carrying ComicInfo.xml is read through its nested key
	write_cbz(
		&root.join("Pub/Series B/nested/ch1.cbz"),
		&[("z.png", b"sb-1-z"), ("ComicInfo.xml", COMIC_INFO_DELTA.as_bytes())],
	);
	write_page(&root.join("Pub/Series C/vol1/ch01"), "1.png", b"sc-1-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let a_id = register_local(&t.app, "Pub/Series A").await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{a_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Alpha Series");
	let keys: Vec<String> = chapters_of(&t.app, a_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert_eq!(keys, ["vol1/ch1.cbz"]);

	let b_id = register_local(&t.app, "Pub/Series B").await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{b_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Delta Series");

	let (status, headers, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{a_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/jpeg");
	assert_eq!(bytes, b"sa-sidecar-cover");

	// without a sidecar the fallback chain walks the nested chapter list: last chapter's first page
	let c_id = register_local(&t.app, "Pub/Series C").await;
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{c_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"sc-1-1");
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_nested_entry_never_deletes_rows() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let sigma = tmp.path().join("local/Sigma");
	write_page(&sigma.join("vol1/ch 1"), "1.png", b"sigma-1-1");
	// a dangling chapter link exercises exact protection, a dangling container link prefix protection
	let target_ch2 = tmp.path().join("target-ch2");
	write_page(&target_ch2, "1.png", b"sigma-2-1");
	std::os::unix::fs::symlink(&target_ch2, sigma.join("vol1/ch 2")).expect("symlink ch 2");
	let target_vol2 = tmp.path().join("target-vol2");
	write_page(&target_vol2.join("ch 1"), "1.png", b"sigma-v2-1");
	std::os::unix::fs::symlink(&target_vol2, sigma.join("vol2")).expect("symlink vol2");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Sigma").await;
	let before = chapters_of(&t.app, manga_id).await;
	assert_eq!(before.len(), 3);

	std::fs::remove_dir_all(&target_ch2).expect("remove ch 2 target");
	std::fs::remove_dir_all(&target_vol2).expect("remove vol2 target");
	std::fs::remove_dir_all(sigma.join("vol1/ch 1")).expect("remove ch 1");

	// an entry-level failure degrades instead of erroring: assert on the content set, not positions
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let mut keys: Vec<String> = chapters_of(&t.app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	keys.sort();
	assert_eq!(
		keys,
		["vol1/ch 2", "vol2/ch 1"],
		"chapters under dangling links must be kept, truly deleted ones reconciled: {keys:?}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_nested_container_never_deletes_subtree() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let tau = tmp.path().join("local/Tau");
	write_page(&tau.join("vol1/ch 1"), "1.png", b"tau-1-1");
	write_page(&tau.join("vol1/ch 2"), "1.png", b"tau-2-1");
	write_page(&tau.join("ch 9"), "1.png", b"tau-9-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Tau").await;
	assert_eq!(chapters_of(&t.app, manga_id).await.len(), 3);

	// an unlistable container degrades the scan instead of erroring; the prefix rule protects its whole
	// subtree while a truly deleted sibling still reconciles away
	let vol1 = tau.join("vol1");
	let mut perms = std::fs::metadata(&vol1).expect("metadata").permissions();
	use std::os::unix::fs::PermissionsExt;
	perms.set_mode(0o000);
	std::fs::set_permissions(&vol1, perms).expect("chmod 000");
	std::fs::remove_dir_all(tau.join("ch 9")).expect("remove ch 9");

	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let mut keys: Vec<String> = chapters_of(&t.app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	keys.sort();
	assert_eq!(keys, ["vol1/ch 1", "vol1/ch 2"], "{keys:?}");

	let mut perms = std::fs::metadata(&vol1).expect("metadata").permissions();
	perms.set_mode(0o755);
	std::fs::set_permissions(&vol1, perms).expect("chmod 755");
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<String> = chapters_of(&t.app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert_eq!(keys, ["vol1/ch 2", "vol1/ch 1"], "ch 9 stays deleted, the subtree scans again: {keys:?}");
}
