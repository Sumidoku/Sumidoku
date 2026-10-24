//! Built-in local source contract: seeded by migration, undeletable / real-scan dedup and skip rules /
//! persisted only when opened / page bytes and order (image chapters and CBZ identical) / chapter
//! number sorting / ComicInfo mapping / cover fallback chain / refresh rescan / failed-directory
//! protection / local_dir config override / browse search arm (real scan + substring filter, same
//! shape for single source and cross-source aggregation) / listings 501 unsupported /
//! nested recognition (container-vs-series classification, relative-path keys, subtree absorption,
//! nested failed-entry protection with the container prefix rule) / oneshots directory rule /
//! EPUB chapters (divina profile: validated packages, OPF spine-ordered direct-image and XHTML/SVG
//! pages, declared media types, persisted compatibility, nested and oneshot shapes) / manga metadata sources
//! (first hit wins: top-level ComicInfo.xml > archive ComicInfo.xml > series.json > EPUB OPF)
mod common;

use aidoku::{ContentRating, Viewer};
use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, request, request_full, source_by_key, test_app};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use sumidoku_server::local::{ZipCache, manga_update};

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

	// no wasm instance: dynamic capabilities 501 unsupported per the endpoint contract
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{local_id}/filters"), None).await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], "unsupported");
}

#[tokio::test]
async fn scan_lists_only_real_manga_dirs() {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_local_tree(&tmp.path().join("local"));
	let dirs = sumidoku_server::local::scan_manga_dirs(&[tmp.path().join("local")], None)
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
	config.local_dirs = Some(vec![outside.path().join("manga")]);
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
	let (status, _) = request(&app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
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
async fn settings_no_longer_inject_local_dirs() {
	let TestApp { app, tmp, db_path } = local_app().await;
	let effective = tmp.path().join("local").to_string_lossy().into_owned();

	// the global settings response carries neither the retired local group nor the readOnly injections
	let (status, body) = request(&app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body.get("local").is_none(), "{body}");
	assert!(body.get("readOnly").is_none(), "{body}");

	// the effective roots live on the instance's own settings instead (config-layer value here)
	let source_id = local_source_id(&app).await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["paths"], json!([effective]));

	// a structured write still carrying the retired local group is rejected loudly, not silently dropped
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"local": {"dirs": ["/elsewhere"], "oneshotsDir": null}})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	// the retired flat readOnly key stays skipped: other keys persist as usual, nothing lands in the table
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"localDirs": ["/elsewhere"], "incognito.enabled": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["incognito"]["enabled"], true);

	let pool = common::connect_db(&db_path).await;
	let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'localDirs'")
		.fetch_optional(&pool)
		.await
		.expect("query setting");
	assert!(row.is_none(), "localDirs 不应落 setting 表");
}

#[tokio::test]
async fn local_listings_answer_unsupported() {	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// the local source has no listings concept: same contract as a wasm source lacking the capability, 501 unsupported
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], "unsupported");

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
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
	let dirs = sumidoku_server::local::scan_manga_dirs(&[tmp.path().join("local")], None)
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
	let dirs = sumidoku_server::local::scan_manga_dirs(&[t.tmp.path().join("local")], None)
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

// ----- oneshots directory (SUMIDOKU_LOCAL_ONESHOTS_DIR / the local.oneshots_dir setting) -----

/// Oneshots fixture: archives directly in the oneshots directory become one-chapter manga; the
/// neighboring ComicInfo.xml/cover.jpg are series sidecars and must be ignored, and the nested
/// subdirectory is never entered
fn seed_oneshots_tree(root: &std::path::Path) {
	write_cbz(&root.join("oneshots/Foo.cbz"), &[("p2.png", b"foo-p2"), ("p1.png", b"foo-p1")]);
	write_cbz(&root.join("oneshots/Bar.cbz"), &[("1.png", b"bar-1")]);
	write_cbz(&root.join("oneshots/nested/Baz.cbz"), &[("1.png", b"baz-1")]);
	std::fs::write(root.join("oneshots/ComicInfo.xml"), COMIC_INFO_DELTA).expect("write sidecar comicinfo");
	std::fs::write(root.join("oneshots/cover.jpg"), b"oneshots-sidecar-cover").expect("write sidecar cover");
	write_page(&root.join("Normal/ch 1"), "1.png", b"normal-1-1");
}

async fn oneshots_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_oneshots_tree(&tmp.path().join("local"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn put_oneshots_dir(app: &Router, value: serde_json::Value) -> (StatusCode, serde_json::Value) {
	request(app, "PUT", "/api/v1/settings", Some(json!({"local.oneshots_dir": value}))).await
}

/// The default instance's effective oneshots marker, read through the per-instance settings surface
async fn oneshots_value(app: &Router) -> serde_json::Value {
	let source_id = local_source_id(app).await;
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["values"]["oneshotsDir"].clone()
}

async fn enable_oneshots(app: &Router) {
	let (status, body) = put_oneshots_dir(app, json!("oneshots")).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

/// (key, title) pairs of one local browse, in response order
async fn browse_local(app: &Router) -> Vec<(String, String)> {
	let source_id = local_source_id(app).await;
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["entries"]
		.as_array()
		.expect("entries array")
		.iter()
		.map(|e| {
			(
				e["key"].as_str().expect("key").to_string(),
				e["title"].as_str().expect("title").to_string(),
			)
		})
		.collect()
}

#[tokio::test]
async fn oneshots_dir_lists_archives_as_one_shot_manga() {
	let TestApp { app, tmp: _tmp, .. } = oneshots_app().await;
	enable_oneshots(&app).await;

	let mut entries = browse_local(&app).await;
	entries.sort();
	let pairs: Vec<(&str, &str)> = entries
		.iter()
		.map(|(k, t)| (k.as_str(), t.as_str()))
		.collect();
	// the oneshots directory itself is never claimed (the archive trigger would take it), its nested
	// subdirectory is not entered, and the sibling normal directory keeps the usual behavior
	assert_eq!(
		pairs,
		[
			("Normal", "Normal"),
			("oneshots/Bar.cbz", "Bar"),
			("oneshots/Foo.cbz", "Foo"),
		]
	);
}

#[tokio::test]
async fn oneshots_dir_default_off_claims_directory() {
	let TestApp { app, tmp: _tmp, .. } = oneshots_app().await;
	// control group: with nothing configured the oneshots directory is claimed by the plain archive
	// trigger, so the rule never misfires on an unconfigured library
	let mut entries = browse_local(&app).await;
	entries.sort();
	let pairs: Vec<(&str, &str)> = entries
		.iter()
		.map(|(k, t)| (k.as_str(), t.as_str()))
		.collect();
	assert_eq!(pairs, [("Normal", "Normal"), ("oneshots", "oneshots")]);
}

#[tokio::test]
async fn oneshot_manga_opens_as_single_chapter() {
	let TestApp { app, tmp: _tmp, .. } = oneshots_app().await;
	enable_oneshots(&app).await;
	let manga_id = register_local(&app, "oneshots/Foo.cbz").await;

	// the stem is the title: the ComicInfo.xml sidecar sitting next to the archive is ignored
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Foo");

	let chapters = chapters_of(&app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["Foo.cbz"], "an oneshot has exactly one chapter: the archive itself");
	assert_eq!(chapters[0].3, "Foo", "the archive chapter's display name drops the extension");

	let chapter_id = chapter_id_of(&app, manga_id, "Foo.cbz").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["p1.png", "p2.png"]);
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"foo-p2");

	// the cover.* sidecar next to the archive is ignored: the cover is the single chapter's first page
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"foo-p1");
}

#[tokio::test]
async fn oneshots_rule_matches_at_any_depth_case_insensitive() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_cbz(&tmp.path().join("local/pub/oneshots/x.cbz"), &[("p1.png", b"x-p1")]);
	// the match is a case-insensitive substring of the relative path, same as kmrs's contains_ignore_case
	write_cbz(&tmp.path().join("local/pub/my-oneshots/y.cbz"), &[("1.png", b"y-1")]);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let (status, body) = put_oneshots_dir(&t.app, json!("ONESHOTS")).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let mut entries = browse_local(&t.app).await;
	entries.sort();
	let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
	assert_eq!(keys, ["pub/my-oneshots/y.cbz", "pub/oneshots/x.cbz"]);

	let manga_id = register_local(&t.app, "pub/oneshots/x.cbz").await;
	let chapters = chapters_of(&t.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["x.cbz"]);
}

#[tokio::test]
async fn oneshots_dir_setting_validation_clear_and_injection() {
	let TestApp { app, tmp: _tmp, .. } = oneshots_app().await;

	// unconfigured: the effective value is null
	assert_eq!(oneshots_value(&app).await, serde_json::Value::Null);

	// only a string is accepted
	for value in [json!(123), json!(true), json!(["oneshots"])] {
		let (status, body) = put_oneshots_dir(&app, value.clone()).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}

	enable_oneshots(&app).await;
	assert_eq!(oneshots_value(&app).await, json!("oneshots"));
	let entries = browse_local(&app).await;
	assert!(
		entries.iter().any(|(k, _)| k == "oneshots/Foo.cbz"),
		"the override takes effect without a restart: {entries:?}"
	);

	// an empty or blank string clears the override; with nothing in the config layer the feature is off
	for value in [json!(""), json!("   ")] {
		let (status, body) = put_oneshots_dir(&app, value).await;
		assert_eq!(status, StatusCode::OK, "{body}");
		assert_eq!(oneshots_value(&app).await, serde_json::Value::Null);
		let mut entries = browse_local(&app).await;
		entries.sort();
		let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
		assert_eq!(keys, ["Normal", "oneshots"], "cleared = back to the plain classification");
	}
}

#[tokio::test]
async fn oneshots_dir_from_config_and_setting_override() {
	// the config layer (SUMIDOKU_LOCAL_ONESHOTS_DIR / local_oneshots_dir) turns the rule on with no setting stored
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_oneshots_tree(&tmp.path().join("local"));
	let mut config = base_config(&tmp);
	config.local_oneshots_dir = Some("oneshots".into());
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let (status, body) = request(&t.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(oneshots_value(&t.app).await, json!("oneshots"), "config alone enables the rule");
	let entries = browse_local(&t.app).await;
	assert!(
		entries.iter().any(|(k, _)| k == "oneshots/Foo.cbz"),
		"config alone enables the rule: {entries:?}"
	);

	// a runtime setting wins over the config name: matching nothing, the directory is pass1-claimed again
	let (status, body) = put_oneshots_dir(&t.app, json!("singles")).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(oneshots_value(&t.app).await, json!("singles"));
	let mut entries = browse_local(&t.app).await;
	entries.sort();
	let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
	assert_eq!(keys, ["Normal", "oneshots"], "{keys:?}");
}

// ----- EPUB chapters (divina profile) -----

/// Minimal OCF container: `opf` is the rootfile's (full-path, raw bytes); None writes no
/// container.xml/OPF at all. The fixture is built in-test so no binary EPUB lands in the repo.
fn write_epub(path: &std::path::Path, opf: Option<(&str, &[u8])>, entries: &[(&str, &[u8])]) {
	write_epub_with_mimetype(path, Some(b"application/epub+zip"), opf, entries);
}

fn write_epub_with_mimetype(path: &std::path::Path, mimetype: Option<&[u8]>, opf: Option<(&str, &[u8])>, entries: &[(&str, &[u8])]) {
	std::fs::create_dir_all(path.parent().expect("epub parent")).expect("mkdir epub parent");
	let file = std::fs::File::create(path).expect("create epub");
	let mut writer = zip::ZipWriter::new(file);
	if let Some(mimetype) = mimetype {
		writer
			.start_file("mimetype", zip::write::SimpleFileOptions::default())
			.expect("start mimetype entry");
		std::io::Write::write_all(&mut writer, mimetype).expect("write mimetype entry");
	}
	if let Some((opf_path, opf_bytes)) = opf {
		let container = format!(
			r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="{opf_path}" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
		);
		writer
			.start_file("META-INF/container.xml", zip::write::SimpleFileOptions::default())
			.expect("start container entry");
		std::io::Write::write_all(&mut writer, container.as_bytes()).expect("write container entry");
		writer
			.start_file(opf_path, zip::write::SimpleFileOptions::default())
			.expect("start opf entry");
		std::io::Write::write_all(&mut writer, opf_bytes).expect("write opf entry");
	}
	for (name, bytes) in entries {
		writer
			.start_file(*name, zip::write::SimpleFileOptions::default())
			.expect("start entry");
		std::io::Write::write_all(&mut writer, bytes).expect("write entry");
	}
	writer.finish().expect("finish epub");
}

/// Divina-profile OPF: manifest of (id, href, media-type), spine of idrefs in reading order
fn divina_opf(manifest: &[(&str, &str, &str)], spine: &[&str]) -> Vec<u8> {
	divina_opf_with_metadata("", manifest, spine)
}

/// `metadata_xml` lands verbatim inside <metadata> next to the identifier. The shared fixture
/// carries no dc:title on purpose: these EPUBs predate the metadata chain, and a parseable OPF
/// must leave their leaf-name manga titles alone
fn divina_opf_with_metadata(metadata_xml: &str, manifest: &[(&str, &str, &str)], spine: &[&str]) -> Vec<u8> {
	let items: String = manifest
		.iter()
		.map(|(id, href, media_type)| format!(r#"<item id="{id}" href="{href}" media-type="{media_type}"/>"#))
		.collect();
	let itemrefs: String = spine
		.iter()
		.map(|idref| format!(r#"<itemref idref="{idref}"/>"#))
		.collect();
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">sumidoku-test</dc:identifier>
    {metadata_xml}
  </metadata>
  <manifest>{items}</manifest>
  <spine>{itemrefs}</spine>
</package>"#
	)
	.into_bytes()
}

async fn epub_page_entries(app: &Router, chapter_id: i64) -> Vec<String> {
	let (status, body) = request(app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body.as_array()
		.expect("pages array")
		.iter()
		.map(|p| {
			p["content"]["Local"]
				.as_str()
				.expect("local entry")
				.to_string()
		})
		.collect()
}

#[tokio::test]
async fn epub_chapter_pages_follow_spine_order() {
	let tmp = tempfile::tempdir().expect("tempdir");
	// the spine order c, a, d, b is neither alphabetical nor archive entry order; the extensionless
	// page d is a page only through its declared media type, and the unreferenced z.png must not leak
	let opf = divina_opf(
		&[
			("pa", "images/a.png", "image/png"),
			("pb", "images/b.png", "image/png"),
			("pc", "images/c.png", "image/png"),
			("pd", "images/d", "image/webp"),
			("nav", "text/nav.xhtml", "application/xhtml+xml"),
		],
		&["pc", "pa", "pd", "pb"],
	);
	write_epub(
		&tmp.path().join("local/Spine/ch 1.epub"),
		Some(("OEBPS/content.opf", &opf)),
		&[
			("OEBPS/images/a.png", b"spine-1-a"),
			("OEBPS/images/b.png", b"spine-1-b"),
			("OEBPS/images/c.png", b"spine-1-c"),
			("OEBPS/images/d", b"spine-1-d"),
			("OEBPS/images/z.png", b"spine-1-z-unreferenced"),
			("OEBPS/text/nav.xhtml", b"<html/>"),
		],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Spine").await;
	let chapter_id = chapter_id_of(&t.app, manga_id, "ch 1.epub").await;

	let entries = epub_page_entries(&t.app, chapter_id).await;
	assert_eq!(
		entries,
		["OEBPS/images/c.png", "OEBPS/images/a.png", "OEBPS/images/d", "OEBPS/images/b.png"],
		"pages follow the OPF spine, not the natural archive order"
	);

	for (index, want_mime, want_bytes) in [
		(0, "image/png", b"spine-1-c" as &[u8]),
		(1, "image/png", b"spine-1-a"),
		(2, "image/webp", b"spine-1-d"),
		(3, "image/png", b"spine-1-b"),
	] {
		let (status, headers, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
		assert_eq!(status, StatusCode::OK, "page {index}");
		assert_eq!(headers["content-type"], want_mime, "page {index}: the OPF-declared media type wins");
		assert_eq!(bytes, want_bytes, "page {index}");
	}

	// single-chapter manga: the cover chain lands on the first spine page
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"spine-1-c");
}

#[tokio::test]
async fn epub_xhtml_and_svg_pages_follow_spine_order() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let opf = divina_opf(
		&[
			("one", "text/one.xhtml", "application/xhtml+xml"),
			("two", "text/two.xhtml", "application/xhtml+xml"),
			("a", "images/a one.png", "image/png"),
			("b", "images/b", "image/webp"),
		],
		&["two", "one"],
	);
	write_epub(
		&tmp.path().join("local/Markup/ch 1.epub"),
		Some(("OEBPS/content.opf", &opf)),
		&[
			(
				"OEBPS/text/one.xhtml",
				br#"<html><body><img src="../images/a%20one.png"/><img src="../images/a%20one.png"/></body></html>"#,
			),
			(
				"OEBPS/text/two.xhtml",
				br#"<html xmlns:xlink="http://www.w3.org/1999/xlink"><body><svg><image xlink:href="../images/b"/></svg></body></html>"#,
			),
			("OEBPS/images/a one.png", b"markup-a"),
			("OEBPS/images/b", b"markup-b"),
			("OEBPS/images/unreferenced.png", b"markup-unreferenced"),
		],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Markup").await;
	let chapter_id = chapter_id_of(&t.app, manga_id, "ch 1.epub").await;

	let entries = epub_page_entries(&t.app, chapter_id).await;
	assert_eq!(entries, ["OEBPS/images/b", "OEBPS/images/a one.png"]);
	for (index, want_mime, want_bytes) in [(0, "image/webp", b"markup-b" as &[u8]), (1, "image/png", b"markup-a")] {
		let (status, headers, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["content-type"], want_mime);
		assert_eq!(bytes, want_bytes);
	}
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"markup-b");
}

#[tokio::test]
async fn epub_invalid_packages_do_not_fall_back_to_archive_images() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_epub(
		&tmp.path().join("local/Broken/ch 1.epub"),
		Some(("OEBPS/content.opf", b"not xml at all <")),
		&[("OEBPS/images/z.png", b"broken-1-z"), ("OEBPS/images/m.png", b"broken-1-m")],
	);
	write_epub(
		&tmp.path().join("local/Broken/ch 2.epub"),
		None,
		&[("images/b.png", b"broken-2-b"), ("images/a.png", b"broken-2-a")],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Broken").await;

	for key in ["ch 1.epub", "ch 2.epub"] {
		let chapter_id = chapter_id_of(&t.app, manga_id, key).await;
		let entries = epub_page_entries(&t.app, chapter_id).await;
		assert!(entries.is_empty(), "invalid EPUB must not expose archive images: {entries:?}");
	}
}

#[tokio::test]
async fn epub_mimetype_is_validated() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let opf = divina_opf(&[("p1", "images/1.png", "image/png")], &["p1"]);
	write_epub_with_mimetype(
		&tmp.path().join("local/Fake/wrong.epub"),
		Some(b"application/zip"),
		Some(("OEBPS/content.opf", &opf)),
		&[("OEBPS/images/1.png", b"wrong")],
	);
	write_epub_with_mimetype(
		&tmp.path().join("local/Fake/missing.epub"),
		None,
		Some(("OEBPS/content.opf", &opf)),
		&[("OEBPS/images/1.png", b"missing")],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Fake").await;

	for key in ["wrong.epub", "missing.epub"] {
		let chapter_id = chapter_id_of(&t.app, manga_id, key).await;
		let entries = epub_page_entries(&t.app, chapter_id).await;
		assert!(entries.is_empty(), "invalid EPUB mimetype must not expose pages: {entries:?}");
	}
}

#[tokio::test]
async fn epub_non_divina_and_incomplete_books_have_no_pages() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let prose = divina_opf(
		&[("page", "text/page.xhtml", "application/xhtml+xml"), ("image", "images/1.png", "image/png")],
		&["page"],
	);
	write_epub(
		&tmp.path().join("local/Cases/prose.epub"),
		Some(("OEBPS/content.opf", &prose)),
		&[
			(
				"OEBPS/text/page.xhtml",
				br#"<html><body>This is an ordinary reflowable prose page with too much text.<img src="../images/1.png"/></body></html>"#,
			),
			("OEBPS/images/1.png", b"prose"),
		],
	);
	let multi = divina_opf(
		&[
			("page", "text/page.xhtml", "application/xhtml+xml"),
			("one", "images/1.png", "image/png"),
			("two", "images/2.png", "image/png"),
		],
		&["page"],
	);
	write_epub(
		&tmp.path().join("local/Cases/multi.epub"),
		Some(("OEBPS/content.opf", &multi)),
		&[
			(
				"OEBPS/text/page.xhtml",
				br#"<html><body><img src="../images/1.png"/><img src="../images/2.png"/></body></html>"#,
			),
			("OEBPS/images/1.png", b"multi-1"),
			("OEBPS/images/2.png", b"multi-2"),
		],
	);
	let missing = divina_opf(&[("page", "images/missing.png", "image/png")], &["page"]);
	write_epub(
		&tmp.path().join("local/Cases/missing.epub"),
		Some(("OEBPS/content.opf", &missing)),
		&[("OEBPS/images/fallback.png", b"missing-fallback")],
	);
	let unlisted = divina_opf(&[("page", "text/page.xhtml", "application/xhtml+xml")], &["page"]);
	write_epub(
		&tmp.path().join("local/Cases/unlisted.epub"),
		Some(("OEBPS/content.opf", &unlisted)),
		&[
			("OEBPS/text/page.xhtml", br#"<html><body><img src="../images/unlisted.png"/></body></html>"#),
			("OEBPS/images/unlisted.png", b"unlisted"),
		],
	);
	let dangling = divina_opf(&[("page", "images/1.png", "image/png")], &["page", "missing"]);
	write_epub(
		&tmp.path().join("local/Cases/dangling.epub"),
		Some(("OEBPS/content.opf", &dangling)),
		&[("OEBPS/images/1.png", b"dangling")],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Cases").await;

	for key in ["prose.epub", "multi.epub", "missing.epub", "unlisted.epub", "dangling.epub"] {
		let chapter_id = chapter_id_of(&t.app, manga_id, key).await;
		let entries = epub_page_entries(&t.app, chapter_id).await;
		assert!(entries.is_empty(), "{key} must not be divina-compatible: {entries:?}");
	}
}

async fn divina_flags(db_path: &std::path::Path, manga_id: i64) -> Vec<(String, Option<bool>)> {
	let pool = common::connect_db(db_path).await;
	let flags = sqlx::query_as("SELECT key, epub_divina_compatible FROM chapter WHERE manga_id = ? ORDER BY key")
		.bind(manga_id)
		.fetch_all(&pool)
		.await
		.expect("divina flags");
	pool.close().await;
	flags
}

#[tokio::test]
async fn epub_divina_compatibility_is_persisted_and_refreshed() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let divina = divina_opf(&[("page", "images/1.png", "image/png")], &["page"]);
	write_epub(
		&tmp.path().join("local/Types/divina.epub"),
		Some(("OEBPS/content.opf", &divina)),
		&[("OEBPS/images/1.png", b"divina")],
	);
	let prose = divina_opf(
		&[("page", "text/page.xhtml", "application/xhtml+xml"), ("image", "images/1.png", "image/png")],
		&["page"],
	);
	write_epub(
		&tmp.path().join("local/Types/prose.epub"),
		Some(("OEBPS/content.opf", &prose)),
		&[
			(
				"OEBPS/text/page.xhtml",
				br#"<html><body>This is an ordinary reflowable prose page with too much text.<img src="../images/1.png"/></body></html>"#,
			),
			("OEBPS/images/1.png", b"prose"),
		],
	);
	write_cbz(&tmp.path().join("local/Types/images.cbz"), &[("1.png", b"cbz")]);
	write_page(&tmp.path().join("local/Types/fake.epub"), "1.png", b"directory");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Types").await;
	let expected = vec![
		("divina.epub".to_string(), Some(true)),
		("fake.epub".to_string(), None),
		("images.cbz".to_string(), None),
		("prose.epub".to_string(), Some(false)),
	];
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);

	let directory_id = chapter_id_of(&t.app, manga_id, "fake.epub").await;
	assert!(!epub_page_entries(&t.app, directory_id).await.is_empty());
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);

	let chapter_id = chapter_id_of(&t.app, manga_id, "divina.epub").await;
	let pool = common::connect_db(&t.db_path).await;
	sqlx::query("UPDATE chapter SET epub_divina_compatible = NULL WHERE id = ?")
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("reset divina flag");
	pool.close().await;
	assert!(!epub_page_entries(&t.app, chapter_id).await.is_empty());
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);

	write_epub(
		&t.tmp.path().join("local/Types/divina.epub"),
		Some(("OEBPS/content.opf", &prose)),
		&[
			(
				"OEBPS/text/page.xhtml",
				br#"<html><body>This is an ordinary reflowable prose page with too much text.<img src="../images/1.png"/></body></html>"#,
			),
			("OEBPS/images/1.png", b"now-prose"),
		],
	);
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let expected = vec![
		("divina.epub".to_string(), Some(false)),
		("fake.epub".to_string(), None),
		("images.cbz".to_string(), None),
		("prose.epub".to_string(), Some(false)),
	];
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);

	write_epub(
		&t.tmp.path().join("local/Types/divina.epub"),
		Some(("OEBPS/content.opf", &divina)),
		&[("OEBPS/images/1.png", b"divina-again")],
	);
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let expected = vec![
		("divina.epub".to_string(), Some(true)),
		("fake.epub".to_string(), None),
		("images.cbz".to_string(), None),
		("prose.epub".to_string(), Some(false)),
	];
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);

	let replacement = t.tmp.path().join("local/Types/replacement.epub");
	write_epub(
		&replacement,
		Some(("OEBPS/content.opf", &prose)),
		&[
			(
				"OEBPS/text/page.xhtml",
				br#"<html><body>This is an ordinary reflowable prose page with too much text.<img src="../images/1.png"/></body></html>"#,
			),
			("OEBPS/images/1.png", b"atomic-prose"),
		],
	);
	std::fs::rename(&replacement, t.tmp.path().join("local/Types/divina.epub")).expect("replace EPUB atomically");
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let expected = vec![
		("divina.epub".to_string(), Some(false)),
		("fake.epub".to_string(), None),
		("images.cbz".to_string(), None),
		("prose.epub".to_string(), Some(false)),
	];
	assert_eq!(divina_flags(&t.db_path, manga_id).await, expected);
}

#[tokio::test]
async fn nested_epub_chapter_reads_in_spine_order() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let opf = divina_opf(
		&[
			("p1", "p/1.png", "image/png"),
			("p2", "p/2.png", "image/png"),
			("p3", "p/3.png", "image/png"),
		],
		&["p3", "p1", "p2"],
	);
	write_epub(
		&tmp.path().join("local/Nest/vol1/x.epub"),
		Some(("OEBPS/content.opf", &opf)),
		&[
			("OEBPS/p/1.png", b"nest-x-1"),
			("OEBPS/p/2.png", b"nest-x-2"),
			("OEBPS/p/3.png", b"nest-x-3"),
		],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Nest").await;

	let chapters = chapters_of(&t.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["vol1/x.epub"]);
	assert_eq!(chapters[0].3, "vol1/x", "the archive display name drops the extension, keeps the path");

	let chapter_id = chapter_id_of(&t.app, manga_id, "vol1/x.epub").await;
	let entries = epub_page_entries(&t.app, chapter_id).await;
	assert_eq!(entries, ["OEBPS/p/3.png", "OEBPS/p/1.png", "OEBPS/p/2.png"]);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"nest-x-3");
}

#[tokio::test]
async fn oneshot_epub_opens_as_single_chapter() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let opf = divina_opf(
		&[("p1", "1.png", "image/png"), ("p2", "2.png", "image/png")],
		&["p2", "p1"],
	);
	write_epub(
		&tmp.path().join("local/oneshots/Single.epub"),
		Some(("OEBPS/content.opf", &opf)),
		&[("OEBPS/1.png", b"single-1"), ("OEBPS/2.png", b"single-2")],
	);
	write_page(&tmp.path().join("local/Normal/ch 1"), "1.png", b"normal-1-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	enable_oneshots(&t.app).await;

	let entries = browse_local(&t.app).await;
	assert!(
		entries.iter().any(|(k, title)| k == "oneshots/Single.epub" && title == "Single"),
		"the epub oneshot lists as its own manga titled by the file stem: {entries:?}"
	);

	let manga_id = register_local(&t.app, "oneshots/Single.epub").await;
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Single");
	assert_eq!(divina_flags(&t.db_path, manga_id).await, vec![("Single.epub".to_string(), Some(true))]);
	let chapters = chapters_of(&t.app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["Single.epub"], "an oneshot has exactly one chapter: the archive itself");

	let chapter_id = chapter_id_of(&t.app, manga_id, "Single.epub").await;
	let entries = epub_page_entries(&t.app, chapter_id).await;
	assert_eq!(entries, ["OEBPS/2.png", "OEBPS/1.png"]);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"single-2");

	// the cover is the single chapter's first spine page
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"single-2");
}

// ----- series.json and EPUB OPF metadata sources -----

const OPF_METADATA: &str = r#"<dc:title>Book Title</dc:title>
    <dc:creator>Writer One</dc:creator>
    <dc:creator>Writer Two</dc:creator>
    <dc:description>book desc</dc:description>
    <dc:subject>manga</dc:subject>
    <dc:subject>action</dc:subject>"#;

/// One-page EPUB chapter whose OPF carries the shared OPF_METADATA fragment
fn write_metadata_epub(path: &std::path::Path) {
	let opf = divina_opf_with_metadata(OPF_METADATA, &[("p1", "images/1.png", "image/png")], &["p1"]);
	write_epub(path, Some(("OEBPS/content.opf", &opf)), &[("OEBPS/images/1.png", b"epub-page-1")]);
}

async fn manga_json(app: &Router, manga_id: i64) -> serde_json::Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

#[tokio::test]
async fn series_json_maps_into_manga() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	// plural spellings, plus an unknown field that must be ignored
	let theta = root.join("Theta");
	std::fs::create_dir_all(&theta).expect("mkdir theta");
	std::fs::write(
		theta.join("series.json"),
		r#"{
  "title": "Theta Series",
  "authors": ["Author One", "Author Two"],
  "artists": ["Artist One"],
  "description": "theta desc",
  "genres": ["Action", "Drama"],
  "status": "COMPLETED",
  "publisher": "unknown fields are ignored"
}"#,
	)
	.expect("write series.json");
	write_page(&theta.join("ch 1"), "1.png", b"theta-1-1");
	// singular spellings in scalar shape, and a lowercase status
	let iota = root.join("Iota");
	std::fs::create_dir_all(&iota).expect("mkdir iota");
	std::fs::write(
		iota.join("series.json"),
		r#"{"title": "Iota Series", "author": "Solo Author", "artist": "Solo Artist", "status": "hiatus"}"#,
	)
	.expect("write series.json");
	write_page(&iota.join("ch 1"), "1.png", b"iota-1-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let theta_id = register_local(&t.app, "Theta").await;
	let body = manga_json(&t.app, theta_id).await;
	assert_eq!(body["title"], "Theta Series");
	assert_eq!(body["authors"], json!(["Author One", "Author Two"]));
	assert_eq!(body["artists"], json!(["Artist One"]));
	assert_eq!(body["description"], "theta desc");
	assert_eq!(body["tags"], json!(["Action", "Drama"]));
	assert_eq!(body["status"], "Completed");

	let iota_id = register_local(&t.app, "Iota").await;
	let body = manga_json(&t.app, iota_id).await;
	assert_eq!(body["title"], "Iota Series");
	assert_eq!(body["authors"], json!(["Solo Author"]));
	assert_eq!(body["artists"], json!(["Solo Artist"]));
	assert_eq!(body["status"], "Hiatus");
}

#[tokio::test]
async fn series_json_unknown_status_is_unset() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let kappa = tmp.path().join("local/Kappa");
	std::fs::create_dir_all(&kappa).expect("mkdir kappa");
	std::fs::write(kappa.join("series.json"), r#"{"title": "Kappa Series", "status": "DROPPED"}"#)
		.expect("write series.json");
	write_page(&kappa.join("ch 1"), "1.png", b"kappa-1-1");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let manga_id = register_local(&t.app, "Kappa").await;
	let body = manga_json(&t.app, manga_id).await;
	assert_eq!(body["title"], "Kappa Series");
	assert_eq!(body["status"], "Unknown", "an unrecognized status string counts as unset: {body}");
}

#[tokio::test]
async fn epub_opf_maps_into_manga() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_metadata_epub(&tmp.path().join("local/Lambda/ch 1.epub"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let manga_id = register_local(&t.app, "Lambda").await;
	let body = manga_json(&t.app, manga_id).await;
	assert_eq!(body["title"], "Book Title");
	assert_eq!(body["authors"], json!(["Writer One", "Writer Two"]));
	assert!(body["artists"].is_null(), "creators without artist roles leave artists unset: {body}");
	assert_eq!(body["description"], "book desc");
	assert_eq!(body["tags"], json!(["manga", "action"]));
	assert_eq!(body["status"], "Unknown");
}

#[tokio::test]
async fn comic_info_outranks_series_json() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	// top-level ComicInfo.xml vs series.json
	let mu = root.join("Mu");
	std::fs::create_dir_all(&mu).expect("mkdir mu");
	std::fs::write(mu.join("ComicInfo.xml"), COMIC_INFO_ALPHA).expect("write comicinfo");
	std::fs::write(mu.join("series.json"), r#"{"title": "Json Title", "status": "HIATUS"}"#)
		.expect("write series.json");
	write_page(&mu.join("ch 1"), "1.png", b"mu-1-1");
	// archive-carried ComicInfo.xml vs series.json
	let omicron = root.join("Omicron");
	std::fs::create_dir_all(&omicron).expect("mkdir omicron");
	std::fs::write(omicron.join("series.json"), r#"{"title": "Json Title", "status": "HIATUS"}"#)
		.expect("write series.json");
	write_cbz(
		&omicron.join("ch 1.cbz"),
		&[("1.png", b"omicron-1-1"), ("ComicInfo.xml", COMIC_INFO_DELTA.as_bytes())],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let mu_id = register_local(&t.app, "Mu").await;
	let body = manga_json(&t.app, mu_id).await;
	assert_eq!(body["title"], "Alpha Series", "top-level ComicInfo.xml wins: {body}");
	assert_eq!(body["status"], "Completed");

	let omicron_id = register_local(&t.app, "Omicron").await;
	let body = manga_json(&t.app, omicron_id).await;
	assert_eq!(body["title"], "Delta Series", "archive ComicInfo.xml wins: {body}");
	assert_eq!(body["status"], "Ongoing");
}

#[tokio::test]
async fn series_json_outranks_epub_opf() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let nu = tmp.path().join("local/Nu");
	std::fs::create_dir_all(&nu).expect("mkdir nu");
	std::fs::write(
		nu.join("series.json"),
		r#"{"title": "Json Title", "authors": ["Json Author"], "status": "ONGOING"}"#,
	)
	.expect("write series.json");
	write_metadata_epub(&nu.join("ch 1.epub"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let manga_id = register_local(&t.app, "Nu").await;
	let body = manga_json(&t.app, manga_id).await;
	assert_eq!(body["title"], "Json Title", "series.json wins over the OPF: {body}");
	assert_eq!(body["authors"], json!(["Json Author"]));
	assert_eq!(body["status"], "Ongoing");
}

#[tokio::test]
async fn broken_series_json_falls_through_to_epub_opf() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let xi = tmp.path().join("local/Xi");
	std::fs::create_dir_all(&xi).expect("mkdir xi");
	std::fs::write(xi.join("series.json"), b"{ not json").expect("write series.json");
	write_metadata_epub(&xi.join("ch 1.epub"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	// a broken sidecar warns and is skipped: shelving succeeds and the chain lands on the OPF
	let manga_id = register_local(&t.app, "Xi").await;
	let body = manga_json(&t.app, manga_id).await;
	assert_eq!(body["title"], "Book Title", "{body}");
	assert_eq!(body["authors"], json!(["Writer One", "Writer Two"]));
}

#[tokio::test]
async fn structured_metadata_preserves_commas_in_api() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	let mylar = root.join("MylarCommas");
	std::fs::create_dir_all(&mylar).expect("mkdir mylar");
	std::fs::write(
		mylar.join("series.json"),
		r#"{"metadata":{"name":"Mylar Commas","year":2024,"authors":[{"name":"Last, First"},{"name":"Artist, One","role":"penciller"}],"tags":["Tag, One","Tag Two"]}}"#,
	)
	.expect("write series.json");
	write_page(&mylar.join("ch 1"), "1.png", b"mylar-page");
	let opf = divina_opf_with_metadata(
		r#"<dc:title>OPF Commas</dc:title>
    <dc:creator>Last, First</dc:creator>
    <dc:creator xmlns:opf="http://www.idpf.org/2007/opf" opf:role="art">Artist, One</dc:creator>
    <dc:subject>Tag, One</dc:subject>
    <dc:subject> Tag, One </dc:subject>
    <dc:subject>Tag Two</dc:subject>"#,
		&[("p1", "images/1.png", "image/png")],
		&["p1"],
	);
	write_epub(
		&root.join("OpfCommas/ch 1.epub"),
		Some(("OEBPS/content.opf", &opf)),
		&[("OEBPS/images/1.png", b"opf-page")],
	);
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };

	let mylar_id = register_local(&t.app, "MylarCommas").await;
	let body = manga_json(&t.app, mylar_id).await;
	assert_eq!(body["authors"], json!(["Last, First"]));
	assert_eq!(body["artists"], json!(["Artist, One"]));
	assert_eq!(body["tags"], json!(["Tag, One", "Tag Two"]));

	let opf_id = register_local(&t.app, "OpfCommas").await;
	let body = manga_json(&t.app, opf_id).await;
	assert_eq!(body["authors"], json!(["Last, First"]));
	assert_eq!(body["artists"], json!(["Artist, One"]));
	assert_eq!(body["tags"], json!(["Tag, One", "Tag Two"]));
}

#[tokio::test]
async fn mylar_reading_direction_maps_to_aidoku_viewer() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	let local_dirs = vec![root.clone()];
	let zip = Arc::new(ZipCache::new());
	let cases = [
		(Some("LEFT_TO_RIGHT"), Viewer::LeftToRight),
		(Some("RIGHT_TO_LEFT"), Viewer::RightToLeft),
		(Some("VERTICAL"), Viewer::Vertical),
		(Some("WEBTOON"), Viewer::Webtoon),
		(Some("SIDEWAYS"), Viewer::Unknown),
		(None, Viewer::Unknown),
	];
	let mut actual = Vec::new();
	for (index, (reading_direction, _)) in cases.iter().enumerate() {
		let key = format!("Direction{index}");
		let dir = root.join(&key);
		std::fs::create_dir_all(&dir).expect("mkdir direction");
		let reading_direction = reading_direction
			.map(|direction| format!(r#""readingDirection":"{direction}","#))
			.unwrap_or_default();
		let json = format!(
			r#"{{"metadata":{{"name":"Direction {index}","year":2024,"volume":1,{reading_direction}"status":"Continuing"}}}}"#
		);
		std::fs::write(dir.join("series.json"), json).expect("write series.json");
		write_page(&dir.join("ch 1"), "1.png", b"direction-page");
		let update = manga_update(&local_dirs, &zip, &key).await.expect("manga update");
		actual.push(update.manga.viewer);
	}
	assert_eq!(actual, cases.map(|(_, viewer)| viewer));
}

#[tokio::test]
async fn epub_spine_direction_maps_to_aidoku_viewer() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	let local_dirs = vec![root.clone()];
	let zip = Arc::new(ZipCache::new());
	let cases = [
		(Some("rtl"), Viewer::RightToLeft),
		(Some("ltr"), Viewer::LeftToRight),
		(Some("vertical"), Viewer::Unknown),
		(None, Viewer::Unknown),
	];
	let mut actual = Vec::new();
	for (index, (direction, _)) in cases.iter().enumerate() {
		let key = format!("Spine{index}");
		let opf = String::from_utf8(divina_opf_with_metadata(
			OPF_METADATA,
			&[("p1", "images/1.png", "image/png")],
			&["p1"],
		))
		.expect("opf utf8");
		let opf = if let Some(direction) = direction {
			opf.replace("<spine>", &format!("<spine page-progression-direction=\"{direction}\">"))
		} else {
			opf
		};
		write_epub(
			&root.join(&key).join("ch 1.epub"),
			Some(("OEBPS/content.opf", opf.as_bytes())),
			&[("OEBPS/images/1.png", b"spine-page")],
		);
		let update = manga_update(&local_dirs, &zip, &key).await.expect("manga update");
		actual.push(update.manga.viewer);
	}
	assert_eq!(actual, cases.map(|(_, viewer)| viewer));
}

#[tokio::test]
async fn mylar_age_ratings_map_to_aidoku_content_ratings() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let root = tmp.path().join("local");
	let local_dirs = vec![root.clone()];
	let zip = Arc::new(ZipCache::new());
	let cases = [
		(Some("All"), ContentRating::Safe),
		(Some("9+"), ContentRating::Safe),
		(Some("12+"), ContentRating::Safe),
		(Some("15+"), ContentRating::Suggestive),
		(Some("17+"), ContentRating::Suggestive),
		(Some("Adult"), ContentRating::NSFW),
		(Some("MA15+"), ContentRating::Unknown),
		(None, ContentRating::Unknown),
	];
	let mut actual = Vec::new();
	for (index, (age_rating, _)) in cases.iter().enumerate() {
		let key = format!("Rating{index}");
		let dir = root.join(&key);
		std::fs::create_dir_all(&dir).expect("mkdir rating");
		let age_rating = age_rating.map(|rating| format!(r#""age_rating":"{rating}","#)).unwrap_or_default();
		let json = format!(
			r#"{{"metadata":{{"name":"Rating {index}","year":2024,"volume":1,{age_rating}"status":"Continuing"}}}}"#
		);
		std::fs::write(dir.join("series.json"), json).expect("write series.json");
		write_page(&dir.join("ch 1"), "1.png", b"rating-page");
		let update = manga_update(&local_dirs, &zip, &key).await.expect("manga update");
		actual.push(update.manga.content_rating);
	}
	assert_eq!(actual, cases.map(|(_, rating)| rating));
}

#[tokio::test]
async fn oneshot_epub_reads_opf_not_sidecar_series_json() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_metadata_epub(&tmp.path().join("local/oneshots/Solo.epub"));
	// the series.json next to the archive is a series sidecar, which the oneshots rule ignores —
	// if it were read it would outrank the OPF
	std::fs::write(
		tmp.path().join("local/oneshots/series.json"),
		r#"{"title": "Sidecar Json"}"#,
	)
	.expect("write series.json");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	enable_oneshots(&t.app).await;

	let manga_id = register_local(&t.app, "oneshots/Solo.epub").await;
	let body = manga_json(&t.app, manga_id).await;
	assert_eq!(body["title"], "Book Title", "{body}");
	assert_eq!(body["authors"], json!(["Writer One", "Writer Two"]));
}
