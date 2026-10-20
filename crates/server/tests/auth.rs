mod common;

use axum::Router;
use axum::http::{StatusCode, header};
use common::*;
use serde_json::{Value, json};
use sumidoku_server::build_app;

const PASSWORD: &str = "secret";
/// base64("admin:secret")，OPDS 真实凭证时代的管理员组合
const ADMIN_BASIC: &str = "Basic YWRtaW46c2VjcmV0";

/// 统一凭证模型下 basic_password 是 auth.password 的 legacy 别名，同样开启全站认证
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
	// 未配置密码时全站开放（兼容契约）
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
	// status 端点永远公开，SPA 靠它决定要不要跳登录页
	let (status, body) = request(&app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], true);
}

/// 安全类报错对外归一：无凭证、坏 token、错密码、不存在的用户，客户端看到同一形状
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
	// 用户不存在与密码错误不可区分
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
	assert_eq!(body["user"], json!({"id": 1, "username": "admin", "role": "admin"}));

	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::OK);

	// <img>/WebSocket 发不了 Authorization 头，媒体与流端点接受 query 传 token
	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/sources?access_token={token}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
}

/// 会话 token 在库里归属登录用户
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

/// 用户被删时其会话 token 随外键级联一并清除
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
	// 第二个会话，用来区分「吊销自己全部 token」的作用范围
	let (_, body) = login(&app, "admin", PASSWORD).await;
	let other = format!("Bearer {}", body["token"].as_str().expect("token"));

	// 旧密码不对，改密码被拒
	let (status, _, _) = request_with_headers(
		&app,
		"PUT",
		"/api/v1/auth/password",
		Some(json!({"currentPassword": "wrong", "newPassword": "new-secret"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);

	let (status, _, _) = request_with_headers(
		&app,
		"PUT",
		"/api/v1/auth/password",
		Some(json!({"currentPassword": PASSWORD, "newPassword": "new-secret"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::NO_CONTENT);

	// 改密码吊销该用户全部会话 token
	for auth in [&auth, &other] {
		let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", auth)]).await;
		assert_eq!(status, StatusCode::UNAUTHORIZED);
	}
	// 旧密码失效，新密码可登录
	let (status, _) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "admin", "new-secret").await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn opds_uses_real_credentials() {
	let (app, _tmp) = password_app().await;
	let (status, headers, _) = request_with_headers(&app, "GET", "/opds/v1.2/root", None, &[]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(
		headers[header::WWW_AUTHENTICATE],
		"Basic realm=\"Sumidoku OPDS\""
	);
	// 错误密码（admin:wrong）
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/opds/v1.2/root",
		None,
		&[("authorization", "Basic YWRtaW46d3Jvbmc=")],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	// 任意用户名时代结束：user:secret 不再放行
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/opds/v1.2/root",
		None,
		&[("authorization", "Basic dXNlcjpzZWNyZXQ=")],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	// 真实用户名+密码，v1.2 与 v2 都过
	for path in ["/opds/v1.2/root", "/opds/v2/root"] {
		let (status, _, _) = request_with_headers(&app, "GET", path, None, &[("authorization", ADMIN_BASIC)]).await;
		assert_eq!(status, StatusCode::OK, "{path}");
	}
}

#[tokio::test]
async fn public_surface_stays_open() {
	let (app, _tmp) = password_app().await;
	let (status, _) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	// 版本串不敏感，与 /health 一样免鉴权
	let (status, body) = request(&app, "GET", "/api/v1/version", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
	// 静态资源永远公开，SPA 要能加载出登录页
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
	// 两个都配时 legacy 别名不生效
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

/// 配置面密码每次启动覆盖库内哈希：改过的密码遇重启以配置为准（headless 部署语义）
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

/// 旧库迁移：auth_credential 的站点密码成为 admin 密码，旧 token 会话保留并归到 user 1，
/// 隐身键搬进 user.1 命名空间，download 行保留且表变复合主键
#[tokio::test]
async fn migration_from_site_password_era() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let db_url = format!("sqlite://{}/test.db?mode=rwc", tmp.path().display());
	let pool = sqlx::SqlitePool::connect(&db_url).await.expect("pool");
	// 重放 0012 之前的迁移并补记账，build_app 的 migrate 只该跑 0012
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
	// 旧世界残骸：站点密码、无 user 列的会话 token、全局隐身键、单行主键的 download
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

	// 站点密码直接登录 admin
	let (status, body) = login(&app, "admin", PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"]["id"], 1);
	// 旧 token 仍有效（迁移回填 user 1，会话不丢）
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
