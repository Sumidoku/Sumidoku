mod common;

use axum::Router;
use axum::http::{StatusCode, header};
use common::*;
use serde_json::{Value, json};
use sumidoku_server::build_app;

const PASSWORD: &str = "secret";

/// 统一凭证模型下 basic_password 是 auth.password 的 legacy 别名，同样开启全站认证
async fn password_app() -> (Router, tempfile::TempDir) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.basic_password = Some(PASSWORD.into());
	let app = build_app(config).await.expect("build app");
	(app, tmp)
}

async fn login(app: &Router, password: &str) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"password": password})),
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

/// 安全类报错对外归一：无凭证、坏 token、错密码，客户端看到同一形状
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
	let (status, wrong_login) = login(&app, "wrong").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(no_cred, wrong_login);
}

#[tokio::test]
async fn login_token_flow() {
	let (app, _tmp) = password_app().await;
	let (status, body) = login(&app, PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
	let token = body["token"].as_str().expect("token");
	assert!(!token.is_empty());
	assert!(body["expiresAt"].as_i64().expect("expiresAt") > 0);

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

#[tokio::test]
async fn logout_revokes_token() {
	let (app, _tmp) = password_app().await;
	let (_, body) = login(&app, PASSWORD).await;
	let token = body["token"].as_str().expect("token").to_string();
	let auth = format!("Bearer {token}");
	let (status, _, _) = request_with_headers(&app, "POST", "/api/v1/auth/logout", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn change_password() {
	let (app, _tmp) = password_app().await;
	let (_, body) = login(&app, PASSWORD).await;
	let token = body["token"].as_str().expect("token").to_string();
	let auth = format!("Bearer {token}");

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

	// 改密码吊销全部会话 token
	let (status, _, _) = request_with_headers(&app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	// 旧密码失效，新密码可登录
	let (status, _) = login(&app, PASSWORD).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, "new-secret").await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn opds_shares_the_same_password() {
	let (app, _tmp) = password_app().await;
	let (status, headers, _) = request_with_headers(&app, "GET", "/opds/v1.2/root", None, &[]).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(
		headers[header::WWW_AUTHENTICATE],
		"Basic realm=\"Sumidoku OPDS\""
	);
	// 错误密码（user:wrong）
	let (status, _, _) = request_with_headers(
		&app,
		"GET",
		"/opds/v1.2/root",
		None,
		&[("authorization", "Basic dXNlcjp3cm9uZw==")],
	)
	.await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	// 同一密码（user:secret），v1.2 与 v2 都过
	for path in ["/opds/v1.2/root", "/opds/v2/root"] {
		let (status, _, _) = request_with_headers(
			&app,
			"GET",
			path,
			None,
			&[("authorization", "Basic dXNlcjpzZWNyZXQ=")],
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{path}");
	}
}

#[tokio::test]
async fn public_surface_stays_open() {
	let (app, _tmp) = password_app().await;
	let (status, _) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
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
	let (status, _) = login(&app, PASSWORD).await;
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
	let (status, _) = login(&app, "legacy-only").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, PASSWORD).await;
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
	let (_, body) = login(&app, PASSWORD).await;
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
	let (status, _) = login(&app, "new-secret").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	let (status, _) = login(&app, PASSWORD).await;
	assert_eq!(status, StatusCode::OK);
}
