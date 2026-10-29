//! One visibility rule for the local read model: search and every home listing hide the same manga —
//! exclusions as a Unicode-lowercase literal substring of the key, and a relative path mirrored under several
//! roots listing once, from the lowest root
mod common;

use axum::http::StatusCode;
use common::{TestApp, base_config, request, source_by_key};
use serde_json::json;
use sumidoku_server::build_app;

fn write_page(dir: &std::path::Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

async fn app_with_roots(roots: &[&std::path::Path]) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dirs = Some(roots.iter().map(|p| p.to_path_buf()).collect());
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn local_id(app: &axum::Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "local.local")["id"].as_i64().expect("local source id")
}

async fn search_keys(app: &axum::Router, id: i64) -> Vec<String> {
	let (status, body) = request(app, "POST", &format!("/api/v1/sources/{id}/search"), Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let mut keys: Vec<String> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["key"].as_str().expect("key").to_string())
		.collect();
	keys.sort();
	keys
}

async fn all_listing_keys(app: &axum::Router, id: i64) -> Vec<String> {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{id}/listings/all?page=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let mut keys: Vec<String> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["key"].as_str().expect("key").to_string())
		.collect();
	keys.sort();
	keys
}

async fn set_exclusions(app: &axum::Router, id: i64, exclusions: &[&str]) {
	let (status, body) =
		request(app, "PUT", &format!("/api/v1/sources/{id}/settings"), Some(json!({"exclusions": exclusions}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_mirror_between_non_zero_roots_lists_once() {
	let (a, b, c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
	write_page(&a.path().join("Alpha/ch 1"), "1.png", b"a");
	write_page(&b.path().join("Shared/ch 1"), "1.png", b"b");
	write_page(&c.path().join("Shared/ch 1"), "1.png", b"c");
	let t = app_with_roots(&[a.path(), b.path(), c.path()]).await;
	let id = local_id(&t.app).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	assert_eq!(search_keys(&t.app, id).await, ["1:Shared", "Alpha"]);
	assert_eq!(all_listing_keys(&t.app, id).await, ["1:Shared", "Alpha"]);
}

#[tokio::test]
async fn the_all_listing_dedups_a_mirror_like_search() {
	let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
	write_page(&a.path().join("Shared/ch 1"), "1.png", b"a");
	write_page(&b.path().join("Shared/ch 1"), "1.png", b"b");
	let t = app_with_roots(&[a.path(), b.path()]).await;
	let id = local_id(&t.app).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	assert_eq!(search_keys(&t.app, id).await, ["Shared"]);
	assert_eq!(all_listing_keys(&t.app, id).await, ["Shared"]);
}

#[tokio::test]
async fn a_non_ascii_exclusion_hides_from_listings_like_search() {
	let a = tempfile::tempdir().unwrap();
	write_page(&a.path().join("Ärger/ch 1"), "1.png", b"a");
	write_page(&a.path().join("Beta/ch 1"), "1.png", b"b");
	let t = app_with_roots(&[a.path()]).await;
	let id = local_id(&t.app).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	set_exclusions(&t.app, id, &["ärger"]).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	assert_eq!(search_keys(&t.app, id).await, ["Beta"]);
	assert_eq!(all_listing_keys(&t.app, id).await, ["Beta"]);
}

#[tokio::test]
async fn an_underscore_exclusion_is_literal_in_listings_like_search() {
	let a = tempfile::tempdir().unwrap();
	write_page(&a.path().join("raw-1/ch 1"), "1.png", b"a");
	write_page(&a.path().join("raw_1/ch 1"), "1.png", b"b");
	let t = app_with_roots(&[a.path()]).await;
	let id = local_id(&t.app).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	set_exclusions(&t.app, id, &["raw_1"]).await;
	common::local_scan_and_settle(&t.app, Some(id)).await;
	assert_eq!(search_keys(&t.app, id).await, ["raw-1"]);
	assert_eq!(all_listing_keys(&t.app, id).await, ["raw-1"]);
}
