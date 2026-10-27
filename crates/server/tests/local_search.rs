//! Local-source search supports title substring matching plus author/artist/tag jumps,
//! using the same Aidoku-style filter values that MangaDetailPage sends for online sources.

mod common;

use axum::http::StatusCode;
use common::{TestApp, base_config, local_scan_and_settle, request};
use serde_json::json;
use sumidoku_server::build_app;

fn write_page(dir: &std::path::Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

fn seed_tree(root: &std::path::Path) {
	let alpha = root.join("Alpha");
	std::fs::create_dir_all(&alpha).expect("mkdir alpha");
	std::fs::write(
		alpha.join("series.json"),
		r#"{"title":"Alpha Title","authors":["Author A","Shared Author"],"artists":["Artist A"],"genres":["Action","Adventure"]}"#,
	)
	.expect("write series.json");
	write_page(&alpha.join("ch1"), "1.png", b"alpha-1");

	let beta = root.join("Beta");
	std::fs::create_dir_all(&beta).expect("mkdir beta");
	std::fs::write(
		beta.join("series.json"),
		r#"{"title":"Beta Title","authors":["Author B"],"artists":["Shared Artist"],"genres":["Action","Drama"]}"#,
	)
	.expect("write series.json");
	write_page(&beta.join("ch1"), "1.png", b"beta-1");

	let gamma = root.join("Gamma");
	std::fs::create_dir_all(&gamma).expect("mkdir gamma");
	std::fs::write(
		gamma.join("series.json"),
		r#"{"title":"Gamma Title","authors":["Shared Author"],"artists":["Artist C"],"genres":["Comedy"]}"#,
	)
	.expect("write series.json");
	write_page(&gamma.join("ch1"), "1.png", b"gamma-1");
}

async fn local_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_tree(&tmp.path().join("local"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	local_scan_and_settle(&app, None).await;
	TestApp { app, tmp, db_path }
}

async fn local_source_id(app: &axum::Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == "local.local")
		.map(|s| s["id"].as_i64().expect("local id"))
		.expect("local.local source")
}

async fn search(app: &axum::Router, source_id: i64, query: Option<&str>, filters: serde_json::Value) -> Vec<serde_json::Value> {
	let body = json!({
		"query": query,
		"filters": filters,
		"page": 1,
	});
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(body),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["entries"].as_array().expect("entries array").clone()
}

fn titles(entries: &[serde_json::Value]) -> Vec<&str> {
	entries.iter().map(|e| e["title"].as_str().expect("title")).collect()
}

#[tokio::test]
async fn search_by_title_substring() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	let entries = search(&app, id, Some("lpha"), json!([])).await;
	assert_eq!(titles(&entries), vec!["Alpha Title"]);
}

#[tokio::test]
async fn search_by_author_exact() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	let entries = search(&app, id, None, json!([{"Text": {"id": "author", "value": "Author A"}}])).await;
	assert_eq!(titles(&entries), vec!["Alpha Title"]);
}

#[tokio::test]
async fn search_by_artist_exact() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	let entries = search(&app, id, None, json!([{"Text": {"id": "artist", "value": "Shared Artist"}}])).await;
	assert_eq!(titles(&entries), vec!["Beta Title"]);
}

#[tokio::test]
async fn search_by_tag_exact() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	let entries = search(&app, id, None, json!([{"Select": {"id": "genre", "value": "Action"}}])).await;
	let mut got = titles(&entries);
	got.sort();
	assert_eq!(got, vec!["Alpha Title", "Beta Title"]);
}

#[tokio::test]
async fn search_query_matches_author_and_tag_substring() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	// "Author" matches authors; "Comedy" matches tags.
	let entries = search(&app, id, Some("Author B"), json!([])).await;
	assert_eq!(titles(&entries), vec!["Beta Title"]);
	let entries = search(&app, id, Some("omed"), json!([])).await;
	assert_eq!(titles(&entries), vec!["Gamma Title"]);
}

#[tokio::test]
async fn search_by_multiple_tags() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	let entries = search(
		&app,
		id,
		None,
		json!([{"MultiSelect": {"id": "genre", "included": ["Action", "Adventure"], "excluded": []}}]),
	)
	.await;
	assert_eq!(titles(&entries), vec!["Alpha Title"]);
}

#[tokio::test]
async fn search_combines_query_and_filters() {
	let TestApp { app, .. } = local_app().await;
	let id = local_source_id(&app).await;
	// "Title" matches all; author filter narrows to Beta.
	let entries = search(&app, id, Some("Title"), json!([{"Text": {"id": "author", "value": "Author B"}}])).await;
	assert_eq!(titles(&entries), vec!["Beta Title"]);
}
