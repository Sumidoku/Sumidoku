//! Sidecar 交互式网页登录（login/web/browser）的端点测试。
//! 未配置 SUMIDOKU_NET_BROWSER_WS 时端点报 501 且设置响应不带能力标记；
//! 配置后（测试用 mock CDP 连接器）走完整管线：开会话 → mock 喂 cookie →
//! 完成收割 → 与粘贴路径同库同语义（marker/keys/values/source_cookie/重启）。
mod common;

use axum::http::StatusCode;
use common::{TestApp, request, test_app};
use serde_json::{Value, json};

async fn app_with_login_source(wasm: &std::path::Path) -> (TestApp, i64) {
	let app = test_app().await;
	let dir = app.tmp.path().join("sources").join("en.weblogin");
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(wasm, dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": "en.weblogin", "name": "en.weblogin", "version": 1, "languages": ["en"], "url": "https://base.example.net"}
		})
		.to_string(),
	)
	.expect("write source.json");
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let id = body
		.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == "en.weblogin")
		.and_then(|s| s["id"].as_i64())
		.expect("source id");
	(app, id)
}

#[tokio::test]
async fn browser_login_requires_configured_sidecar() {
	let (app, id) = app_with_login_source(&common::weblogin_wasm_path()).await;

	// 未配置 browser_ws：能力面为假，webui 据此只显示粘贴路径
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		body["capabilities"]["browserLogin"],
		Value::Bool(false),
		"未配置 sidecar 时能力标记必须为 false: {body}"
	);

	// 三个端点同口径 501 unsupported
	for (path, body) in [
		("/login/web/browser", json!({"key": "login"})),
		("/login/web/browser/complete", json!({"key": "login", "session": "s"})),
		("/login/web/browser/cancel", json!({"key": "login", "session": "s"})),
	] {
		let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{id}{path}"), Some(body)).await;
		assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}: {body}");
		assert_eq!(body["error"]["code"], "unsupported", "{path}: {body}");
	}
}

// ---- 配置 sidecar（mock CDP）后的全管线 ----

use common::browser_mock::{MockBrowserState, cookie};
use std::sync::{Arc, Mutex};

async fn app_with_browser_and_login_source(
	wasm: &std::path::Path,
) -> (TestApp, i64, Arc<Mutex<MockBrowserState>>) {
	let browser = Arc::new(Mutex::new(MockBrowserState::default()));
	let app = common::test_app_with_browser(browser.clone()).await;
	let dir = app.tmp.path().join("sources").join("en.weblogin");
	std::fs::create_dir_all(&dir).expect("mkdir fixture source");
	std::fs::copy(wasm, dir.join("main.wasm")).expect("copy fixture wasm");
	std::fs::write(
		dir.join("source.json"),
		serde_json::json!({
			"info": {"id": "en.weblogin", "name": "en.weblogin", "version": 1, "languages": ["en"], "url": "https://base.example.net"}
		})
		.to_string(),
	)
	.expect("write source.json");
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let id = body
		.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == "en.weblogin")
		.and_then(|s| s["id"].as_i64())
		.expect("source id");
	(app, id, browser)
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

async fn settings_values(app: &axum::Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK);
	body["values"].clone()
}

async fn start_session(app: &axum::Router, id: i64, key: &str) -> Value {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser"),
		Some(json!({"key": key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "start {key}: {body}");
	body
}

#[tokio::test]
async fn browser_login_pipeline_harvests_cookies_like_paste() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;

	// 能力面为真：webui 据此显示「在浏览器中打开登录页」
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["capabilities"]["browserLogin"], Value::Bool(true));

	// login_keep 走 url_key 解析：https://auth.example.org/
	let started = start_session(&app.app, id, "login_keep").await;
	let session = started["session"].as_str().expect("session id").to_string();
	assert_eq!(started["loginUrl"], "https://auth.example.org/");
	assert!(started["expiresAt"].as_i64().expect("expiresAt") > 0);
	{
		let state = browser.lock().expect("mock lock");
		assert_eq!(state.connects, 1, "一次会话一条 CDP 连接");
		assert_eq!(state.contexts, vec!["ctx-1"]);
		assert_eq!(state.viewports, vec![("ctx-1".into(), 1280, 800)]);
		assert_eq!(state.loads, vec![("ctx-1".into(), "https://auth.example.org/".into())]);
		assert_eq!(state.screencasts, vec!["ctx-1"], "开会话即起推流");
	}

	// 用户在视图里完成登录，context 里有了 cookie（含一条外域的，证明过滤）
	browser.lock().expect("mock lock").cookies.insert(
		"ctx-1".into(),
		vec![
			cookie("session", "s2", ".auth.example.org"),
			cookie("other", "o1", "auth.example.org"),
			cookie("foreign", "f1", "other.net"),
		],
	);

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login_keep", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	// 与粘贴路径同库同语义：marker、keys/values（按名排序）、source_cookie、重启后实例可见
	let values = settings_values(&app.app, id).await;
	assert_eq!(values["login_keep"], "logged_in");
	assert_eq!(values["login_keep.keys"], json!(["other", "session"]));
	assert_eq!(values["login_keep.values"], json!(["o1", "s2"]));
	assert_eq!(
		cookie_rows(&app.db_path, id).await,
		vec![
			("auth.example.org".into(), "other".into(), "o1".into()),
			("auth.example.org".into(), "session".into(), "s2".into()),
		],
		"外域 cookie 不得入库"
	);

	// 会话一次性：context 已 dispose，重复 complete 404
	{
		let state = browser.lock().expect("mock lock");
		assert_eq!(state.disposed, vec!["ctx-1"]);
		assert_eq!(state.screencast_stopped, vec!["ctx-1"]);
	}
	let (status, _) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login_keep", "session": started["session"]})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browser_login_complete_harvests_declared_local_storage() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;

	// login 设置声明了 local_storage_keys: ["token"]
	let started = start_session(&app.app, id, "login").await;
	let session = started["session"].as_str().expect("session id").to_string();
	assert_eq!(started["loginUrl"], "https://login.example.com/signin");
	{
		let mut state = browser.lock().expect("mock lock");
		state
			.cookies
			.insert("ctx-1".into(), vec![cookie("session", "s1", "login.example.com")]);
		state.local_storage.insert("token".into(), "t1".into());
		state.local_storage.insert("undeclared".into(), "u1".into());
	}

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let values = settings_values(&app.app, id).await;
	assert_eq!(values["login"], "logged_in");
	assert_eq!(values["login.ls.token"], "t1", "声明键被收割");
	assert!(values.get("login.ls.undeclared").is_none(), "未声明键不收: {values}");
}

#[tokio::test]
async fn browser_login_complete_without_host_cookies_fails() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;
	let started = start_session(&app.app, id, "login_keep").await;
	let session = started["session"].as_str().expect("session id").to_string();
	// 只有外域 cookie：用户没完成登录
	browser
		.lock()
		.expect("mock lock")
		.cookies
		.insert("ctx-1".into(), vec![cookie("foreign", "f1", "other.net")]);

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login_keep", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login_keep").is_none(), "不得有 marker: {values}");
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
	// 会话已终结且 context 已 dispose（一次性语义，失败不残留）
	assert_eq!(browser.lock().expect("mock lock").disposed, vec!["ctx-1"]);
}

#[tokio::test]
async fn browser_login_handler_veto_blocks_marker() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;
	let started = start_session(&app.app, id, "login").await;
	let session = started["session"].as_str().expect("session id").to_string();
	// fixture 只认 session cookie：缺它 handle_web_login 返回 false
	browser
		.lock()
		.expect("mock lock")
		.cookies
		.insert("ctx-1".into(), vec![cookie("other", "o1", "login.example.com")]);

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "否决后不得有 marker: {values}");
	// Aidoku 顺序是先存 keys/values 再调导出，否决不回滚
	assert_eq!(values["login.keys"], json!(["other"]));
	assert!(cookie_rows(&app.db_path, id).await.is_empty());
}

#[tokio::test]
async fn browser_login_cancel_disposes_context() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;
	let started = start_session(&app.app, id, "login_keep").await;
	let session = started["session"].as_str().expect("session id").to_string();

	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/cancel"),
		Some(json!({"key": "login_keep", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(browser.lock().expect("mock lock").disposed, vec!["ctx-1"]);

	// 取消后再 complete：会话已不存在
	let (status, _) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login_keep", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browser_login_start_rejects_bad_requests() {
	let (app, id, _browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;
	let path = format!("/api/v1/sources/{id}/login/web/browser");

	// 空 key
	let (status, body) = request(&app.app, "POST", &path, Some(json!({"key": ""}))).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	// 未安装源
	let (status, body) = request(&app.app, "POST", "/api/v1/sources/999/login/web/browser", Some(json!({"key": "login"}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

	// key 撞在非 login 设置上与未知键同口径
	for key in ["auth_url", "nope"] {
		let (status, body) = request(&app.app, "POST", &path, Some(json!({"key": key}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {body}");
	}
}

#[tokio::test]
async fn browser_login_complete_rejects_mismatched_key() {
	let (app, id, browser) = app_with_browser_and_login_source(&common::weblogin_wasm_path()).await;
	let started = start_session(&app.app, id, "login_keep").await;
	let session = started["session"].as_str().expect("session id").to_string();
	browser.lock().expect("mock lock").cookies.insert(
		"ctx-1".into(),
		vec![cookie("session", "s2", "auth.example.org")],
	);

	// 会话钉在 login_keep 上：拿别的 key 完成是 400
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/login/web/browser/complete"),
		Some(json!({"key": "login", "session": session})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let values = settings_values(&app.app, id).await;
	assert!(values.get("login").is_none(), "{values}");
	assert!(values.get("login_keep").is_none(), "{values}");
}
