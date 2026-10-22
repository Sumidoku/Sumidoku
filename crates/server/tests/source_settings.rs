//! Endpoint tests for source settings merging, notification delivery, and default-value refresh.
//! Fixture source: notify (exports get_dynamic_settings and handle_notification; each received
//! notification is woven into the echo setting's title, read back via GET settings to assert delivery).
//! Fixture rebuild: `cd tests/fixtures/notify-src && cargo build --release --target
//! wasm32-unknown-unknown`, then copy the artifact to ../notify/main.wasm.
mod common;

use axum::http::StatusCode;
use axum::Router;
use common::{TestApp, request, test_app};
use serde_json::{Value, json};

/// Registers the notify fixture source: two languages (to verify the synthesized language setting still sorts first), optionally with a static settings.json
fn install_notify_source(tmp: &tempfile::TempDir, settings: Option<&str>) {
	let dir = tmp.path().join("sources").join("en.notify");
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(common::notify_wasm_path(), dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": "en.notify", "name": "Notify", "version": 1, "languages": ["en", "ja"]}
		})
		.to_string(),
	)
	.expect("write source.json");
	if let Some(settings) = settings {
		std::fs::write(dir.join("settings.json"), settings).expect("settings.json");
	}
}

/// A static source without dynamic settings (mutation payload), version-bumped by the default-refresh test
fn write_static_source(dir: &std::path::Path, version: i64, url: &str, settings: &str) {
	std::fs::create_dir_all(dir).expect("mkdir");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {
				"id": "en.versioned", "name": "Versioned", "version": version,
				"url": url, "urls": ["https://m1.example"], "languages": ["en"]
			},
			"config": {"allowsBaseUrlSelect": true}
		})
		.to_string(),
	)
	.expect("source.json");
	std::fs::copy(common::mutation_wasm_path(), dir.join("main.wasm")).expect("main.wasm");
	std::fs::write(dir.join("settings.json"), settings).expect("settings.json");
}

async fn source_id_by_key(app: &Router, key: &str) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("source id")
}

async fn scan(app: &Router) {
	let (status, body) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

async fn settings_body(app: &Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

/// Flattens groups/pages and returns all leaf settings (merge assertions look items up by key)
fn flatten(items: &[Value]) -> Vec<&Value> {
	let mut out = Vec::new();
	for item in items {
		let container = item["value"]["Group"]["items"]
			.as_array()
			.or_else(|| item["value"]["Page"]["items"].as_array());
		if let Some(children) = container {
			out.extend(flatten(children));
		} else {
			out.push(item);
		}
	}
	out
}

#[tokio::test]
async fn static_and_dynamic_settings_merge_with_dedup() {
	let app = test_app().await;
	install_notify_source(
		&app.tmp,
		Some(r#"[
			{"type": "group", "title": "静态组", "items": [
				{"type": "switch", "key": "staticOnly", "title": "纯静态"},
				{"type": "switch", "key": "dup", "title": "静态 dup"}
			]}
		]"#),
	);
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.notify").await;

	let body = settings_body(&app.app, id).await;
	let list = body["settings"].as_array().expect("settings array");
	// the synthesized language setting sorts first (extraSettings semantics)
	assert_eq!(list[0]["key"], "__language", "{body}");

	let leaves = flatten(&list[1..]);
	let keys: Vec<&str> = leaves.iter().filter_map(|s| s["key"].as_str()).collect();
	// static items come before dynamic ones; the key-colliding dynamic dup is deduped away, keeping the static declaration
	let static_pos = keys.iter().position(|k| *k == "staticOnly").expect("staticOnly present");
	let echo_pos = keys.iter().position(|k| *k == "echo").expect("echo present");
	assert!(static_pos < echo_pos, "静态应排在动态前: {keys:?}");
	assert!(keys.contains(&"notifyOnly"), "纯动态项应保留: {keys:?}");
	let dups: Vec<&&Value> = leaves.iter().filter(|s| s["key"] == "dup").collect();
	assert_eq!(dups.len(), 1, "dup 只应出现一次: {keys:?}");
	assert_eq!(dups[0]["title"], "静态 dup", "撞键保留先出现的静态项: {body}");
}

#[tokio::test]
async fn put_settings_delivers_notifications() {
	let app = test_app().await;
	install_notify_source(&app.tmp, None);
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.notify").await;
	let path = format!("/api/v1/sources/{id}/settings");

	// triggers instance loading; echo's initial title is none
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "none", "{body}");

	// a save without notify triggers no notification
	let (status, _) = request(&app.app, "PUT", &path, Some(json!({"echo": "x"}))).await;
	assert_eq!(status, StatusCode::OK);
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "none", "无 notify 不应触发: {body}");

	// multiple notify values are delivered in order; the last becomes the instance's latest state
	let (status, body) = request(
		&app.app,
		"PUT",
		&format!("{path}?notify=first&notify=second"),
		Some(json!({"echo": "y"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "second", "两个 notification 都应送达: {body}");
}

#[tokio::test]
async fn notify_endpoint_invokes_source_action() {
	let app = test_app().await;
	install_notify_source(&app.tmp, None);
	common::install_fixture_source(&app.tmp, "en.mutation", "Mutation", &common::mutation_wasm_path());
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.notify").await;

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/notify"),
		Some(json!({"notification": "direct"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "direct", "{body}");

	// a source without the notification capability 501s per the endpoint contract
	let mutation_id = source_id_by_key(&app.app, "en.mutation").await;
	let (status, _) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{mutation_id}/notify"),
		Some(json!({"notification": "direct"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn source_update_refreshes_untouched_defaults() {
	let app: TestApp = test_app().await;
	let dir = app.tmp.path().join("sources/en.versioned");
	write_static_source(
		&dir,
		1,
		"https://v1.example",
		r#"[
			{"type": "select", "key": "quality", "title": "Q", "values": ["low", "high"], "default": "low"},
			{"type": "select", "key": "mode", "title": "M", "values": ["a", "b"], "default": "a"}
		]"#,
	);
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.versioned").await;
	let body = settings_body(&app.app, id).await;
	assert_eq!(body["values"]["quality"], "low", "{body}");
	assert_eq!(body["values"]["url"], "https://v1.example", "{body}");

	// the user changed mode; quality and url stay untouched
	let (status, _) = request(
		&app.app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"mode": "b"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// source upgrade: quality default changes to high, base url changes domain, mode default unchanged
	write_static_source(
		&dir,
		2,
		"https://v2.example",
		r#"[
			{"type": "select", "key": "quality", "title": "Q", "values": ["low", "high"], "default": "high"},
			{"type": "select", "key": "mode", "title": "M", "values": ["a", "b"], "default": "a"}
		]"#,
	);
	scan(&app.app).await;

	let body = settings_body(&app.app, id).await;
	assert_eq!(body["values"]["quality"], "high", "未动过的默认值应随源升级刷新: {body}");
	assert_eq!(body["values"]["url"], "https://v2.example", "未动过的 base url 应刷新: {body}");
	assert_eq!(body["values"]["mode"], "b", "用户改过的值必须保留: {body}");
}

/// A single-language, single-URL source gets no synthesized language/line settings (example fixture: languages ["en"], no urls)
#[tokio::test]
async fn single_language_source_has_no_synthesized_settings() {
	let app = test_app().await;
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.example-source").await;
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let keys: Vec<&str> = leaves.iter().filter_map(|s| s["key"].as_str()).collect();
	assert!(!keys.contains(&"languages") && !keys.contains(&"language"), "{keys:?}");
	assert!(!keys.contains(&"url"), "{keys:?}");
}

/// With languageSelectType == "single", a multi-language source gets a single-select language setting carrying content refresh semantics
#[tokio::test]
async fn multi_language_single_select_setting() {
	let app = test_app().await;
	let dir = app.tmp.path().join("sources/en.single-lang");
	std::fs::create_dir_all(&dir).expect("mkdir");
	std::fs::copy(common::mutation_wasm_path(), dir.join("main.wasm")).expect("main.wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": "en.single-lang", "name": "SingleLang", "version": 1,
				"languages": [{"code": "en"}, {"code": "ja", "isDefault": true}]},
			"config": {"languageSelectType": "single"}
		})
		.to_string(),
	)
	.expect("source.json");
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.single-lang").await;

	// the manifest's object form is normalized to language codes in the DTO, so the source list doesn't fail to decode
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let source = body
		.as_array()
		.unwrap()
		.iter()
		.find(|s| s["key"] == "en.single-lang")
		.expect("source listed");
	assert_eq!(source["languages"], json!(["en", "ja"]), "{source}");

	let body = settings_body(&app.app, id).await;
	let list = body["settings"].as_array().expect("settings array");
	assert_eq!(list[0]["key"], "__language", "{body}");
	let item = &list[0]["value"]["Group"]["items"][0];
	assert_eq!(item["type"], "select", "{body}");
	assert_eq!(item["key"], "language", "{body}");
	assert_eq!(item["notification"], "language", "{body}");
	assert_eq!(item["refreshes"], json!(["content"]), "{body}");
	assert_eq!(item["value"]["Select"]["values"], json!(["en", "ja"]), "{body}");
	assert_eq!(item["value"]["Select"]["default"], "ja", "{body}");
	assert_eq!(body["values"]["language"], "ja", "{body}");
}

#[tokio::test]
async fn static_settings_preserve_normalized_nulls_and_omitted_icon_inset() {
	let app = test_app().await;
	common::install_fixture_source(
		&app.tmp,
		"en.static-nulls",
		"Static Nulls",
		&common::mutation_wasm_path(),
	);
	std::fs::write(
		app.tmp.path().join("sources/en.static-nulls/settings.json"),
		r#"[
			{"type": "login", "key": "auth", "title": "Auth", "method": "oauth"},
			{"type": "editable-list", "key": "paths", "title": "Paths"},
			{"type": "page", "title": "Page", "icon": {"type": "system", "name": "gear", "color": "blue"}, "items": []}
		]"#,
	)
	.expect("settings.json");
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.static-nulls").await;
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	let login = items.iter().find(|item| item["key"] == "auth").expect("login setting");
	for key in ["pkce", "use_email", "clear_cookies_on_log_out"] {
		assert!(login["value"]["Login"][key].is_null(), "{key} should remain null: {body}");
	}
	let editable = items.iter().find(|item| item["key"] == "paths").expect("editable list");
	assert!(editable["value"]["EditableList"]["inline"].is_null(), "{body}");
	let page = items.iter().find(|item| item["type"] == "page").expect("page setting");
	let icon = &page["value"]["Page"]["icon"];
	assert_eq!(icon["type"], "system");
	assert!(icon.get("inset").is_none(), "static icon omitted inset: {icon}");
}
