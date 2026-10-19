//! OAuth 登录（method: "oauth"）的端点测试。
//! fixture 源：oauth（三个 login 设置——login_oauth 走 pkce+token_url 交换分支，
//! login_oauth_plain 走回调串+notification 分支，login_oauth_key 走 url_key 兜底）。
//! 令牌交换的上游是本地 mock（axum + 127.0.0.1:0），捕获请求体验证参数不编码。
//! fixture 重建：`cd crates/wasm/tests/fixtures/oauth-src &&
//! cargo build --release --target wasm32-unknown-unknown`，产物复制为
//! crates/wasm/tests/fixtures/oauth/main.wasm。
mod common;

use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::Router;
use base64::Engine;
use common::{TestApp, request, test_app};
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::{Arc, Mutex};

fn install_oauth_source(tmp: &tempfile::TempDir) {
	let dir = tmp.path().join("sources").join("en.oauth");
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(common::oauth_wasm_path(), dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": "en.oauth", "name": "en.oauth", "version": 1, "languages": ["en"], "url": "https://base.example.net"}
		})
		.to_string(),
	)
	.expect("write source.json");
}

async fn scan(app: &Router) {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
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

async fn app_with_oauth_source() -> (TestApp, i64) {
	let app = test_app().await;
	install_oauth_source(&app.tmp);
	scan(&app.app).await;
	let id = source_id_by_key(&app.app, "en.oauth").await;
	(app, id)
}

async fn settings_body(app: &Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK);
	body
}

async fn settings_values(app: &Router, source_id: i64) -> Value {
	settings_body(app, source_id).await["values"].clone()
}

fn echo_title(body: &Value) -> String {
	body["settings"]
		.as_array()
		.expect("settings array")
		.iter()
		.find(|s| s["key"] == "echo")
		.and_then(|s| s["title"].as_str())
		.expect("echo setting")
		.to_string()
}

// ---- 本地 mock 令牌端点：捕获请求体与 Content-Type，返回原始响应串 ----

#[derive(Clone, Default)]
struct MockToken {
	captured: Arc<Mutex<Vec<(String, String)>>>,
}

async fn token_handler(AxumState(mock): AxumState<MockToken>, headers: HeaderMap, body: axum::body::Bytes) -> &'static str {
	let content_type = headers
		.get(axum::http::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.unwrap_or_default()
		.to_string();
	mock
		.captured
		.lock()
		.expect("mock lock")
		.push((content_type, String::from_utf8(body.to_vec()).expect("utf8 body")));
	r#"{"access_token":"tok123","token_type":"Bearer","expires_in":3600}"#
}

struct TokenServer {
	url: String,
	mock: MockToken,
	serve: tokio::task::JoinHandle<()>,
}

impl Drop for TokenServer {
	fn drop(&mut self) {
		self.serve.abort();
	}
}

async fn serve_token() -> TokenServer {
	let mock = MockToken::default();
	let app = axum::Router::new()
		.route("/token", axum::routing::post(token_handler))
		.with_state(mock.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind mock token");
	let addr = listener.local_addr().expect("local addr");
	let serve = tokio::spawn(async move {
		axum::serve(listener, app).await.expect("serve mock token");
	});
	TokenServer {
		url: format!("http://{addr}/token"),
		mock,
		serve,
	}
}

const TOKEN_BODY: &str = r#"{"access_token":"tok123","token_type":"Bearer","expires_in":3600}"#;

async fn put_token_url(app: &Router, id: i64, url: &str) {
	let (status, body) = request(
		app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"oauth_token_url": url})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

fn oauth_start_path(id: i64) -> String {
	format!("/api/v1/sources/{id}/login/oauth/start")
}

fn oauth_complete_path(id: i64) -> String {
	format!("/api/v1/sources/{id}/login/oauth/complete")
}

fn code_challenge(verifier: &str) -> String {
	let digest = sha2::Sha256::digest(verifier.as_bytes());
	base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[tokio::test]
async fn oauth_start_builds_pkce_url_and_persists_verifier() {
	let (app, id) = app_with_oauth_source().await;

	let (status, body) = request(&app.app, "POST", &oauth_start_path(id), Some(json!({"key": "login_oauth"}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let values = settings_values(&app.app, id).await;
	let verifier = values["login_oauth.codeVerifier"]
		.as_str()
		.expect("verifier persisted at <key>.codeVerifier");
	assert_eq!(verifier.len(), 128, "Aidoku verifier 固定 128 字符: {verifier}");
	assert!(
		verifier
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')),
		"verifier 只能取 unreserved 集: {verifier}"
	);
	// challenge 必须能由落库的 verifier 重算：非填充 base64url(SHA-256(verifier))
	assert_eq!(
		body["url"].as_str().expect("url"),
		format!(
			"https://auth.example.com/authorize?client_id=c1&redirect_uri=oauthdemo%3A%2F%2Fcb&code_challenge={}&code_challenge_method=S256&response_type=code",
			code_challenge(verifier)
		)
	);
}

#[tokio::test]
async fn oauth_start_resolves_url_from_url_key_default() {
	let (app, id) = app_with_oauth_source().await;

	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_start_path(id),
		Some(json!({"key": "login_oauth_key"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let values = settings_values(&app.app, id).await;
	let verifier = values["login_oauth_key.codeVerifier"]
		.as_str()
		.expect("verifier persisted");
	// url_key 指向 oauth_auth_url 设置的默认值，pkce 参数照常追加
	assert_eq!(
		body["url"].as_str().expect("url"),
		format!(
			"https://auth.example.org/authorize?client_id=c2&redirect_uri=https%3A%2F%2Fapp.example.io%2Fcb&code_challenge={}&code_challenge_method=S256&response_type=code",
			code_challenge(verifier)
		)
	);
}

#[tokio::test]
async fn oauth_start_without_pkce_returns_url_unchanged() {
	let (app, id) = app_with_oauth_source().await;

	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_start_path(id),
		Some(json!({"key": "login_oauth_plain"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["url"], "https://auth.example.com/plain");
	let values = settings_values(&app.app, id).await;
	assert!(
		values.get("login_oauth_plain.codeVerifier").is_none(),
		"无 pkce 不得生成 verifier: {values}"
	);
}

#[tokio::test]
async fn oauth_complete_exchanges_code_and_stores_raw_response() {
	let server = serve_token().await;
	let (app, id) = app_with_oauth_source().await;
	put_token_url(&app.app, id, &server.url).await;

	let (status, body) = request(&app.app, "POST", &oauth_start_path(id), Some(json!({"key": "login_oauth"}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body["url"].as_str().expect("url").contains("code_challenge="));
	let values = settings_values(&app.app, id).await;
	let verifier = values["login_oauth.codeVerifier"].as_str().expect("verifier");

	// code 里带 / = +：落进请求体必须原样（Aidoku 的交换参数不做 percent-encode）
	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_complete_path(id),
		Some(json!({"key": "login_oauth", "url": "oauthdemo://cb?code=ab%2Fcd%3D%2B&state=x"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["login_oauth"], TOKEN_BODY);

	let (content_type, exchanged) = {
		let captured = server.mock.captured.lock().expect("mock lock");
		assert_eq!(captured.len(), 1, "令牌端点应只收到一次请求");
		captured[0].clone()
	};
	assert_eq!(content_type, "application/x-www-form-urlencoded");
	assert_eq!(
		exchanged,
		format!(
			"grant_type=authorization_code&code=ab/cd=+&code_verifier={verifier}&redirect_uri=oauthdemo://cb&client_id=c1"
		),
		"交换参数逐字节对齐 Aidoku：不编码，redirect_uri/client_id 取自授权地址"
	);

	// 登录触发实例重启：新实例从库回填 defaults，运行时读到的应与库一致
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["entries"][0]["title"], format!("token={TOKEN_BODY}"));
}

#[tokio::test]
async fn oauth_complete_without_token_url_stores_callback_and_notifies() {
	let (app, id) = app_with_oauth_source().await;

	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_complete_path(id),
		Some(json!({"key": "login_oauth_plain", "url": "myscheme://done?code=zzz"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["values"]["login_oauth_plain"], "myscheme://done?code=zzz");

	// 设置的 notification 送达源：fixture 把送达串编进 echo 标题
	let body = settings_body(&app.app, id).await;
	assert_eq!(echo_title(&body), "oauth-logged-in");
}

#[tokio::test]
async fn oauth_logout_clears_key_and_code_verifier() {
	let server = serve_token().await;
	let (app, id) = app_with_oauth_source().await;
	put_token_url(&app.app, id, &server.url).await;

	let (status, _) = request(&app.app, "POST", &oauth_start_path(id), Some(json!({"key": "login_oauth"}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app.app,
		"POST",
		&oauth_complete_path(id),
		Some(json!({"key": "login_oauth", "url": "oauthdemo://cb?code=c1"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login_oauth").is_some(), "{values}");
	assert!(values.get("login_oauth.codeVerifier").is_some(), "{values}");

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/logout"),
		Some(json!({"key": "login_oauth"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let values = settings_values(&app.app, id).await;
	for key in ["login_oauth", "login_oauth.codeVerifier"] {
		assert!(values.get(key).is_none(), "登出应清掉 {key}: {values}");
	}
}

#[tokio::test]
async fn oauth_login_persists_across_server_restart() {
	let server = serve_token().await;
	let tmp = tempfile::tempdir().expect("tempdir");

	// 第一次启动：走完 start → complete，令牌落库
	{
		let app = sumidoku_server::build_app(common::base_config(&tmp))
			.await
			.expect("build app");
		install_oauth_source(&tmp);
		scan(&app).await;
		let id = source_id_by_key(&app, "en.oauth").await;
		put_token_url(&app, id, &server.url).await;
		let (status, _) = request(&app, "POST", &oauth_start_path(id), Some(json!({"key": "login_oauth"}))).await;
		assert_eq!(status, StatusCode::OK);
		let (status, body) = request(
			&app,
			"POST",
			&oauth_complete_path(id),
			Some(json!({"key": "login_oauth", "url": "oauthdemo://cb?code=c1"})),
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{body}");
		assert_eq!(body["values"]["login_oauth"], TOKEN_BODY);
	}

	// 第二次启动（同一数据目录，模拟进程重启）：设置值从库读回，
	// 新实例懒加载时也须从库回填到登录态
	let app = sumidoku_server::build_app(common::base_config(&tmp))
		.await
		.expect("build app");
	let id = source_id_by_key(&app, "en.oauth").await;
	let values = settings_values(&app, id).await;
	assert_eq!(values["login_oauth"], TOKEN_BODY, "{values}");
	assert!(values.get("login_oauth.codeVerifier").is_some(), "{values}");
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["entries"][0]["title"], format!("token={TOKEN_BODY}"));
}

#[tokio::test]
async fn oauth_rejects_bad_requests() {
	let server = serve_token().await;
	let (app, id) = app_with_oauth_source().await;
	put_token_url(&app.app, id, &server.url).await;

	// 空 key / 未安装源 / key 撞在非 login 设置上 / 未知键
	for (path, body, want) in [
		(oauth_start_path(id), json!({"key": ""}), StatusCode::BAD_REQUEST),
		(oauth_start_path(999), json!({"key": "login_oauth"}), StatusCode::NOT_FOUND),
		(oauth_start_path(id), json!({"key": "echo"}), StatusCode::BAD_REQUEST),
		(oauth_start_path(id), json!({"key": "nope"}), StatusCode::BAD_REQUEST),
	] {
		let (status, body) = request(&app.app, "POST", &path, Some(body)).await;
		assert_eq!(status, want, "{body}");
	}

	// complete：空 url / 不可解析的回调串
	for paste in ["", "not a url"] {
		let (status, body) = request(
			&app.app,
			"POST",
			&oauth_complete_path(id),
			Some(json!({"key": "login_oauth", "url": paste})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{paste}: {body}");
	}

	// 交换分支没走过 start（库无 verifier）
	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_complete_path(id),
		Some(json!({"key": "login_oauth", "url": "oauthdemo://cb?code=c1"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	// 走过 start 但回调里缺 code
	let (status, _) = request(&app.app, "POST", &oauth_start_path(id), Some(json!({"key": "login_oauth"}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(
		&app.app,
		"POST",
		&oauth_complete_path(id),
		Some(json!({"key": "login_oauth", "url": "oauthdemo://cb"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	// 全部坏请求之后状态保持干净：无 marker、无令牌交换发生
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login_oauth").is_none(), "{values}");
	assert!(server.mock.captured.lock().expect("mock lock").is_empty());
}
