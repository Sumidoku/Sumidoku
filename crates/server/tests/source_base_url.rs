//! 动态 base URL 解析（issue #136）的端点测试：安装/更新时调源的 get_base_url，
//! 解析值落 source.base_url 并前置进合成 base-url 设置的候选与种子默认。
//! fixture 源：baseurl（get_base_url 返回 defaults.dynamic_url，缺省
//! https://dyn-v1.example，值为 "fail" 时 bail 模拟导出失败）。
//! fixture 重建：`cd tests/fixtures/baseurl-src && cargo build --release --target
//! wasm32-unknown-unknown`，产物复制为 ../baseurl/main.wasm。
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{request, test_app};
use serde_json::{Value, json};

const KEY: &str = "en.dynbase";
const STATIC: &str = "https://static.example";
const MIRROR: &str = "https://m1.example";
const DYN_V1: &str = "https://dyn-v1.example";
const DYN_V2: &str = "https://dyn-v2.example";

/// 登记 baseurl fixture 源：静态 URL + 一个镜像，allowsBaseUrlSelect 可选
fn write_baseurl_source(dir: &std::path::Path, version: i64, allows_select: bool) {
	std::fs::create_dir_all(dir).expect("mkdir");
	std::fs::copy(common::baseurl_wasm_path(), dir.join("main.wasm")).expect("main.wasm");
	std::fs::write(
		dir.join("source.json"),
		json!({
			"info": {
				"id": KEY, "name": "DynBase", "version": version,
				"url": STATIC, "urls": [MIRROR], "languages": ["en"]
			},
			"config": {"allowsBaseUrlSelect": allows_select}
		})
		.to_string(),
	)
	.expect("source.json");
}

async fn scan(app: &Router) -> Value {
	let (status, body) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

async fn source_json(app: &Router) -> Value {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == KEY)
		.cloned()
		.expect("dynbase source")
}

async fn settings_body(app: &Router, source_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

/// 摊平 group/page 后按 key 找叶子设置
fn find_leaf<'a>(items: &'a [Value], key: &str) -> Option<&'a Value> {
	for item in items {
		let container = item["value"]["Group"]["items"]
			.as_array()
			.or_else(|| item["value"]["Page"]["items"].as_array());
		if let Some(children) = container {
			if let Some(found) = find_leaf(children, key) {
				return Some(found);
			}
		} else if item["key"] == key {
			return Some(item);
		}
	}
	None
}

async fn put_settings(app: &Router, source_id: i64, values: Value) {
	let (status, body) = request(app, "PUT", &format!("/api/v1/sources/{source_id}/settings"), Some(values)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn install_resolves_dynamic_base_url() {
	let app = test_app().await;
	write_baseurl_source(&app.tmp.path().join("sources").join(KEY), 1, true);
	scan(&app.app).await;
	let source = source_json(&app.app).await;
	assert_eq!(source["baseUrl"], DYN_V1, "安装后 base_url 应是动态解析值");

	let id = source["id"].as_i64().expect("id");
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	let url = find_leaf(items, "url").expect("base url select");
	assert_eq!(
		url["value"]["Select"]["values"],
		json!([DYN_V1, STATIC, MIRROR]),
		"动态值应前置进候选: {body}"
	);
	assert_eq!(url["value"]["Select"]["default"], DYN_V1, "{body}");
	assert_eq!(body["values"]["url"], DYN_V1, "种子默认应取动态值: {body}");
}

#[tokio::test]
async fn update_re_resolves_and_untouched_default_follows() {
	let app = test_app().await;
	let dir = app.tmp.path().join("sources").join(KEY);
	write_baseurl_source(&dir, 1, true);
	scan(&app.app).await;
	let id = source_json(&app.app).await["id"].as_i64().expect("id");

	// 源更新后解析值变化（经 defaults 注入模拟），未动过的 url 默认应跟随
	put_settings(&app.app, id, json!({"dynamic_url": DYN_V2})).await;
	write_baseurl_source(&dir, 2, true);
	let outcome = scan(&app.app).await;
	assert!(
		outcome["updated"].as_array().expect("updated").iter().any(|k| k == KEY),
		"重扫应判为更新: {outcome}"
	);

	assert_eq!(source_json(&app.app).await["baseUrl"], DYN_V2);
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	let url = find_leaf(items, "url").expect("base url select");
	assert_eq!(url["value"]["Select"]["values"], json!([DYN_V2, STATIC, MIRROR]), "{body}");
	assert_eq!(url["value"]["Select"]["default"], DYN_V2, "{body}");
	assert_eq!(body["values"]["url"], DYN_V2, "未动过的默认应跟随新解析值: {body}");
}

#[tokio::test]
async fn update_preserves_user_picked_url() {
	let app = test_app().await;
	let dir = app.tmp.path().join("sources").join(KEY);
	write_baseurl_source(&dir, 1, true);
	scan(&app.app).await;
	let id = source_json(&app.app).await["id"].as_i64().expect("id");

	// 用户手动选了镜像线路，更新后保留；候选与默认仍跟随新解析值
	put_settings(&app.app, id, json!({"url": MIRROR})).await;
	put_settings(&app.app, id, json!({"dynamic_url": DYN_V2})).await;
	write_baseurl_source(&dir, 2, true);
	scan(&app.app).await;

	assert_eq!(source_json(&app.app).await["baseUrl"], DYN_V2);
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	let url = find_leaf(items, "url").expect("base url select");
	assert_eq!(url["value"]["Select"]["values"], json!([DYN_V2, STATIC, MIRROR]), "{body}");
	assert_eq!(url["value"]["Select"]["default"], DYN_V2, "{body}");
	assert_eq!(body["values"]["url"], MIRROR, "用户选过的值应保留: {body}");
}

#[tokio::test]
async fn export_failure_falls_back_to_static() {
	let app = test_app().await;
	let dir = app.tmp.path().join("sources").join(KEY);
	write_baseurl_source(&dir, 1, true);
	scan(&app.app).await;
	let id = source_json(&app.app).await["id"].as_i64().expect("id");
	assert_eq!(source_json(&app.app).await["baseUrl"], DYN_V1);

	// 导出失败不杀死更新：base_url 回退包内静态值，未动过的默认一并跟随
	put_settings(&app.app, id, json!({"dynamic_url": "fail"})).await;
	write_baseurl_source(&dir, 2, true);
	let outcome = scan(&app.app).await;
	assert!(
		outcome["updated"].as_array().expect("updated").iter().any(|k| k == KEY),
		"解析失败不应跳过注册: {outcome}"
	);

	assert_eq!(source_json(&app.app).await["baseUrl"], STATIC);
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	let url = find_leaf(items, "url").expect("base url select");
	assert_eq!(url["value"]["Select"]["values"], json!([STATIC, MIRROR]), "{body}");
	assert_eq!(url["value"]["Select"]["default"], STATIC, "{body}");
	assert_eq!(body["values"]["url"], STATIC, "未动过的默认应回退静态值: {body}");
}

#[tokio::test]
async fn resolution_applies_without_base_url_select() {
	let app = test_app().await;
	write_baseurl_source(&app.tmp.path().join("sources").join(KEY), 1, false);
	scan(&app.app).await;

	// 不开 allowsBaseUrlSelect 也解析（登录 host 兜底等消费方读 source.base_url），
	// 但不合成 select、不播种 url
	let source = source_json(&app.app).await;
	assert_eq!(source["baseUrl"], DYN_V1);
	let id = source["id"].as_i64().expect("id");
	let body = settings_body(&app.app, id).await;
	let items = body["settings"].as_array().expect("settings array");
	assert!(find_leaf(items, "url").is_none(), "不应合成 base url 设置: {body}");
	assert!(body["values"]["url"].is_null(), "不应播种 url: {body}");
}
