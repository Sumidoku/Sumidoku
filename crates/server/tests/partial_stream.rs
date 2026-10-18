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

	// fixture 序列：Layout 占位 → Component A/B/C（C 为新增）→ 终态
	assert_eq!(event_types(&events), ["layout", "component", "component", "component", "final"]);
	assert_eq!(events[0]["layout"]["components"].as_array().expect("layout components").len(), 2);
	for (event, index) in events[1..4].iter().zip([0, 1, 2]) {
		assert_eq!(event["index"], index);
	}
	assert_eq!(events[3]["component"]["title"], "Partial C");

	// 终态 payload 与非流式响应体是同一个 JSON 值
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
	// 非流式响应不含事件包装，直接是布局对象
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
	// partial 条目与终态条目走同一套 id attach 语义
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
	let source_id = ids[0].1;

	// example fixture 不发 partial：流里只有终态事件
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
async fn home_stream_unsupported_source_404() {
	let TestApp { app, tmp, .. } = test_app().await;
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &common::mutation_wasm_path());
	let ids = scan_sources(&app).await;
	let source_id = ids
		.iter()
		.find(|(key, _)| key == "en.mutation")
		.map(|(_, id)| *id)
		.expect("mutation source id");

	// 能力缺失在流开始前返回 404，与非流式同一契约
	let (status, headers, bytes) = common::request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/home"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(
		headers.get("content-type").expect("content-type"),
		"application/json"
	);
	let body: Value = serde_json::from_slice(&bytes).expect("json body");
	assert_eq!(body["error"]["code"], "unsupported");

	let (status, _, _) =
		request_ndjson(&app, "GET", &format!("/api/v1/sources/{source_id}/home"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}
