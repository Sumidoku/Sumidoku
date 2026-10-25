//! Multi-instance local sources: registration through the internal-source endpoint (validation, slug
//! collision counter, no pin at registration), per-instance roots/oneshots in the source settings, delete
//! semantics (a config-defined default instance refuses; others cascade), boot recreation
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, request, source_by_key};
use serde_json::{Value, json};
use sumidoku_server::build_app;

fn write_page(dir: &std::path::Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

/// Two roots outside the config dir; the app boots without an explicit config root list, so the default
/// instance points at the (empty) <config_dir>/local derivation and instances carry their own paths
struct InstanceApp {
	app: TestApp,
	root_a: tempfile::TempDir,
	root_b: tempfile::TempDir,
}

async fn instance_app() -> InstanceApp {
	let root_a = tempfile::tempdir().expect("root a");
	let root_b = tempfile::tempdir().expect("root b");
	write_page(&root_a.path().join("Alpha/ch 1"), "1.png", b"a-alpha-1-1");
	write_page(&root_b.path().join("Beta/ch 1"), "1.png", b"b-beta-1-1");
	// a same-named directory in both roots: two instances over them must stay independent
	write_page(&root_b.path().join("Alpha/ch 1"), "1.png", b"b-alpha-1-1");
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	InstanceApp {
		app: TestApp { app, tmp, db_path },
		root_a,
		root_b,
	}
}

fn root_paths(dirs: &[&tempfile::TempDir]) -> Vec<String> {
	dirs.iter()
		.map(|dir| dir.path().to_string_lossy().into_owned())
		.collect()
}

async fn register_local(app: &Router, body: Value) -> (StatusCode, Value) {
	request(app, "POST", "/api/v1/sources/internal", Some(body)).await
}

async fn instance_settings(app: &Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

async fn browse_keys(app: &Router, source_id: i64) -> Vec<String> {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let mut keys: Vec<String> = body["entries"]
		.as_array()
		.expect("entries array")
		.iter()
		.map(|e| e["key"].as_str().expect("key").to_string())
		.collect();
	keys.sort();
	keys
}

async fn register_manga(app: &Router, source_id: i64, manga_key: &str) -> i64 {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["id"].as_i64().expect("manga id")
}

#[tokio::test]
async fn register_local_instances_validate() {
	let t = instance_app().await;

	// validation: empty name / no paths / empty entry / missing directory / a file all 400
	for body in [
		json!({"type": "local", "name": "  ", "paths": root_paths(&[&t.root_a])}),
		json!({"type": "local", "name": "x", "paths": []}),
		json!({"type": "local", "name": "x", "paths": ["  "]}),
		json!({"type": "local", "name": "x", "paths": [t.root_a.path().join("missing").to_string_lossy()]}),
		json!({"type": "local", "name": "x", "paths": [t.root_a.path().join("Alpha/ch 1/1.png").to_string_lossy()]}),
	] {
		let (status, body) = register_local(&t.app.app, body).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	}

	// registration never pins: internal sources show in their own browse section instead
	let (_, settings) = request(&t.app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["browse"]["pinnedSources"], json!([]));

	let (status, first) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "Manga A", "paths": root_paths(&[&t.root_a])}),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{first}");
	assert_eq!(first["key"], "local.manga-a");
	assert_eq!(first["kind"], "local");
	assert_eq!(first["pinned"], false);

	// collision counting, same rule as opds instances
	let (status, second) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "Manga A", "paths": root_paths(&[&t.root_b])}),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{second}");
	assert_eq!(second["key"], "local.manga-a-2");

	// pinning stays available through the ordinary PATCH toggle
	let first_id = first["id"].as_i64().expect("first id");
	let (status, body) = request(
		&t.app.app,
		"PATCH",
		&format!("/api/v1/sources/{first_id}"),
		Some(json!({"pinned": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true);
}

/// A CJK instance name slugs to itself (Unicode-aware), not to a "no usable characters" 400
#[tokio::test]
async fn register_local_instance_with_cjk_name() {
	let t = instance_app().await;
	let (status, body) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "漫画", "paths": root_paths(&[&t.root_a])}),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "local.漫画");
	assert_eq!(body["name"], "漫画");

	// the same CJK name collides and takes the counter form
	let (status, second) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "漫画", "paths": root_paths(&[&t.root_b])}),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{second}");
	assert_eq!(second["key"], "local.漫画-2");
}

#[tokio::test]
async fn instances_scan_their_own_roots() {
	let t = instance_app().await;
	let (_, first) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "A", "paths": root_paths(&[&t.root_a])}),
	)
	.await;
	let (_, second) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "B", "paths": root_paths(&[&t.root_b])}),
	)
	.await;
	let id_a = first["id"].as_i64().expect("first id");
	let id_b = second["id"].as_i64().expect("second id");

	assert_eq!(browse_keys(&t.app.app, id_a).await, ["Alpha"]);
	assert_eq!(browse_keys(&t.app.app, id_b).await, ["Alpha", "Beta"]);

	// the same relative path in two instances is two independent manga with independent progress
	let manga_a = register_manga(&t.app.app, id_a, "Alpha").await;
	let manga_b = register_manga(&t.app.app, id_b, "Alpha").await;
	assert_ne!(manga_a, manga_b);

	// reading one does not mark the other: distinct rows, distinct chapter state
	let (status, body) = request(&t.app.app, "GET", &format!("/api/v1/manga/{manga_a}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_a = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(
		&t.app.app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_a}"),
		Some(json!({"isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&t.app.app, "GET", &format!("/api/v1/manga/{manga_b}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["isRead"], false, "the other instance's row is untouched");

	// page bytes come from the owning instance's root
	let (status, _, bytes) = common::request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_a}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"a-alpha-1-1");
	let chapter_b = body[0]["id"].as_i64().expect("chapter id b");
	let (status, _, bytes) = common::request_full(
		&t.app.app,
		"GET",
		&format!("/api/v1/chapter/{chapter_b}/page/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"b-alpha-1-1");
}

#[tokio::test]
async fn instance_settings_roundtrip_and_rescan() {
	let t = instance_app().await;
	let (_, first) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "B", "paths": root_paths(&[&t.root_b]), "oneshotsDir": "singles"}),
	)
	.await;
	let id = first["id"].as_i64().expect("id");

	// the synthesized definition and the effective values
	let doc = instance_settings(&t.app.app, id).await;
	assert_eq!(doc["settings"][0]["type"], "list");
	assert_eq!(doc["settings"][0]["key"], "paths");
	assert_eq!(doc["settings"][1]["key"], "oneshotsDir");
	assert_eq!(doc["values"]["paths"], json!(root_paths(&[&t.root_b])));
	assert_eq!(doc["values"]["oneshotsDir"], "singles");

	// write validation: wrong shapes and bad paths 400; unknown keys 400
	for body in [
		json!({"paths": []}),
		json!({"paths": "not-an-array"}),
		json!({"paths": [t.root_a.path().join("missing").to_string_lossy()]}),
		json!({"oneshotsDir": 1}),
		json!({"unknown": "x"}),
	] {
		let (status, body) = request(&t.app.app, "PUT", &format!("/api/v1/sources/{id}/settings"), Some(body)).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	}

	// repointing the roots takes effect on the next scan, no restart
	let (status, body) = request(
		&t.app.app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"paths": root_paths(&[&t.root_a])})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["paths"], json!(root_paths(&[&t.root_a])));
	assert_eq!(browse_keys(&t.app.app, id).await, ["Alpha"]);

	// an explicit blank oneshots marker turns the rule off for the instance; null clears the override
	let (status, body) = request(
		&t.app.app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"oneshotsDir": "  "})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["oneshotsDir"], Value::Null);
	let (status, body) = request(
		&t.app.app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"oneshotsDir": null})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["oneshotsDir"], Value::Null);

	// reset clears the oneshots override but keeps the roots (they have no default to reset to)
	let (status, body) = request(
		&t.app.app,
		"POST",
		&format!("/api/v1/sources/{id}/settings/reset"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["oneshotsDir"], Value::Null);
	assert_eq!(body["values"]["paths"], json!(root_paths(&[&t.root_a])));
}

#[tokio::test]
async fn oneshots_blank_disables_rule_while_null_restores_config_fallback() {
	let root = tempfile::tempdir().expect("root");
	write_page(&root.path().join("Alpha/ch 1"), "1.png", b"alpha-1-1");
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_oneshots_dir = Some("oneshots".into());
	let app = build_app(config).await.expect("build app");

	let (status, first) = register_local(
		&app,
		json!({"type": "local", "name": "B", "paths": [root.path().to_string_lossy()], "oneshotsDir": " "}),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{first}");
	let id = first["id"].as_i64().expect("id");
	let doc = instance_settings(&app, id).await;
	assert_eq!(doc["values"]["oneshotsDir"], Value::Null, "an explicit blank disables the configured fallback");

	let (status, body) = request(&app, "PUT", &format!("/api/v1/sources/{id}/settings"), Some(json!({"oneshotsDir": null}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["oneshotsDir"], "oneshots");
	let (status, body) = request(&app, "POST", &format!("/api/v1/sources/{id}/settings/reset"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["oneshotsDir"], "oneshots");
}

#[tokio::test]
async fn delete_instance_cascades() {
	let t = instance_app().await;
	let (_, first) = register_local(
		&t.app.app,
		json!({"type": "local", "name": "B", "paths": root_paths(&[&t.root_b])}),
	)
	.await;
	let id = first["id"].as_i64().expect("id");
	let manga_id = register_manga(&t.app.app, id, "Beta").await;

	// pin it first: deletion prunes the key from the pinned list
	let (status, _) = request(&t.app.app, "PATCH", &format!("/api/v1/sources/{id}"), Some(json!({"pinned": true}))).await;
	assert_eq!(status, StatusCode::OK);

	let (status, _) = request(&t.app.app, "DELETE", &format!("/api/v1/sources/{id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);

	let (status, _) = request(&t.app.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND, "the instance's manga cascade away");
	let (status, body) = request(&t.app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		!body.as_array().expect("sources").iter().any(|s| s["key"] == "local.b"),
		"the instance is gone: {body}"
	);
	let (_, settings) = request(&t.app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["browse"]["pinnedSources"], json!([]), "the pin is pruned");
}

#[tokio::test]
async fn default_instance_delete_rules_follow_the_config() {
	// with an explicit config root list the default instance is config-defined: deleting it 400s with a hint
	let root = tempfile::tempdir().expect("root");
	write_page(&root.path().join("Alpha/ch 1"), "1.png", b"alpha-1-1");
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dirs = Some(vec![root.path().to_path_buf()]);
	let db_url = config.database_url.clone();
	let app = build_app(config.clone()).await.expect("build app");
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local_id = source_by_key(&body, "local.local")["id"].as_i64().expect("local id");
	let (status, body) = request(&app, "DELETE", &format!("/api/v1/sources/{local_id}"), None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert!(body["error"]["message"].as_str().expect("message").contains("SUMIDOKU_LOCAL_DIRS"));
	drop(app);

	// without an explicit config the default instance deletes like any other instance…
	let mut config = base_config(&tmp);
	config.database_url = db_url.clone();
	config.local_dirs = None;
	let app = build_app(config).await.expect("rebuild app");
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local_id = source_by_key(&body, "local.local")["id"].as_i64().expect("local id");
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/sources/{local_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	drop(app);

	// …and stays gone while the config names nothing…
	let mut config = base_config(&tmp);
	config.database_url = db_url.clone();
	config.local_dirs = None;
	let app = build_app(config).await.expect("rebuild app");
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		!body.as_array().expect("sources").iter().any(|s| s["key"] == "local.local"),
		"the deletion sticks: {body}"
	);
	drop(app);

	// …but a config that explicitly names roots again recreates a fresh empty default instance
	let mut config = base_config(&tmp);
	config.database_url = db_url;
	config.local_dirs = Some(vec![root.path().to_path_buf()]);
	let app = build_app(config).await.expect("rebuild app");
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(source_by_key(&body, "local.local")["kind"], "local");
	// the recreated instance is empty (no manga rows), though its config root scans fine
	let recreated_id = source_by_key(&body, "local.local")["id"].as_i64().expect("recreated id");
	assert_eq!(browse_keys(&app, recreated_id).await, ["Alpha"]);
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("library entries").len(), 0);
}

#[tokio::test]
async fn default_instance_keeps_config_fallback_and_keys() {
	// upgrading shape: no per-instance paths stored, the default instance scans the config root, and a
	// registered manga keeps resolving after the same root is later written as the instance setting
	let root = tempfile::tempdir().expect("root");
	write_page(&root.path().join("Alpha/ch 1"), "1.png", b"alpha-1-1");
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dirs = Some(vec![root.path().to_path_buf()]);
	let app = build_app(config).await.expect("build app");
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local_id = source_by_key(&body, "local.local")["id"].as_i64().expect("local id");

	assert_eq!(browse_keys(&app, local_id).await, ["Alpha"]);
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": local_id, "mangaKey": "Alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let manga_id = body["id"].as_i64().expect("manga id");

	// materializing the same root as the instance's own setting changes nothing for the row
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/sources/{local_id}/settings"),
		Some(json!({"paths": [root.path().to_string_lossy()]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["title"], "Alpha");
}
