//! Endpoint tests for web login (method: "web") and source-level logout.
//! Fixture sources: weblogin (exports handle_web_login, vetoes when the cookies or preset defaults
//! lack session, treats empty cookies as confirmed logout) and weblogin-plain (same settings, no export).
//! The three login settings cover the url / url_key / base_url fallback paths of host resolution.
mod common;

use axum::http::StatusCode;
use axum::Router;
use common::{TestApp, request, test_app};
use serde_json::{Value, json};

/// Registers a source with a base url: the login_fallback setting's host fallback relies on it
fn install_login_source(tmp: &tempfile::TempDir, key: &str, wasm: &std::path::Path) {
	let dir = tmp.path().join("sources").join(key);
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(wasm, dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": key, "name": key, "version": 1, "languages": ["en"], "url": "https://base.example.net"}
		})
		.to_string(),
	)
	.expect("write source.json");
}

async fn source_id_by_key(app: &Router, key: &str) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body
		.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("source id")
}

async fn app_with_login_source(wasm: &std::path::Path) -> (TestApp, i64) {
	let app = test_app().await;
	install_login_source(&app.tmp, "en.weblogin", wasm);
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let id = source_id_by_key(&app.app, "en.weblogin").await;
	(app, id)
}

async fn cookie_rows(db_path: &std::path::Path, source_id: i64) -> Vec<(String, String, String)> {
	let pool = common::connect_db(db_path).await;
	let rows = sqlx::query_as("SELECT domain, name, value FROM source_cookie WHERE source_id = ? ORDER BY domain, name")
		.bind(source_id)
		.fetch_all(&pool)
		.await
		.expect("cookie rows");
	pool.close().await;
	rows
}

async fn settings_values(app: &Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK);
	body["values"].clone()
}

fn web_login_body(key: &str, cookies: &[(&str, &str)]) -> Value {
	let map: serde_json::Map<String, Value> = cookies
		.iter()
		.map(|(k, v)| (k.to_string(), json!(v)))
		.collect();
	json!({"key": key, "cookies": Value::Object(map)})
}

#[tokio::test]
async fn web_login_stores_cookies_and_marks_logged_in() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	let (status, body) = request(
		&app.app,
		"POST",
		&login_path,
		Some(json!({
			"key": "login",
			"cookies": {"session": "s1", "other": "o1"},
			"localStorage": {"token": "t1"},
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["ok"], true);

	let values = settings_values(&app.app, id).await;
	assert_eq!(values["login"], "logged_in");
	// keys/values are persisted sorted by cookie name, mirroring Aidoku's <key>.keys/<key>.values
	assert_eq!(values["login.keys"], json!(["other", "session"]));
	assert_eq!(values["login.values"], json!(["o1", "s1"]));
	assert_eq!(values["login.ls.token"], "t1");

	assert_eq!(
		cookie_rows(&app.db_path, id).await,
		vec![
			("login.example.com".into(), "other".into(), "o1".into()),
			("login.example.com".into(), "session".into(), "s1".into()),
		]
	);

	// login triggers an instance restart: the new instance backfills defaults from the DB, so what the source runtime reads must match the DB
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(
		body["entries"][0]["title"],
		"marker=logged_in;keys=other,session"
	);
}

#[tokio::test]
async fn web_login_handler_veto_blocks_marker_and_cookies() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	// the fixture only accepts the session cookie: without it handle_web_login returns false
	let (status, body) = request(&app.app, "POST", &login_path, Some(web_login_body("login", &[("other", "o1")]))).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
	assert_eq!(body["error"]["code"], "bad_gateway");

	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "否决后不得有 marker: {values}");
	// Aidoku's order is storing keys/values before calling the export; a veto doesn't roll back the stored part
	assert_eq!(values["login.keys"], json!(["other"]));
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}

#[tokio::test]
async fn web_login_without_handler_still_stores_cookies() {
	let (app, id) = app_with_login_source(&common::weblogin_plain_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	let (status, body) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login", &[("session", "s1")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let values = settings_values(&app.app, id).await;
	assert_eq!(values["login"], "logged_in");
	assert_eq!(
		cookie_rows(&app.db_path, id).await,
		vec![("login.example.com".into(), "session".into(), "s1".into())]
	);
}

#[tokio::test]
async fn web_login_resolves_host_from_url_key_and_base_url() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	// login_keep has no explicit url: url_key points at the auth_url setting's default value
	let (status, body) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login_keep", &[("session", "s2")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	// login_fallback has neither url nor url_key: falls back to source.json's info.url
	let (status, body) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login_fallback", &[("session", "s3")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	assert_eq!(
		cookie_rows(&app.db_path, id).await,
		vec![
			("auth.example.org".into(), "session".into(), "s2".into()),
			("base.example.net".into(), "session".into(), "s3".into()),
		]
	);
}

#[tokio::test]
async fn web_login_empty_cookies_clears_login_state() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login", &[("session", "s1")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(settings_values(&app.app, id).await["login"], "logged_in");

	let (status, body) = request(&app.app, "POST", &login_path, Some(web_login_body("login", &[]))).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "空 cookies 应清 marker: {values}");
	assert_eq!(values["login.keys"], json!([]));
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}

#[tokio::test]
async fn logout_clears_state_and_cookies_when_flagged() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");
	let logout_path = format!("/api/v1/sources/{id}/login/logout");

	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(json!({
			"key": "login",
			"cookies": {"session": "s1"},
			"localStorage": {"token": "t1"},
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// the login setting declares clear_cookies_on_log_out
	let (status, body) = request(&app.app, "POST", &logout_path, Some(json!({"key": "login"}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let values = settings_values(&app.app, id).await;
	for key in ["login", "login.keys", "login.values", "login.ls.token"] {
		assert!(values.get(key).is_none(), "登出应清掉 {key}: {values}");
	}
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}

#[tokio::test]
async fn logout_keeps_cookies_without_flag_clears_on_confirm() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");
	let logout_path = format!("/api/v1/sources/{id}/login/logout");

	// the login_keep setting doesn't declare clear_cookies_on_log_out: the setting state is cleared, the cookie kept
	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login_keep", &[("session", "s2")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&app.app, "POST", &logout_path, Some(json!({"key": "login_keep"}))).await;
	assert_eq!(status, StatusCode::OK);
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login_keep").is_none(), "{values}");
	assert_eq!(
		cookie_rows(&app.db_path, id).await,
		vec![("auth.example.org".into(), "session".into(), "s2".into())]
	);

	// an explicit confirm (clearCookies: true) overrides the setting: cookies are cleared too
	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login_keep", &[("session", "s2")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app.app,
		"POST",
		&logout_path,
		Some(json!({"key": "login_keep", "clearCookies": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}

#[tokio::test]
async fn logout_scopes_deletion_to_the_key_prefix() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");
	let logout_path = format!("/api/v1/sources/{id}/login/logout");

	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login", &[("session", "s1")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// undeclared key: idempotent 200, and "login"'s state is not collateral damage of the "login_" family
	let (status, _) = request(&app.app, "POST", &logout_path, Some(json!({"key": "login_keep_extra"}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(settings_values(&app.app, id).await["login"], "logged_in");

	// likewise, prefix deletion for "login" must not take "login_keep" with it
	let (status, _) = request(
		&app.app,
		"POST",
		&login_path,
		Some(web_login_body("login_keep", &[("session", "s2")])),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&app.app, "POST", &logout_path, Some(json!({"key": "login"}))).await;
	assert_eq!(status, StatusCode::OK);
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "{values}");
	assert_eq!(values["login_keep"], "logged_in");
}

#[tokio::test]
async fn web_login_rejects_bad_requests() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;
	let login_path = format!("/api/v1/sources/{id}/login/web");

	// empty key
	let (status, body) = request(&app.app, "POST", &login_path, Some(web_login_body("", &[("session", "s1")]))).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	// uninstalled source
	let (status, body) = request(
		&app.app,
		"POST",
		"/api/v1/sources/999/login/web",
		Some(web_login_body("login", &[("session", "s1")])),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

	// key without a matching login setting (a text setting is treated the same as an unknown key)
	for key in ["auth_url", "nope"] {
		let (status, body) = request(&app.app, "POST", &login_path, Some(web_login_body(key, &[("session", "s1")]))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
	}

	// cookie name/value carrying injection characters
	for cookies in [
		json!({"a;b": "v"}),
		json!({"": "v"}),
		json!({"a": "v\nb"}),
	] {
		let (status, body) = request(&app.app, "POST", &login_path, Some(json!({"key": "login", "cookies": cookies}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{cookies}: {body}");
	}

	// the state stays clean after all the bad requests
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "{values}");
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}
