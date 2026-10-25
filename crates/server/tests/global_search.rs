mod common;

use axum::http::StatusCode;
use common::{
	TestApp, fixture_dir, install_fixture_source, mutation_wasm_path, request, request_ndjson,
	test_app,
};
use serde_json::json;

async fn scan_sources(app: &axum::Router) -> Vec<(String, i64)> {
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

fn source_id_of(ids: &[(String, i64)], key: &str) -> i64 {
	ids.iter()
		.find(|(k, _)| k == key)
		.map(|(_, id)| *id)
		.unwrap_or_else(|| panic!("source id for {key}"))
}

#[tokio::test]
async fn aggregate_returns_each_sources_entries_tagged_with_its_id() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.example-two", "Example Two", &fixture_dir().join("main.wasm"));
	let ids = scan_sources(&app).await;
	let one = source_id_of(&ids, "en.example-source");
	let two = source_id_of(&ids, "en.example-two");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"page": 1, "sourceIds": [one, two]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let sources = body["sources"].as_array().expect("sources array");
	assert_eq!(sources.len(), 2);
	// Results are slotted back in the order of the sourceIds in the request
	assert_eq!(sources[0]["sourceId"], one);
	assert_eq!(sources[1]["sourceId"], two);
	for (entry, id) in sources.iter().zip([one, two]) {
		assert!(entry.get("error").is_none());
		let entries = entry["entries"].as_array().expect("entries");
		assert!(!entries.is_empty());
		for manga in entries {
			assert_eq!(manga["sourceId"], id);
		}
	}
}

#[tokio::test]
async fn failing_source_only_marks_itself() {
	let TestApp { app, tmp, .. } = test_app().await;
	// the mutation fixture's get_search_manga_list always bails, serving as the failing source
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &mutation_wasm_path());
	let ids = scan_sources(&app).await;
	let good = source_id_of(&ids, "en.example-source");
	let bad = source_id_of(&ids, "en.mutation");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"sourceIds": [bad, good]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let sources = body["sources"].as_array().expect("sources array");
	assert_eq!(sources.len(), 2);

	assert_eq!(sources[0]["sourceId"], bad);
	assert_eq!(sources[0]["entries"].as_array().expect("entries").len(), 0);
	assert_eq!(sources[0]["hasNextPage"], false);
	assert_eq!(sources[0]["error"]["code"], "source_error");

	assert_eq!(sources[1]["sourceId"], good);
	assert!(sources[1].get("error").is_none());
	assert!(!sources[1]["entries"].as_array().expect("entries").is_empty());
}

#[tokio::test]
async fn unknown_source_only_marks_itself() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let ids = scan_sources(&app).await;
	let good = source_id_of(&ids, "en.example-source");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"sourceIds": [9999, good]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let sources = body["sources"].as_array().expect("sources array");
	assert_eq!(sources.len(), 2);
	// handle resolution failure and source call failure share the same isolation path
	assert_eq!(sources[0]["sourceId"], 9999);
	assert_eq!(sources[0]["error"]["code"], "not_found");
	assert!(sources[1].get("error").is_none());
	assert!(!sources[1]["entries"].as_array().expect("entries").is_empty());
}

#[tokio::test]
async fn ndjson_emits_per_source_events_then_final() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &mutation_wasm_path());
	let ids = scan_sources(&app).await;
	let good = source_id_of(&ids, "en.example-source");
	let bad = source_id_of(&ids, "en.mutation");

	let (status, headers, events) = request_ndjson(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"sourceIds": [good, bad]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/x-ndjson"
	);

	// one event per source completion/failure (completion order is not fixed), plus one final event
	assert_eq!(events.len(), 3);
	assert_eq!(events[2]["type"], "final");
	let source_event = events[..2]
		.iter()
		.find(|e| e["type"] == "source")
		.expect("source event");
	let error_event = events[..2]
		.iter()
		.find(|e| e["type"] == "sourceError")
		.expect("sourceError event");
	assert_eq!(source_event["source"]["sourceId"], good);
	assert!(!source_event["source"]["entries"].as_array().expect("entries").is_empty());
	assert_eq!(error_event["sourceId"], bad);
	assert_eq!(error_event["error"]["code"], "source_error");

	// the final payload and the buffered response are the same JSON value
	let (status, buffered) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"sourceIds": [good, bad]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(events[2]["payload"], buffered);
}

#[tokio::test]
async fn page_must_be_positive() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let ids = scan_sources(&app).await;
	let good = source_id_of(&ids, "en.example-source");

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"page": 0, "sourceIds": [good]})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert_eq!(body["error"]["code"], "bad_request");
}
