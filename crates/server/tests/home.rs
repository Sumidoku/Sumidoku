//! Home endpoint contract. The fixture source for the field pass-through cases lives in
//! crates/wasm/tests/fixtures/home-src (no_std cdylib); rebuild with
//! `cd crates/wasm/tests/fixtures/home-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_home.wasm
//! to crates/wasm/tests/fixtures/home/main.wasm.
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
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.example-source")
		.map(|(_, id)| *id)
		.expect("example source id");

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);

	// the fixture home has a fixed 7 components covering 6 kinds in order (no ImageScroller)
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

	// an uninstalled source 404s with not_found
	let (status, body) = request(&app, "GET", "/api/v1/sources/999/home", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

/// The home fixture sets every easily-dropped component field and asserts the endpoint passes them
/// through verbatim: ImageScroller's subtitle/width/height/auto_scroll_interval, BigScroller's
/// tags/content_rating, Scroller's subtitle, MangaList/MangaChapterList's page_size, and the tags of
/// the Manga embedded in a MangaList link
#[tokio::test]
async fn home_component_fields_pass_through() {
	let TestApp { app, tmp, .. } = test_app().await;
	common::install_fixture_source(&tmp, "en.home", "Home Source", &common::home_wasm_path());
	let ids = scan_and_source_ids(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.home")
		.map(|(_, id)| *id)
		.expect("home source id");

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	let components = body["components"].as_array().expect("components array");
	assert_eq!(components.len(), 6);

	let banners = &components[0]["value"]["ImageScroller"];
	assert_eq!(banners["auto_scroll_interval"], 2.0);
	assert_eq!(banners["width"], 600);
	assert_eq!(banners["height"], 280);
	let links = banners["links"].as_array().expect("banner links");
	assert_eq!(links.len(), 6);
	assert_eq!(links[0]["subtitle"], "Banner sub 1");

	let big = &components[1]["value"]["BigScroller"];
	let entries = big["entries"].as_array().expect("big entries");
	assert_eq!(entries.len(), 3);
	assert_eq!(entries[0]["tags"], serde_json::json!(["Action", "Drama"]));
	assert_eq!(entries[0]["content_rating"], "NSFW");
	assert_eq!(entries[1]["content_rating"], "Suggestive");
	assert_eq!(entries[2]["content_rating"], "Safe");

	let scroller = &components[2]["value"]["Scroller"];
	let entries = scroller["entries"].as_array().expect("scroller entries");
	assert_eq!(entries[0]["subtitle"], "Scroll sub 1");

	let paged = &components[3]["value"]["MangaList"];
	assert_eq!(paged["page_size"], 2);
	let entries = paged["entries"].as_array().expect("paged entries");
	assert_eq!(entries.len(), 5);
	assert_eq!(entries[0]["value"]["Manga"]["tags"], serde_json::json!(["Action", "Fantasy"]));

	let plain = &components[4]["value"]["MangaList"];
	assert!(plain["page_size"].is_null());

	let chapters = &components[5]["value"]["MangaChapterList"];
	assert_eq!(chapters["page_size"], 2);
	assert_eq!(chapters["entries"].as_array().expect("chapter entries").len(), 3);
}

#[tokio::test]
async fn home_unsupported_source_501() {	let TestApp { app, tmp, .. } = test_app().await;
	// mutation fixture only exports Source + get_page_description, no get_home
	common::install_fixture_source(&tmp, "en.mutation", "Mutation Source", &common::mutation_wasm_path());
	let ids = scan_and_source_ids(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.mutation")
		.map(|(_, id)| *id)
		.expect("mutation source id");

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], "unsupported");
}
