//! User management and role gating: the full admin flow, reader 403 on the admin surface,
//! refusal when auth is disabled, and delete-account cascade.
//! A reader's own user state (library/progress/categories/incognito/download rows) is unrestricted,
//! guaranteed wholesale by user_isolation.rs

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{base_config, connect_db, fixture_dir, request, request_with_headers, source_by_key, test_app};
use serde_json::{Value, json};
use sumidoku_server::build_app;

const ADMIN_PW: &str = "secret";
const BOB_PW: &str = "hunter2";

async fn login(app: &Router, username: &str, password: &str) -> (StatusCode, String) {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": username, "password": password})),
	)
	.await;
	let token = body["token"].as_str().unwrap_or("").to_string();
	(status, format!("Bearer {token}"))
}

async fn as_user(app: &Router, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(app, method, path, body, &[("authorization", token)]).await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

/// Password-enabled instance + admin token; the app has only the built-in admin
async fn admin_app() -> (Router, tempfile::TempDir, String) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.auth.password = Some(ADMIN_PW.into());
	let app = build_app(config).await.expect("build app");
	let (status, admin) = login(&app, "admin", ADMIN_PW).await;
	assert_eq!(status, StatusCode::OK);
	(app, tmp, admin)
}

/// Creates a reader account via the admin endpoint and logs in, returning (user id, token)
async fn create_reader(app: &Router, admin: &str) -> (i64, String) {
	let (status, body) = as_user(
		app,
		admin,
		"POST",
		"/api/v1/users",
		Some(json!({"username": "bob", "password": BOB_PW, "role": "reader"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let id = body["id"].as_i64().expect("user id");
	let (status, bob) = login(app, "bob", BOB_PW).await;
	assert_eq!(status, StatusCode::OK);
	(id, bob)
}

async fn example_source_id(app: &Router, admin: &str) -> i64 {
	let (status, _) = as_user(app, admin, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_user(app, admin, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id")
}

#[tokio::test]
async fn user_crud_full_flow() {
	let (app, _tmp, admin) = admin_app().await;

	// the list carries no credential material
	let (status, body) = as_user(&app, &admin, "GET", "/api/v1/users", None).await;
	assert_eq!(status, StatusCode::OK);
	let users = body.as_array().expect("users");
	assert_eq!(users.len(), 1);
	assert_eq!(users[0]["username"], "admin");
	assert_eq!(users[0]["role"], "admin");
	assert!(users[0].get("passwordHash").is_none());
	assert!(users[0].get("password_hash").is_none());

	let (bob_id, bob) = create_reader(&app, &admin).await;
	let (status, body) = as_user(&app, &admin, "GET", "/api/v1/users", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("users").len(), 2);

	// role change: the admin surface that 403'd bob before lets him through once promoted to admin
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/users", None).await;
	assert_eq!(status, StatusCode::FORBIDDEN);
	let (status, body) = as_user(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/users/{bob_id}"),
		Some(json!({"role": "admin"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["role"], "admin");
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/users", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_user(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/users/{bob_id}"),
		Some(json!({"role": "reader"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["role"], "reader");

	// rename: the old name 401s, the new one logs in
	let (status, body) = as_user(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/users/{bob_id}"),
		Some(json!({"username": "robert"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["username"], "robert");
	let (status, _) = login(&app, "bob", BOB_PW).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, robert) = login(&app, "robert", BOB_PW).await;
	assert_eq!(status, StatusCode::OK);

	// password reset: revokes all of the user's sessions; old password dead, new password live
	let (status, _) = as_user(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/users/{bob_id}"),
		Some(json!({"password": "new-secret"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &robert, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "robert", BOB_PW).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "robert", "new-secret").await;
	assert_eq!(status, StatusCode::OK);

	// delete: the list drops back, login 401s
	let (status, _) = as_user(&app, &admin, "DELETE", &format!("/api/v1/users/{bob_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, body) = as_user(&app, &admin, "GET", "/api/v1/users", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("users").len(), 1);
	let (status, _) = login(&app, "robert", "new-secret").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_user_validation() {
	let (app, _tmp, admin) = admin_app().await;
	for body in [
		json!({"username": "  ", "password": "x", "role": "reader"}),
		json!({"username": "bob", "password": "", "role": "reader"}),
		json!({"username": "bob", "password": "x", "role": "owner"}),
	] {
		let (status, _) = as_user(&app, &admin, "POST", "/api/v1/users", Some(body)).await;
		assert_eq!(status, StatusCode::BAD_REQUEST);
	}
	// duplicate name: both create and rename hit 409
	create_reader(&app, &admin).await;
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		"/api/v1/users",
		Some(json!({"username": "bob", "password": "x", "role": "reader"})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "duplicate");
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		"/api/v1/users",
		Some(json!({"username": "carol", "password": "x", "role": "admin"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let carol_id = body["id"].as_i64().expect("carol id");
	let (status, _) = as_user(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/users/{carol_id}"),
		Some(json!({"username": "bob"})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn builtin_admin_and_missing_user_guards() {
	let (app, _tmp, admin) = admin_app().await;
	let (status, _) = as_user(&app, &admin, "DELETE", "/api/v1/users/1", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let (status, _) = as_user(&app, &admin, "DELETE", "/api/v1/users/99", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = as_user(&app, &admin, "PATCH", "/api/v1/users/99", Some(json!({"role": "reader"}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// reader on the admin surface always gets 403 (code forbidden); own user state and the all-open surface are unaffected
#[tokio::test]
async fn reader_forbidden_on_admin_surface() {
	let (app, _tmp, admin) = admin_app().await;
	let source_id = example_source_id(&app, &admin).await;
	let (_bob_id, bob) = create_reader(&app, &admin).await;

	let admin_surface: Vec<(&str, String, Option<Value>)> = vec![
		("GET", "/api/v1/users".into(), None),
		("POST", "/api/v1/users".into(), Some(json!({"username": "x", "password": "x", "role": "reader"}))),
		("PATCH", "/api/v1/users/1".into(), Some(json!({"role": "reader"}))),
		("DELETE", "/api/v1/users/2".into(), None),
		("POST", "/api/v1/sources/scan".into(), None),
		("POST", "/api/v1/sources/reorder-pinned".into(), Some(json!({"ids": []}))),
		("PATCH", format!("/api/v1/sources/{source_id}"), Some(json!({"pinned": true}))),
		("DELETE", format!("/api/v1/sources/{source_id}"), None),
		("POST", format!("/api/v1/sources/{source_id}/update"), None),
		("GET", format!("/api/v1/sources/{source_id}/settings"), None),
		("PUT", format!("/api/v1/sources/{source_id}/settings"), Some(json!({}))),
		("POST", format!("/api/v1/sources/{source_id}/settings/reset"), None),
		("POST", format!("/api/v1/sources/{source_id}/notify"), Some(json!({"notification": "x"}))),
		("POST", format!("/api/v1/sources/{source_id}/cache/clear"), None),
		("POST", format!("/api/v1/sources/{source_id}/login/basic"), Some(json!({"key": "k", "username": "u", "password": "p"}))),
		("POST", format!("/api/v1/sources/{source_id}/login/logout"), None),
		("GET", "/api/v1/source-repos".into(), None),
		("POST", "/api/v1/source-repos".into(), Some(json!({"url": "https://example.com/repo"}))),
		("GET", "/api/v1/source-repos/sources".into(), None),
		("POST", "/api/v1/source-repos/install".into(), Some(json!({}))),
		("POST", "/api/v1/source-repos/update-all".into(), None),
		("GET", "/api/v1/backups".into(), None),
		("POST", "/api/v1/backups/restore".into(), Some(json!({"name": "sumidoku-backup-20260101-000000.json"}))),
		("PUT", "/api/v1/settings".into(), Some(json!({"backup.enabled": true}))),
		("PUT", "/api/v1/settings".into(), Some(json!({"browse.pinned_sources": []}))),
	];
	for (method, path, body) in admin_surface {
		let (status, resp) = as_user(&app, &bob, method, &path, body).await;
		assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
		assert_eq!(resp["error"]["code"], "forbidden", "{method} {path}");
	}
	// the settings/sourceSettings sections of a restore are global state: reader gets 403 for including them, pure user-state sections pass
	let (status, _) = as_user(
		&app,
		&bob,
		"POST",
		"/api/v1/backup?sections=settings",
		Some(json!({"version": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::FORBIDDEN);
	let (status, _) = as_user(
		&app,
		&bob,
		"POST",
		"/api/v1/backup?sections=library,progress",
		Some(json!({"version": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	// omitting sections means everything (including settings), likewise refused
	let (status, _) = as_user(&app, &bob, "POST", "/api/v1/backup", Some(json!({"version": 1}))).await;
	assert_eq!(status, StatusCode::FORBIDDEN);

	// the all-open surface: export own backup, trigger update manually, user-state keys, own library and categories
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/backup", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "POST", "/api/v1/categories", Some(json!({"name": "mine"}))).await;
	assert_eq!(status, StatusCode::OK);
	// the read surface (browse/search) works as usual for reader
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		&app,
		&bob,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
}

/// Auth-disabled (no password set) deployment: user management is always refused, other surfaces stay open
#[tokio::test]
async fn auth_disabled_refuses_user_endpoints() {
	let app = test_app().await;
	for (method, path, body) in [
		("GET", "/api/v1/users", None),
		("POST", "/api/v1/users", Some(json!({"username": "x", "password": "x", "role": "reader"}))),
		("PATCH", "/api/v1/users/2", Some(json!({"role": "admin"}))),
		("DELETE", "/api/v1/users/2", None),
	] {
		let (status, resp) = request(&app.app, method, path, body).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}");
		assert_eq!(resp["error"]["message"], "authentication is not enabled", "{method} {path}");
	}
	let (status, _) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
}

/// Delete cascade: the user's state rows (library/progress/categories/downloads/tokens/user-state setting keys) are all wiped; admin's rows stay as-is
#[tokio::test]
async fn delete_user_cascades_own_state() {
	let (app, tmp, admin) = admin_app().await;
	let source_id = example_source_id(&app, &admin).await;
	let (bob_id, bob) = create_reader(&app, &admin).await;

	// admin registers the manga, bob accumulates user state: category + shelve + progress + incognito + download row
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");
	let (status, body) = as_user(&app, &admin, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");

	let (status, body) = as_user(&app, &bob, "POST", "/api/v1/categories", Some(json!({"name": "bob-cat"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_id = body["id"].as_i64().expect("category id");
	let (status, _) = as_user(
		&app,
		&bob,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [cat_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		&app,
		&bob,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 3, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		&app,
		&bob,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	// admin shelves the same one too, to reconcile that "only bob's rows are wiped"
	let (status, _) = as_user(&app, &admin, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	let (status, _) = as_user(&app, &admin, "DELETE", &format!("/api/v1/users/{bob_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);

	let pool = connect_db(&tmp.path().join("test.db")).await;
	for (table, column) in [
		("manga_user", "user_id"),
		("chapter_user", "user_id"),
		("category", "user_id"),
		("category_manga", "user_id"),
		("download", "user_id"),
		("auth_token", "user_id"),
	] {
		let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?"))
			.bind(bob_id)
			.fetch_one(&pool)
			.await
			.expect("count rows");
		assert_eq!(count, 0, "{table} rows for deleted user");
	}
	let namespaced: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key LIKE ?")
		.bind(format!("user.{bob_id}.%"))
		.fetch_one(&pool)
		.await
		.expect("count namespaced keys");
	assert_eq!(namespaced, 0);
	// admin's user state stays as-is
	let admin_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE user_id = 1")
		.fetch_one(&pool)
		.await
		.expect("admin manga_user rows");
	assert_eq!(admin_rows, 1);
	pool.close().await;

	// bob's token 401s after the deletion; admin is unaffected
	let (status, _) = as_user(&app, &bob, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, body) = as_user(&app, &admin, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);
}

/// /auth/status also returns the current credential's identity: the SPA uses it to restore the account display after a refresh
#[tokio::test]
async fn auth_status_reports_whoami() {
	let (app, _tmp, admin) = admin_app().await;
	let (_bob_id, bob) = create_reader(&app, &admin).await;

	let (status, body) = as_user(&app, &admin, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], true);
	assert_eq!(body["user"], json!({"id": 1, "username": "admin", "role": "admin"}));
	let (status, body) = as_user(&app, &bob, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"]["username"], "bob");
	assert_eq!(body["user"]["role"], "reader");
	// no credentials and bad credentials: user is null; the endpoint itself is always public
	let (status, body) = request(&app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"], Value::Null);

	let open = test_app().await;
	let (status, body) = request(&open.app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], false);
	assert_eq!(body["user"], Value::Null);
}

/// Decision point 11: a reader export contains only self-data — global settings and all of
/// sourceSettings (source login credentials) stay out of the document; own library/progress sections
/// and the bare keys translated back from the own namespace are present; admin gets everything
#[tokio::test]
async fn backup_export_scopes_self_data_for_reader() {
	let (app, _tmp, admin) = admin_app().await;
	let source_id = example_source_id(&app, &admin).await;
	let (_bob_id, bob) = create_reader(&app, &admin).await;

	// one of each piece of global state: a config key + source credentials (via the real write path)
	let (status, _) = as_user(&app, &admin, "PUT", "/api/v1/settings", Some(json!({"backup.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		&app,
		&admin,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"password": "site-credential"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// bob accumulates his own user state: shelve + progress + incognito
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = as_user(
		&app,
		&admin,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");
	let (status, body) = as_user(&app, &bob, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = as_user(&app, &bob, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		&app,
		&bob,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 4})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&app, &bob, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);

	let (status, doc) = as_user(&app, &bob, "GET", "/api/v1/backup", None).await;
	assert_eq!(status, StatusCode::OK);
	let settings = doc["settings"].as_object().expect("settings object");
	// only the bare keys translated back from his own namespace remain: global keys (backup.enabled etc.) and others' keys are all absent
	assert_eq!(settings.len(), 1, "reader export settings: {settings:?}");
	assert_eq!(settings["incognito.enabled"], true);
	let source_settings = doc["sourceSettings"].as_object().expect("sourceSettings object");
	assert!(source_settings.is_empty(), "reader export sourceSettings: {source_settings:?}");
	assert_eq!(doc["library"].as_array().expect("library").len(), 1);
	assert_eq!(doc["progress"].as_array().expect("progress").len(), 1);

	// admin's export is unchanged: global keys and source credentials are all present
	let (status, doc) = as_user(&app, &admin, "GET", "/api/v1/backup", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(doc["settings"]["backup.enabled"], true);
	assert_eq!(
		doc["sourceSettings"]["en.example-source"]["password"],
		json!("site-credential")
	);
}
