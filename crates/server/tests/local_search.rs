//! Local-source search supports title substring matching plus author/artist/tag jumps,
//! using the same Aidoku-style filter values that MangaDetailPage sends for online sources.
//! Visibility and filter predicates run in SQL; natural-sort ordering and pagination over the
//! filtered set stay server-side.

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

async fn local_app_seeded(seed: impl FnOnce(&std::path::Path)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed(&tmp.path().join("local"));
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	local_scan_and_settle(&app, None).await;
	TestApp::new(app, tmp, db_path)
}

async fn local_app() -> TestApp {
	local_app_seeded(seed_tree).await
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

async fn search_page(
	app: &axum::Router,
	source_id: i64,
	query: Option<&str>,
	filters: serde_json::Value,
	page: i32,
) -> (Vec<serde_json::Value>, bool) {
	let body = json!({
		"query": query,
		"filters": filters,
		"page": page,
	});
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(body),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entries = body["entries"].as_array().expect("entries array").clone();
	(entries, body["hasNextPage"].as_bool().expect("hasNextPage"))
}

async fn search(app: &axum::Router, source_id: i64, query: Option<&str>, filters: serde_json::Value) -> Vec<serde_json::Value> {
	search_page(app, source_id, query, filters, 1).await.0
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

#[tokio::test]
async fn search_paginates_with_has_next() {
	// 25 titles against the fixed page size of 20: natural sort puts "Series 2" ahead of "Series 10",
	// and page 2 carries the 5 leftovers
	let TestApp { app, .. } = local_app_seeded(|root| {
		for n in 1..=25 {
			write_page(&root.join(format!("Series {n}/ch1")), "1.png", b"page");
		}
	})
	.await;
	let id = local_source_id(&app).await;
	let (first, has_next) = search_page(&app, id, None, json!([]), 1).await;
	assert_eq!(first.len(), 20, "page 1 fills the page size");
	assert!(has_next);
	let first_titles = titles(&first);
	assert_eq!(first_titles[..2], ["Series 1", "Series 2"], "natural order, not lexicographic");
	let (second, has_next) = search_page(&app, id, None, json!([]), 2).await;
	assert_eq!(titles(&second), vec!["Series 21", "Series 22", "Series 23", "Series 24", "Series 25"]);
	assert!(!has_next);
}

#[tokio::test]
async fn search_filter_applies_before_pagination() {
	// 22 genre matches among 32 indexed titles: the SQL filter runs before the page slice, so page 1 is
	// full (20 matches) and page 2 holds the 2 remaining matches — not the instance paginated before filtering
	let TestApp { app, .. } = local_app_seeded(|root| {
		for n in 1..=10 {
			write_page(&root.join(format!("Other {n}/ch1")), "1.png", b"page");
		}
		for n in 1..=22 {
			let dir = root.join(format!("ZMatch {n}"));
			std::fs::create_dir_all(&dir).expect("mkdir match");
			std::fs::write(dir.join("series.json"), format!(r#"{{"title":"ZMatch {n}","genres":["Action"]}}"#))
				.expect("write series.json");
			write_page(&dir.join("ch1"), "1.png", b"page");
		}
	})
	.await;
	let id = local_source_id(&app).await;
	let filters = json!([{"Select": {"id": "genre", "value": "Action"}}]);
	let (first, has_next) = search_page(&app, id, None, filters.clone(), 1).await;
	assert_eq!(first.len(), 20, "the genre match alone fills page 1");
	assert!(has_next);
	let first_titles = titles(&first);
	assert_eq!(first_titles[..2], ["ZMatch 1", "ZMatch 2"]);
	let (second, has_next) = search_page(&app, id, None, filters, 2).await;
	assert_eq!(titles(&second), vec!["ZMatch 21", "ZMatch 22"]);
	assert!(!has_next);
}

#[tokio::test]
async fn search_matches_a_title_typed_exactly_with_non_ascii_capitals() {
	// SQLite's LOWER and LIKE fold ASCII only, so the query must reach LIKE as typed: lowercasing it in
	// Rust turns the capital in "Берсерк" or "École" into a letter the stored title does not carry
	let TestApp { app, .. } = local_app_seeded(|root| {
		write_page(&root.join("Берсерк/ch1"), "1.png", b"page");
		write_page(&root.join("École Days/ch1"), "1.png", b"page");
	})
	.await;
	let id = local_source_id(&app).await;
	assert_eq!(titles(&search(&app, id, Some("Берсерк"), json!([])).await), vec!["Берсерк"]);
	assert_eq!(titles(&search(&app, id, Some("École"), json!([])).await), vec!["École Days"]);
	// ASCII still folds both ways
	assert_eq!(titles(&search(&app, id, Some("cole DAYS"), json!([])).await), vec!["École Days"]);
}
