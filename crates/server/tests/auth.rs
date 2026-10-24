mod common;

use axum::Router;
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use sumidoku_server::build_app;

const PASSWORD: &str = "secret";

/// Under the unified credential model basic_password is the legacy alias of auth.password and likewise enables site-wide auth
async fn password_app() -> (Router, tempfile::TempDir) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.basic_password = Some(PASSWORD.into());
	let app = build_app(config).await.expect("build app");
	(app, tmp)
}

async fn login(app: &Router, username: &str, password: &str) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": username, "password": password})),
		&[],
	)
	.await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

#[tokio::test]
async fn auth_disabled_by_default() {
	let app = test_app().await;
	let (status, body) = request(&app.app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], false);
	// with no password configured the site is fully open (compatibility contract)
	let (status, _) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn requires_credentials() {
	let (app, _tmp) = password_app().await;
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(
		body,
		json!({"error": {"code": "unauthorized", "message": "authentication required"}})
	);
	// the status endpoint is always public; the SPA relies on it to decide whether to jump to the login page
	let (status, body) = request(&app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], true);
}

/// Security errors are unified externally: no credentials, bad token, wrong password, and nonexistent user all look the same to the client
#[tokio::test]
async fn uniform_401_shape() {
	let (app, _tmp) = password_app().await;
	let (status, no_cred) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _, bytes) = request_with_headers(
		&app,
		"GET",
		"/api/v1/sources",
		None,
		&[("authorization", "Bearer nope")],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let bad_token: Value = serde_json::from_slice(&bytes).expect("json");
	assert_eq!(no_cred, bad_token);
	let (status, wrong_login) = login(&app, "admin", "wrong").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(no_cred, wrong_login);
	// nonexistent user and wrong password are indistinguishable
	let (status, no_such_user) = login(&app, "nobody", PASSWORD).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(no_cred, no_such_user);
}

#[tokio::test]
async fn login_token_flow() {
	let (app, _tmp) = password_app().await;
	let (status, body) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
	let token = body["token"].as_str().expect("token");
	assert!(!token.is_empty());
	assert!(body["expiresAt"].as_i64().expect("expiresAt") > 0);
	assert_eq!(body["user"], json!({"id": 1, "username": "admin"}));

	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::OK);

	// <img>/WebSocket can't send the Authorization header, so media and streaming endpoints accept the token via query
	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/sources?access_token={token}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn session_cookie_is_limited_to_events() {
	let (app, _tmp) = password_app().await;
	let (status, headers, bytes) = request_with_headers(
		&app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": "admin", "password": PASSWORD})),
		&[("x-forwarded-proto", "https")],
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let set_cookie = headers.get("set-cookie").expect("session cookie").to_str().expect("cookie text");
	assert!(set_cookie.contains("HttpOnly"));
	assert!(set_cookie.contains("SameSite=Strict"));
	assert!(set_cookie.contains("Secure"));
	let body: Value = serde_json::from_slice(&bytes).expect("login json");
	let token = body["token"].as_str().expect("token");
	let cookie = format!("sumidoku.session={token}");
	let bearer = format!("Bearer {token}");
	let (status, headers, _) = request_with_headers(&app, "GET", "/api/v1/auth/status", None, &[("authorization", &bearer)]).await;
	assert_eq!(status, StatusCode::OK);
	let refreshed = headers.get("set-cookie").expect("refreshed session cookie").to_str().expect("cookie text");
	assert!(refreshed.contains("sumidoku.session="));
	assert!(refreshed.contains("Max-Age=2592000"));
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("cookie", &cookie)]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let addr = serve_app(app.clone()).await;
	let (_query_stream, _query_rest, query_head) =
		connect_sse_response(addr, &format!("/api/v1/events?access_token={token}"), None, &[]).await;
	assert!(query_head.contains("401 Unauthorized"), "{query_head}");
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", Some(0), &[("Cookie", &cookie)]).await;
	let snapshot = read_sse_kind(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["data"]["type"], "snapshot");
	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(&app, "POST", "/api/v1/auth/logout", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, headers, _) = request_with_headers(&app, "POST", "/api/v1/auth/logout", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let cleared = headers.get("set-cookie").expect("clear cookie").to_str().expect("clear text");
	assert!(cleared.contains("Max-Age=0"));
}

/// A session token belongs to the logged-in user in the DB
#[tokio::test]
async fn token_carries_user_identity() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	let app = build_app(config).await.expect("build app");
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let token = body["token"].as_str().expect("token");
	let pool = connect_db(&tmp.path().join("test.db")).await;
	let user_id: i64 = sqlx::query_scalar("SELECT user_id FROM auth_token WHERE token_hash = ?")
		.bind(sumidoku_server::auth::sha256_hex(token))
		.fetch_one(&pool)
		.await
		.expect("token row");
	assert_eq!(user_id, 1);
}

#[tokio::test]
async fn logout_revokes_token() {
	let (app, _tmp) = password_app().await;
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let token = body["token"].as_str().expect("token").to_string();
	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(&app, "POST", "/api/v1/auth/logout", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// When a user is deleted, their session tokens are wiped by the foreign-key cascade
#[tokio::test]
async fn token_dies_with_its_user() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	let app = build_app(config).await.expect("build app");
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let auth = format!("Bearer {}", body["token"].as_str().expect("token"));
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&tmp.path().join("test.db")).await;
	sqlx::query("DELETE FROM user WHERE id = 1")
		.execute(&pool)
		.await
		.expect("delete user");
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn change_password() {
	let (app, _tmp) = password_app().await;
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let token = body["token"].as_str().expect("token").to_string();
	let auth = format!("Bearer {token}");
	// a second session, to tell apart the scope of "revoke all of one's own tokens"
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let other = format!("Bearer {}", body["token"].as_str().expect("token"));

	// wrong old password: the change is rejected
	let (status, _, _) = request_with_headers(
		&app,
		"PUT",
		"/api/v1/auth/password",
		Some(json!({"currentPassword": "wrong", "newPassword": "new-secret"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);

	let (status, headers, _) = request_with_headers(
		&app,
		"PUT",
		"/api/v1/auth/password",
		Some(json!({"currentPassword": PASSWORD, "newPassword": "new-secret"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let cleared = headers.get("set-cookie").expect("clear cookie").to_str().expect("clear text");
	assert!(cleared.contains("Max-Age=0"));

	// changing the password revokes all of the user's session tokens
	for auth in [&auth, &other] {
		let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", auth)]).await;
		assert_eq!(status, StatusCode::UNAUTHORIZED);
	}
	// the old password is dead, the new one logs in
	let (status, _) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "admin", "new-secret").await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn public_surface_stays_open() {
	let (app, _tmp) = password_app().await;
	let (status, _) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	// the version string is not sensitive; like /health it skips auth
	let (status, body) = request(&app, "GET", "/api/v1/version", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
	// static assets are always public so the SPA can load the login page
	let (status, _) = request(&app, "GET", "/", None).await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn auth_password_config_enables_auth() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	let app = build_app(config).await.expect("build app");
	let (status, body) = request(&app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], true);
	let (status, _) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn auth_password_wins_over_legacy_alias() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	config.basic_password = Some("legacy-only".into());
	let app = build_app(config).await.expect("build app");
	// when both are configured, the legacy alias has no effect
	let (status, _) = login(&app, "admin", "legacy-only").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn api_keys_authenticate_without_login() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	config.auth.api_keys = vec!["smk_test_key".into()];
	let app = build_app(config).await.expect("build app");
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/api/v1/sources",
		None,
		&[("authorization", "Bearer smk_test_key")],
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, headers, _) = request_with_headers(
		&app,
		"GET",
		"/api/v1/auth/status",
		None,
		&[("authorization", "Bearer smk_test_key")],
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(headers.get("set-cookie").is_none());
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/api/v1/sources",
		None,
		&[("authorization", "Bearer smk_wrong_key")],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The configured password overwrites the in-DB hash on every startup: a changed password yields to the config on restart (headless deployment semantics)
#[tokio::test]
async fn config_password_reseeds_on_restart() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(PASSWORD.into());
	let app = build_app(config.clone()).await.expect("build app");
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let token = body["token"].as_str().expect("token").to_string();
	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(
		&app,
		"PUT",
		"/api/v1/auth/password",
		Some(json!({"currentPassword": PASSWORD, "newPassword": "new-secret"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	drop(app);

	let app = build_app(config).await.expect("rebuild app");
	let (status, _) = login(&app, "admin", "new-secret").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
}

/// Old-DB migration: auth_credential's site password becomes the admin password, old token sessions
/// are kept and assigned to user 1, the incognito key moves into the user.1 namespace, and download
/// rows are kept with the table gaining a composite primary key
#[tokio::test]
async fn migration_from_site_password_era() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let db_url = format!("sqlite://{}/test.db?mode=rwc", tmp.path().display());
	let pool = sqlx::SqlitePool::connect(&db_url).await.expect("pool");
	// replay the migrations before 0012 with bookkeeping filled in, so build_app's migrate only runs 0012
	sqlx::raw_sql(
		"CREATE TABLE IF NOT EXISTS _sqlx_migrations (
		 version BIGINT PRIMARY KEY,
		 description TEXT NOT NULL,
		 installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
		 success BOOLEAN NOT NULL,
		 checksum BLOB NOT NULL,
		 execution_time BIGINT NOT NULL
	 )",
	)
	.execute(&pool)
	.await
	.expect("migrations table");
	let migrator = sqlx::migrate!("./migrations");
	for migration in migrator.iter() {
		if migration.version > 11 {
			continue;
		}
		sqlx::raw_sql(&migration.sql).execute(&pool).await.expect("replay migration");
		sqlx::query(
			"INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
			 VALUES (?, ?, TRUE, ?, 0)",
		)
		.bind(migration.version)
		.bind(migration.description.as_ref())
		.bind(migration.checksum.as_ref())
		.execute(&pool)
		.await
		.expect("migration bookkeeping");
	}
	// old-world relics: the site password, a session token without a user column, the global incognito key, a download table with a single-column primary key
	let site_hash = sumidoku_server::auth::hash_password(PASSWORD).expect("hash");
	sqlx::query("INSERT INTO auth_credential (id, password_hash) VALUES (1, ?)")
		.bind(&site_hash)
		.execute(&pool)
		.await
		.expect("seed credential");
	let old_token_hash = sumidoku_server::auth::sha256_hex("old-session-token");
	sqlx::query("INSERT INTO auth_token (token_hash, created_at, expires_at) VALUES (?, 1, 9999999999)")
		.bind(&old_token_hash)
		.execute(&pool)
		.await
		.expect("seed token");
	sqlx::query("INSERT INTO setting (key, value) VALUES ('incognito.enabled', 'true')")
		.execute(&pool)
		.await
		.expect("seed incognito");
	sqlx::query("INSERT INTO source (id, key, name, wasm_path) VALUES (100, 'en.x', 'X', '/x.wasm')")
		.execute(&pool)
		.await
		.expect("seed source");
	sqlx::query("INSERT INTO manga (id, source_id, key) VALUES (100, 100, 'm')")
		.execute(&pool)
		.await
		.expect("seed manga");
	sqlx::query("INSERT INTO chapter (id, manga_id, source_order, key) VALUES (100, 100, 1, 'c')")
		.execute(&pool)
		.await
		.expect("seed chapter");
	sqlx::query("INSERT INTO download (chapter_id, user_id, status) VALUES (100, 1, 'finished')")
		.execute(&pool)
		.await
		.expect("seed download");
	pool.close().await;

	let config = base_config(&tmp);
	let app = build_app(config).await.expect("build app");

	// the site password logs in as admin directly
	let (status, body) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"]["id"], 1);
	// the old token still works (the migration backfills user 1, the session is not lost)
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/api/v1/sources",
		None,
		&[("authorization", "Bearer old-session-token")],
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let pool = connect_db(&tmp.path().join("test.db")).await;
	let legacy_keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = 'incognito.enabled'")
		.fetch_one(&pool)
		.await
		.expect("count legacy key");
	assert_eq!(legacy_keys, 0);
	let namespaced: Option<(String,)> =
		sqlx::query_as("SELECT value FROM setting WHERE key = 'user.1.incognito.enabled'")
			.fetch_optional(&pool)
			.await
			.expect("namespaced key");
	assert_eq!(namespaced.map(|(v,)| v).as_deref(), Some("true"));
	let download_user: i64 = sqlx::query_scalar("SELECT user_id FROM download WHERE chapter_id = 100")
		.fetch_one(&pool)
		.await
		.expect("download row");
	assert_eq!(download_user, 1);
	let credential_cols: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('auth_credential')")
		.fetch_one(&pool)
		.await
		.expect("credential table gone");
	assert_eq!(credential_cols, 0);
}
