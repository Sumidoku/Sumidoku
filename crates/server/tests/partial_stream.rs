mod common;

use axum::http::StatusCode;
use common::{TestApp, install_fixture_source, partial_wasm_path, request, request_ndjson, test_app};
use serde_json::Value;

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

fn event_types(events: &[Value]) -> Vec<&str> {
	events.iter().map(|e| e["type"].as_str().expect("event type")).collect()
}

#[tokio::test]
async fn home_ndjson_streams_partials_in_order() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.partial", "Partial Source", &partial_wasm_path());
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.partial")
		.map(|(_, id)| *id)
		.expect("partial source id");

	let (status, headers, events) =
		request_ndjson(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/x-ndjson"
	);

	// fixture sequence: Layout placeholder → Component A/B/C (C is new) → final
	assert_eq!(event_types(&events), ["layout", "component", "component", "component", "final"]);
	assert_eq!(events[0]["layout"]["components"].as_array().expect("layout components").len(), 2);
	for (event, index) in events[1..4].iter().zip([0, 1, 2]) {
		assert_eq!(event["index"], index);
	}
	assert_eq!(events[3]["component"]["title"], "Partial C");

	// the final payload and the non-streaming response body are the same JSON value
	let (status, buffered) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(events[4]["payload"], buffered);
}

#[tokio::test]
async fn home_without_accept_keeps_buffered_contract() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.partial", "Partial Source", &partial_wasm_path());
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.partial")
		.map(|(_, id)| *id)
		.expect("partial source id");

	let (status, headers, bytes) = common::request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/home"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/json"
	);
	// the non-streaming response has no event wrapper; it is the layout object directly
	let body: Value = serde_json::from_slice(&bytes).expect("json body");
	assert_eq!(body["components"].as_array().expect("components").len(), 3);
	assert!(body.get("type").is_none());
}

#[tokio::test]
async fn search_ndjson_streams_manga_deltas() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.partial", "Partial Source", &partial_wasm_path());
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.partial")
		.map(|(_, id)| *id)
		.expect("partial source id");

	let (status, _, events) = request_ndjson(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(serde_json::json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	assert_eq!(event_types(&events), ["manga", "manga", "manga", "final"]);
	let keys: Vec<&str> = events[..3]
		.iter()
		.map(|e| e["manga"]["key"].as_str().expect("manga key"))
		.collect();
	assert_eq!(keys, ["partial-1", "partial-2", "partial-3"]);
	// partial entries and final entries share the same id-attach semantics
	assert_eq!(events[0]["manga"]["sourceId"], source_id);

	let payload = &events[3]["payload"];
	let entries = payload["entries"].as_array().expect("final entries");
	assert_eq!(entries.len(), 2);
	assert_eq!(entries[0]["key"], "final-1");
	assert_eq!(payload["hasNextPage"], true);
}

#[tokio::test]
async fn stream_without_partials_has_only_final() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.example-source")
		.map(|(_, id)| *id)
		.expect("example source id");

	// the example fixture emits no partials: the stream has only the final event
	let (status, _, events) =
		request_ndjson(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(event_types(&events), ["final"]);
	assert_eq!(
		events[0]["payload"]["components"].as_array().expect("components").len(),
		7
	);

	let (status, _, events) = request_ndjson(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(serde_json::json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(event_types(&events), ["final"]);
}

#[tokio::test]
async fn home_stream_unsupported_source_501() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &common::mutation_wasm_path());
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.mutation")
		.map(|(_, id)| *id)
		.expect("mutation source id");

	// a missing capability returns 501 unsupported before the stream starts, same contract as non-streaming
	let (status, headers, bytes) = common::request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/home"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/json"
	);
	let body: Value = serde_json::from_slice(&bytes).expect("json body");
	assert_eq!(body["error"]["code"], "unsupported");

	let (status, _, _) =
		request_ndjson(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

async fn partial_source_id(app: &axum::Router, tmp: &tempfile::TempDir) -> i64 {
	install_fixture_source(tmp, "en.partial", "Partial Source", &partial_wasm_path());
	let ids = scan_sources(app).await;
	ids.iter()
		.find(|(key, _)| key == "en.partial")
		.map(|(_, id)| *id)
		.expect("partial source id")
}

async fn register_buffered(app: &axum::Router, source_id: i64, manga_key: &str) -> i64 {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(serde_json::json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	body["id"].as_i64().expect("manga id")
}

#[tokio::test]
async fn register_ndjson_streams_manga_partials() {
	let TestApp { app, tmp, .. } = test_app().await;
	let source_id = partial_source_id(&app, &tmp).await;

	let (status, headers, events) = request_ndjson(
		&app,
		"POST",
		"/api/v1/manga",
		Some(serde_json::json!({"sourceId": source_id, "mangaKey": "streamed-manga"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/x-ndjson"
	);

	// fixture sequence: details snapshot (no chapters) → details+two-chapter snapshot → final
	assert_eq!(event_types(&events), ["manga", "manga", "final"]);
	assert_eq!(events[0]["manga"]["key"], "streamed-manga");
	assert_eq!(events[0]["manga"]["title"], "Partial Title");
	assert!(events[0]["manga"]["chapters"].is_null());
	assert_eq!(
		events[1]["manga"]["chapters"].as_array().expect("partial chapters").len(),
		2
	);

	// the final state is authoritative: three chapters are persisted
	let manga_id = events[2]["payload"]["id"].as_i64().expect("manga id");
	let (status, chapters) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = chapters.as_array().expect("chapters array");
	assert_eq!(chapters.len(), 3);
	assert_eq!(chapters[2]["key"], "c3");
}

#[tokio::test]
async fn register_without_accept_keeps_buffered_contract() {
	let TestApp { app, tmp, .. } = test_app().await;
	let source_id = partial_source_id(&app, &tmp).await;

	let (status, headers, bytes) = common::request_full(
		&app,
		"POST",
		"/api/v1/manga",
		Some(serde_json::json!({"sourceId": source_id, "mangaKey": "buffered-manga"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/json"
	);
	// the non-streaming response has no event wrapper; it is {"id": …} directly
	let body: Value = serde_json::from_slice(&bytes).expect("json body");
	assert!(body["id"].is_i64());
	assert!(body.get("type").is_none());
}

#[tokio::test]
async fn register_existing_with_ndjson_falls_back_to_buffered() {
	let TestApp { app, tmp, .. } = test_app().await;
	let source_id = partial_source_id(&app, &tmp).await;
	let manga_id = register_buffered(&app, source_id, "already-there").await;

	// an already-registered hit has no source call to stream: plain JSON is returned, the client treats it as the non-streaming final state
	let (status, headers, bytes) = common::request_full(
		&app,
		"POST",
		"/api/v1/manga",
		Some(serde_json::json!({"sourceId": source_id, "mangaKey": "already-there"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let body: Value = serde_json::from_slice(&bytes).expect("json body");
	assert_eq!(body["id"], manga_id);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/json"
	);
}

#[tokio::test]
async fn manga_refresh_ndjson_streams_partials() {
	let TestApp { app, tmp, .. } = test_app().await;
	let source_id = partial_source_id(&app, &tmp).await;
	let manga_id = register_buffered(&app, source_id, "refresh-me").await;

	let (status, _, events) =
		request_ndjson(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh"), None).await;
	assert_eq!(status, StatusCode::OK);

	// details refresh only wants details: one details-snapshot partial, no chapters
	assert_eq!(event_types(&events), ["manga", "final"]);
	assert_eq!(events[0]["manga"]["title"], "Partial Title");
	assert!(events[0]["manga"]["chapters"].is_null());

	// the final payload and the non-streaming response body are the same JSON value
	let (status, buffered) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(events[1]["payload"], buffered);
}

#[tokio::test]
async fn chapters_refresh_ndjson_streams_partials() {
	let TestApp { app, tmp, .. } = test_app().await;
	let source_id = partial_source_id(&app, &tmp).await;
	let manga_id = register_buffered(&app, source_id, "refresh-chapters").await;

	let (status, _, events) = request_ndjson(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/chapters?refresh"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// chapters refresh only wants chapters: one two-chapter-snapshot partial
	assert_eq!(event_types(&events), ["manga", "final"]);
	assert_eq!(
		events[0]["manga"]["chapters"].as_array().expect("partial chapters").len(),
		2
	);
	let payload = events[1]["payload"].as_array().expect("chapters payload");
	assert_eq!(payload.len(), 3);

	// the final payload and the non-streaming response body are the same JSON value
	let (status, buffered) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(events[1]["payload"], buffered);
}
