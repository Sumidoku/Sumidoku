//! Built-in local source home/listing semantics: latest-added vs latest-updates are
//! distinguished by first-indexed time and subsequent content-change time.
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, local_scan_and_settle, request, source_by_key};
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, SystemTime};
use sumidoku_server::build_app;

fn write_page(dir: &Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

fn set_dir_mtime(path: &Path, at: SystemTime) {
	let secs = at
		.duration_since(SystemTime::UNIX_EPOCH)
		.expect("mtime after epoch")
		.as_secs() as i64;
	let ft = filetime::FileTime::from_unix_time(secs, 0);
	filetime::set_file_mtime(path, ft).expect("set dir mtime");
}

async fn local_app_with_tree<F: FnOnce(&Path)>(seed: F) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed(&tmp.path().join("local"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn local_source_id(app: &Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "local.local")["id"]
		.as_i64()
		.expect("local source id")
}

fn listing_titles(body: &Value) -> Vec<String> {
	body["entries"]
		.as_array()
		.expect("entries array")
		.iter()
		.map(|e| e["title"].as_str().unwrap_or("").to_string())
		.collect()
}

fn home_section_titles(body: &Value, section: &str) -> Vec<String> {
	body["components"]
		.as_array()
		.expect("components array")
		.iter()
		.find(|c| c["title"].as_str() == Some(section))
		.map(|c| {
			let value = &c["value"];
			if let Some(list) = value["MangaChapterList"]["entries"].as_array() {
				list.iter()
					.map(|e| e["manga"]["title"].as_str().unwrap_or("").to_string())
					.collect()
			} else if let Some(scroller) = value["Scroller"]["entries"].as_array() {
				scroller
					.iter()
					.map(|e| e["title"].as_str().unwrap_or("").to_string())
					.collect()
			} else {
				Vec::new()
			}
		})
		.unwrap_or_default()
}

#[tokio::test]
async fn newly_added_manga_appears_in_latest_added_not_latest_updates() {
	let TestApp { app, tmp: _tmp, db_path: _ } = local_app_with_tree(|root| {
		let alpha = root.join("Alpha");
		write_page(&alpha.join("ch 1"), "1.png", b"alpha-1");
		let beta = root.join("Beta");
		write_page(&beta.join("ch 1"), "1.png", b"beta-1");
	})
	.await;
	let source_id = local_source_id(&app).await;
	local_scan_and_settle(&app, Some(source_id)).await;

	let (status, home) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	let latest_added = home_section_titles(&home, "Latest added");
	assert_eq!(latest_added.len(), 2, "both new manga appear in latest-added");
	assert!(latest_added.contains(&"Alpha".into()));
	assert!(latest_added.contains(&"Beta".into()));

	let latest_updates = home_section_titles(&home, "Latest updates");
	assert!(
		latest_updates.is_empty(),
		"newly added manga with no content changes must not appear in latest-updates: {latest_updates:?}"
	);

	let (status, listing) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest-added?page=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let titles = listing_titles(&listing);
	assert_eq!(titles.len(), 2);

	let (status, listing) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest-updates?page=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(listing["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn adding_a_chapter_bumps_manga_to_latest_updates() {
	let TestApp { app, tmp, db_path: _ } = local_app_with_tree(|root| {
		let alpha = root.join("Alpha");
		write_page(&alpha.join("ch 1"), "1.png", b"alpha-1");
		// Stamp the parent after writing so the first scan records a known baseline.
		set_dir_mtime(&alpha, SystemTime::now());
	})
	.await;
	let source_id = local_source_id(&app).await;
	local_scan_and_settle(&app, Some(source_id)).await;

	// Add a new chapter and bump the parent mtime so the second scan notices.
	let alpha = tmp.path().join("local").join("Alpha");
	write_page(&alpha.join("ch 2"), "1.png", b"alpha-2");
	set_dir_mtime(&alpha, SystemTime::now() + Duration::from_secs(2));
	local_scan_and_settle(&app, Some(source_id)).await;

	let (status, home) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	let latest_updates = home_section_titles(&home, "Latest updates");
	assert_eq!(latest_updates, vec!["Alpha"]);

	let (status, listing) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest-updates?page=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let titles = listing_titles(&listing);
	assert_eq!(titles, vec!["Alpha"]);
}

#[tokio::test]
async fn removing_a_chapter_bumps_manga_to_latest_updates() {
	let TestApp { app, tmp, db_path: _ } = local_app_with_tree(|root| {
		let alpha = root.join("Alpha");
		write_page(&alpha.join("ch 1"), "1.png", b"alpha-1");
		write_page(&alpha.join("ch 2"), "1.png", b"alpha-2");
		set_dir_mtime(&alpha, SystemTime::now());
	})
	.await;
	let source_id = local_source_id(&app).await;
	local_scan_and_settle(&app, Some(source_id)).await;

	// Remove a chapter and bump the parent mtime.
	let alpha = tmp.path().join("local").join("Alpha");
	std::fs::remove_dir_all(alpha.join("ch 2")).expect("remove ch 2");
	set_dir_mtime(&alpha, SystemTime::now() + Duration::from_secs(2));
	local_scan_and_settle(&app, Some(source_id)).await;

	let (status, home) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	let latest_updates = home_section_titles(&home, "Latest updates");
	assert_eq!(latest_updates, vec!["Alpha"]);
}

#[tokio::test]
async fn unchanged_manga_stays_out_of_latest_updates() {
	let TestApp { app, tmp: _tmp, db_path: _ } = local_app_with_tree(|root| {
		let alpha = root.join("Alpha");
		write_page(&alpha.join("ch 1"), "1.png", b"alpha-1");
		set_dir_mtime(&alpha, SystemTime::now());
	})
	.await;
	let source_id = local_source_id(&app).await;
	local_scan_and_settle(&app, Some(source_id)).await;

	// Re-scan with no filesystem changes.
	local_scan_and_settle(&app, Some(source_id)).await;

	let (status, home) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	let latest_updates = home_section_titles(&home, "Latest updates");
	assert!(
		latest_updates.is_empty(),
		"unchanged manga must not appear in latest-updates after a no-op scan"
	);
}

#[tokio::test]
async fn latest_added_orders_by_created_at() {
	let TestApp { app, tmp: _tmp, db_path } = local_app_with_tree(|root| {
		write_page(&root.join("Alpha").join("ch 1"), "1.png", b"alpha-1");
		write_page(&root.join("Beta").join("ch 1"), "1.png", b"beta-1");
	})
	.await;
	let source_id = local_source_id(&app).await;
	local_scan_and_settle(&app, Some(source_id)).await;

	let pool = common::connect_db(&db_path).await;
	let alpha_id: i64 = sqlx::query_scalar("SELECT id FROM manga WHERE key = 'Alpha'")
		.fetch_one(&pool)
		.await
		.expect("alpha id");
	let beta_id: i64 = sqlx::query_scalar("SELECT id FROM manga WHERE key = 'Beta'")
		.fetch_one(&pool)
		.await
		.expect("beta id");
	// Force Alpha to look older than Beta.
	sqlx::query("UPDATE manga SET created_at = 100 WHERE id = ?")
		.bind(alpha_id)
		.execute(&pool)
		.await
		.expect("age alpha");
	sqlx::query("UPDATE manga SET created_at = 200 WHERE id = ?")
		.bind(beta_id)
		.execute(&pool)
		.await
		.expect("age beta");
	pool.close().await;

	let (status, listing) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest-added?page=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let titles = listing_titles(&listing);
	assert_eq!(titles, vec!["Beta", "Alpha"]);
}
