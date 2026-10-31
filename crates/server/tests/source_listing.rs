//! listing pass-through and the source config DTO (issue #138):
//! GET /sources/{id}/listings/{listing_id} passes the real listing (id, name, kind) to
//! get_manga_list; the installed-source DTO exposes the manifest config section.
//! The fixture source lives in crates/wasm/tests/fixtures/listing-src (no_std cdylib); rebuild with
//! `cd crates/wasm/tests/fixtures/listing-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_listing.wasm
//! to crates/wasm/tests/fixtures/listing/main.wasm.
mod common;

use axum::http::StatusCode;
use common::{
	TestApp, base_config, baseurl_wasm_path, install_fixture_source, install_fixture_source_with_doc,
	listing_wasm_path, request,
};
use serde_json::json;
use sumidoku_server::build_app;

/// App with two sources: the listing echo source (with listings and a config section) + the baseurl control source (no config section)
async fn listing_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let doc = json!({
		"info": {"id": "en.listing", "name": "Listing Source", "version": 1, "languages": ["en"]},
		"listings": [
			{"id": "hot", "name": "Hot Updates", "kind": 1},
			{"id": "fresh"},
			"bare"
		],
		"config": {
			"hidesFiltersWhileSearching": true,
			"supportsTagSearch": true,
			"supportsArtistSearch": true,
			"supportsAuthorSearch": true,
			"maximumParallelRequests": 2
		}
	});
	install_fixture_source_with_doc(&tmp, &doc, &listing_wasm_path());
	install_fixture_source(&tmp, "en.dynbase", "DynBase Source", &baseurl_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp::new(app, tmp, db_path)
}

async fn install_and_ids(app: &TestApp) -> (i64, i64) {
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	let find = |key: &str| {
		sources
			.iter()
			.find(|s| s["key"] == key)
			.and_then(|s| s["id"].as_i64())
			.expect("fixture source")
	};
	(find("en.listing"), find("en.dynbase"))
}

/// The fixture echoes the listing it received back as the title "{id}|{name}|{kind}"
async fn echo_title(app: &TestApp, source_id: i64, listing_id: &str) -> String {
	let (status, body) = request(
		&app.app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/{listing_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["entries"][0]["title"].as_str().expect("echo title").to_string()
}

#[tokio::test]
async fn manga_list_receives_the_real_listing() {
	let app = listing_app().await;
	let (source_id, _) = install_and_ids(&app).await;

	// static entry with kind=1: AidokuRunner decodes it as UInt8 into List
	assert_eq!(echo_title(&app, source_id, "hot").await, "hot|Hot Updates|list");
	// static entry missing name: Runner semantics backfill the id
	assert_eq!(echo_title(&app, source_id, "fresh").await, "fresh|fresh|default");
	// bare string entry: id=name=the string
	assert_eq!(echo_title(&app, source_id, "bare").await, "bare|bare|default");
	// listing provided dynamically by get_listings
	assert_eq!(echo_title(&app, source_id, "dyn").await, "dyn|Dynamic Picks|list");
	// unknown id: pass-through without error (Aidoku has no lookup-by-id path; the source decides the response itself)
	assert_eq!(echo_title(&app, source_id, "ghost").await, "ghost||default");
}

#[tokio::test]
async fn listings_endpoint_normalizes_shape_and_kind() {
	let app = listing_app().await;
	let (source_id, _) = install_and_ids(&app).await;

	let (status, body) = request(
		&app.app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let listings = body.as_array().expect("listings array");
	// static kind is a number, bare string entries have no object shape, and a dynamically exported
	// kind is the enum name; the endpoint normalizes them all to {id, name, kind} with kind lowercased
	// to default/list, and the frontend checks for "list" to decide the row-style layout
	assert_eq!(
		json!(listings),
		json!([
			{"id": "hot", "name": "Hot Updates", "kind": "list"},
			{"id": "fresh", "name": "fresh", "kind": "default"},
			{"id": "bare", "name": "bare", "kind": "default"},
			{"id": "dyn", "name": "Dynamic Picks", "kind": "list"},
		])
	);
}

#[tokio::test]
async fn installed_source_dto_exposes_the_manifest_config() {
	let app = listing_app().await;
	let (source_id, plain_id) = install_and_ids(&app).await;

	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	let config = &sources
		.iter()
		.find(|s| s["id"] == source_id)
		.expect("listing source")["config"];
	assert_eq!(config["hidesFiltersWhileSearching"], true, "{config}");
	assert_eq!(config["supportsTagSearch"], true, "{config}");
	assert_eq!(config["supportsArtistSearch"], true, "{config}");
	assert_eq!(config["supportsAuthorSearch"], true, "{config}");
	assert_eq!(config["maximumParallelRequests"], 2, "{config}");

	let plain = &sources
		.iter()
		.find(|s| s["id"] == plain_id)
		.expect("plain source")["config"];
	assert!(plain.is_null(), "{plain}");

	// user-facing endpoints beyond scan have the same shape: the update DTO also carries config (source_json is the single exit point)
	let (status, body) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/sources/{source_id}"),
		Some(json!({"pinned": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["config"]["maximumParallelRequests"], 2, "{body}");
}
