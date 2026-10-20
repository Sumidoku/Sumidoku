//! Deep-link endpoint (issue #137): POST /api/v1/sources/{id}/deep-link hands the URL to the source's
//! handle_deep_link; Manga/Chapter variants are resolved server-side (registered when needed via the
//! same flow as register_manga) into internal mangaId/chapterId, while Listing variants are returned
//! as-is for frontend navigation. The fixture source lives in
//! crates/wasm/tests/fixtures/deeplink-src (no_std cdylib); rebuild with
//! `cd crates/wasm/tests/fixtures/deeplink-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_deeplink.wasm
//! to crates/wasm/tests/fixtures/deeplink/main.wasm.
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, baseurl_wasm_path, deeplink_wasm_path, install_fixture_source, request};
use serde_json::{Value, json};
use sumidoku_server::build_app;

/// App with two sources: deeplink (has DeepLinkHandler) + baseurl (no export, the 404 unsupported control)
async fn dual_source_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.deeplink", "DeepLink Source", &deeplink_wasm_path());
	install_fixture_source(&tmp, "en.dynbase", "DynBase Source", &baseurl_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn source_id_by_key(app: &Router, key: &str) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("fixture source")
}

async fn deep_link(app: &Router, source_id: i64, url: &str) -> (StatusCode, Value) {
	request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/deep-link"),
		Some(json!({"url": url})),
	)
	.await
}

#[tokio::test]
async fn deep_link_resolves_manga_and_chapter_ids() {
	let app = dual_source_app().await;
	let source_id = source_id_by_key(&app.app, "en.deeplink").await;

	let (status, body) = deep_link(&app.app, source_id, "https://deeplink.fixture/manga/m1").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["result"];
	assert_eq!(result["mangaKey"], "m1");
	assert!(result["chapterKey"].is_null(), "{body}");
	assert!(result["listing"].is_null(), "{body}");
	let manga_id = result["mangaId"].as_i64().expect("mangaId resolved");
	assert!(result["chapterId"].is_null(), "{body}");

	// Registration fetches details immediately: the title comes from the fixture's get_manga_update
	let (status, manga) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{manga}");
	assert_eq!(manga["title"], "Deep Manga m1");
	assert_eq!(manga["inLibrary"], false);

	// Resolving again hits the existing row; the id is unchanged
	let (status, body) = deep_link(&app.app, source_id, "https://deeplink.fixture/manga/m1").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["result"]["mangaId"], manga_id);

	let (status, body) = deep_link(&app.app, source_id, "https://deeplink.fixture/manga/m1/chapter/c2").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["result"];
	assert_eq!(result["mangaKey"], "m1");
	assert_eq!(result["chapterKey"], "c2");
	assert_eq!(result["mangaId"], manga_id);
	let chapter_id = result["chapterId"].as_i64().expect("chapterId resolved");

	let (status, chapters) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{chapters}");
	let chapter = chapters
		.as_array()
		.expect("chapters array")
		.iter()
		.find(|c| c["key"] == "c2")
		.expect("c2 chapter");
	assert_eq!(chapter["id"], chapter_id);
}

#[tokio::test]
async fn deep_link_returns_listing_and_null_for_unmatched() {
	let app = dual_source_app().await;
	let source_id = source_id_by_key(&app.app, "en.deeplink").await;

	let (status, body) = deep_link(&app.app, source_id, "https://deeplink.fixture/listing/latest").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["result"];
	assert_eq!(result["listing"]["id"], "latest");
	assert!(result["mangaKey"].is_null(), "{body}");
	assert!(result["mangaId"].is_null(), "{body}");

	let (status, body) = deep_link(&app.app, source_id, "https://other.fixture/manga/m1").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body["result"].is_null(), "{body}");

	let (status, body) = deep_link(&app.app, source_id, "https://deeplink.fixture/about").await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body["result"].is_null(), "{body}");
}

#[tokio::test]
async fn deep_link_rejects_unsupported_and_bad_input() {
	let app = dual_source_app().await;
	let dynbase_id = source_id_by_key(&app.app, "en.dynbase").await;

	let (status, body) = deep_link(&app.app, dynbase_id, "https://deeplink.fixture/manga/m1").await;
	assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
	assert_eq!(body["error"]["code"], "unsupported");

	let deeplink_id = source_id_by_key(&app.app, "en.deeplink").await;
	let (status, body) = deep_link(&app.app, deeplink_id, "").await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	let (status, _) = deep_link(&app.app, 9999, "https://deeplink.fixture/manga/m1").await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}
