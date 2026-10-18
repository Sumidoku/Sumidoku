mod common;

use axum::http::StatusCode;
use common::{TestApp, request, test_app};

async fn scan_and_source_ids(app: &axum::Router) -> Vec<(String, i64)> {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.map(|s| {
			(
				s["key"].as_str().expect("source key").to_string(),
				s["id"].as_i64().expect("source id"),
			)
		})
		.collect()
}

#[tokio::test]
async fn home_layout_serialized() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let ids = scan_and_source_ids(&app).await;
	let source_id = ids[0].1;

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);

	// fixture 主页固定 7 个组件，按序覆盖 6 种 kind（无 ImageScroller）
	let components = body["components"].as_array().expect("components array");
	assert_eq!(components.len(), 7);
	let kinds: Vec<&str> = components
		.iter()
		.map(|c| {
			let obj = c["value"].as_object().expect("component value object");
			assert_eq!(obj.len(), 1, "value 应为单键的外部标签枚举");
			obj.keys().next().expect("kind key").as_str()
		})
		.collect();
	assert_eq!(
		kinds,
		["BigScroller", "MangaChapterList", "MangaList", "MangaList", "Scroller", "Filters", "Links"]
	);

	let big = &components[0];
	assert_eq!(big["title"], "Big Scroller");
	let entries = big["value"]["BigScroller"]["entries"].as_array().expect("big entries");
	assert_eq!(entries.len(), 20);
	assert_eq!(entries[0]["title"], "Manga 1");
	assert_eq!(entries[0]["cover"], "https://aidoku.app/images/icon.png");
	assert_eq!(big["value"]["BigScroller"]["auto_scroll_interval"], 10.0);

	let chapters = &components[1]["value"]["MangaChapterList"];
	let entries = chapters["entries"].as_array().expect("chapter entries");
	assert_eq!(entries.len(), 3);
	assert_eq!(entries[0]["manga"]["key"], "1");
	assert_eq!(entries[0]["chapter"]["chapter_number"], 1.0);
	assert_eq!(entries[0]["chapter"]["date_uploaded"], 1692318525);

	let list = &components[2]["value"]["MangaList"];
	assert_eq!(list["ranking"], false);
	let entries = list["entries"].as_array().expect("list entries");
	assert_eq!(entries.len(), 2);
	assert_eq!(entries[0]["title"], "Manga 1");
	assert_eq!(entries[0]["value"]["Manga"]["key"], "1");

	let paged = &components[3];
	assert_eq!(paged["title"], "Manga List (Paged, Ranking)");
	let paged = &paged["value"]["MangaList"];
	assert_eq!(paged["ranking"], true);
	assert_eq!(paged["page_size"], 3);
	assert_eq!(paged["entries"].as_array().expect("paged entries").len(), 8);

	let scroller = &components[4]["value"]["Scroller"];
	let entries = scroller["entries"].as_array().expect("scroller entries");
	assert_eq!(entries.len(), 20);
	assert_eq!(entries[0]["image_url"], "https://aidoku.app/images/icon.png");
	assert_eq!(entries[0]["value"]["Manga"]["key"], "1");

	let filters = components[5]["value"]["Filters"].as_array().expect("filters");
	assert_eq!(filters.len(), 7);
	assert_eq!(filters[0]["title"], "Action");
	assert!(filters[0]["values"].is_null());

	let links = components[6]["value"]["Links"].as_array().expect("links");
	assert_eq!(links.len(), 3);
	assert_eq!(links[0]["value"], serde_json::json!({"Url": "https://aidoku.app"}));
	assert_eq!(links[1]["value"]["Manga"]["key"], "1");
	assert_eq!(links[2]["value"]["Listing"]["id"], "listing");

	// 未安装的源 404 not_found
	let (status, body) = request(&app, "GET", "/api/v1/sources/999/home", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn home_unsupported_source_404() {
	let TestApp { app, tmp, .. } = test_app().await;
	// mutation fixture 只导出 Source + get_page_description，没有 get_home
	common::install_fixture_source(&tmp, "en.mutation", "Mutation Source", &common::mutation_wasm_path());
	let ids = scan_and_source_ids(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.mutation")
		.map(|(_, id)| *id)
		.expect("mutation source id");

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");
}
