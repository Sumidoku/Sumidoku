mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, fixture_dir, install_fixture_source, request, test_app};
use serde_json::{Value, json};
use sumidoku_server::build_app;

/// Installs three fixture sources (example + two copies of the same wasm) and returns the list sorted by id
async fn install_three(app: &Router, tmp: &tempfile::TempDir) -> Vec<Value> {
	install_fixture_source(tmp, "en.second", "Second Source", &fixture_dir().join("main.wasm"));
	install_fixture_source(tmp, "en.third", "Third Source", &fixture_dir().join("main.wasm"));
	let (status, body) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["installed"].as_array().expect("installed").len(), 3);
	list_sources(app).await
}

async fn list_sources(app: &Router) -> Vec<Value> {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body.as_array().expect("sources array").clone()
}

fn by_key(sources: &[Value], key: &str) -> Value {
	sources
		.iter()
		.find(|s| s["key"] == key)
		.cloned()
		.unwrap_or_else(|| panic!("source {key} installed"))
}

fn id_of(source: &Value) -> i64 {
	source["id"].as_i64().expect("source id")
}

async fn patch_pin(app: &Router, id: i64, pinned: bool) -> (StatusCode, Value) {
	request(app, "PATCH", &format!("/api/v1/sources/{id}"), Some(json!({"pinned": pinned}))).await
}

async fn reorder(app: &Router, ids: &[&str]) -> (StatusCode, Value) {
	request(app, "POST", "/api/v1/sources/reorder-pinned", Some(json!({"ids": ids}))).await
}

#[tokio::test]
async fn pin_unpin_cycle() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	for s in &sources {
		assert_eq!(s["pinned"], false, "{s}");
		assert!(s["pinOrder"].is_null(), "{s}");
	}
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let third = by_key(&sources, "en.third");

	// pin appends to the end of the list: second → 0, first → 1, third → 2
	let (status, body) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");
	let (status, body) = patch_pin(&app, id_of(&first), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 1, "{body}");
	let (status, body) = patch_pin(&app, id_of(&third), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 2, "{body}");

	// repeated pin is idempotent: no reordering, no duplicate entry
	let (status, body) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(sources.iter().filter(|s| s["pinned"] == true).count(), 3);

	// unpinning second in the middle: first moves up, third stays last
	let (status, body) = patch_pin(&app, id_of(&second), false).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], false, "{body}");
	assert!(body["pinOrder"].is_null(), "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinned"], false);
	// repeated unpin is idempotent
	let (status, body) = patch_pin(&app, id_of(&second), false).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], false, "{body}");

	// empty body / missing pinned field is a no-op
	let (status, body) = request(&app, "PATCH", &format!("/api/v1/sources/{}", id_of(&first)), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/sources/{}", id_of(&first)),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");

	// invalid value 400s and is not persisted
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/sources/{}", id_of(&first)),
		Some(json!({"pinned": "yes"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert_eq!(body["error"]["code"], "bad_request");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.example-source")["pinned"], true);

	// nonexistent source 404s
	let (status, _) = patch_pin(&app, 999, true).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disable_enable_cycle_enforces_source_state() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let source = by_key(&sources, "en.example-source");
	let id = id_of(&source);
	let path = format!("/api/v1/sources/{id}");

	let (status, body) = patch_pin(&app, id, true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");

	let filters_path = format!("{path}/filters");
	let (status, body) = request(&app, "GET", &filters_path, None).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let (status, body) = request(&app, "PATCH", &path, Some(json!({"enabled": false}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["enabled"], false, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");

	let (status, body) = request(&app, "GET", &filters_path, None).await;
	assert_eq!(status, StatusCode::CONFLICT, "disabled source returned filters: {body}");
	assert_eq!(body["error"]["code"], "source_disabled", "{body}");

	let (status, body) = request(&app, "PATCH", &path, Some(json!({"enabled": "yes"}))).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert_eq!(body["error"]["code"], "bad_request", "{body}");

	let (status, body) = request(&app, "PATCH", &path, Some(json!({"enabled": true}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["enabled"], true, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");

	let (status, body) = request(&app, "GET", &filters_path, None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn reorder_pinned_validates_set() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let third = by_key(&sources, "en.third");
	for s in [&first, &second, &third] {
		let (status, _) = patch_pin(&app, id_of(s), true).await;
		assert_eq!(status, StatusCode::OK);
	}

	// valid reorder: [first, second, third] → [third, first, second]
	let (status, body) = reorder(&app, &["en.third", "en.example-source", "en.second"]).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 2);

	// the set must match exactly: missing id / extra id / duplicate id / unpinned key / nonexistent key / malformed shape all 400, and the order is unchanged
	for bad in [
		json!({"ids": ["en.third", "en.example-source"]}),
		json!({"ids": ["en.third", "en.example-source", "en.second", "en.extra"]}),
		json!({"ids": ["en.third", "en.third", "en.example-source"]}),
		json!({"ids": ["en.third", "en.example-source", "en.ghost"]}),
		json!({"ids": "en.third"}),
		json!({}),
	] {
		let (status, body) = request(&app, "POST", "/api/v1/sources/reorder-pinned", Some(bad.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
	}
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 2);
}

#[tokio::test]
async fn delete_source_prunes_pinned() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	for s in [&first, &second] {
		let (status, _) = patch_pin(&app, id_of(s), true).await;
		assert_eq!(status, StatusCode::OK);
	}

	// uninstalling first at the head of the list: its key is removed from the list, second moves up to 0
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/sources/{}", id_of(&first)), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 0);

	// after uninstall, reorder only accepts the remaining set: a stale key or an "installed but not pinned" key both 400
	let (status, body) = reorder(&app, &["en.example-source", "en.second"]).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let (status, body) = reorder(&app, &["en.third"]).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn pins_survive_restart() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let (status, _) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = patch_pin(&app, id_of(&first), true).await;
	assert_eq!(status, StatusCode::OK);
	drop(app);

	// restart: new AppState, new connection pool, same data files
	let app = build_app(base_config(&tmp)).await.expect("rebuild app");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.third")["pinned"], false);
}
