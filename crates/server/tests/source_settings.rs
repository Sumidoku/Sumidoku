//! 源设置的合并、通知送达与默认值刷新端点测试。
//! fixture 源：notify（导出 get_dynamic_settings 与 handle_notification，收到
//! 的 notification 编进 echo 设置标题，经 GET settings 读回断言送达）。
//! fixture 重建：`cd tests/fixtures/notify-src && cargo build --release --target
//! wasm32-unknown-unknown`，产物复制为 ../notify/main.wasm。
mod common;

use axum::http::StatusCode;
use axum::Router;
use common::{TestApp, request, test_app};
use serde_json::{Value, json};

/// 登记 notify fixture 源：两种语言（验证合成语言设置仍排最前），可带静态 settings.json
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

/// 无动态设置的静态源（mutation 载荷），供默认值刷新测试改写版本
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

/// 摊平 group/page，返回全部叶子设置（合并断言按 key 找项用）
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
	// 合成语言设置排在最前（extraSettings 语义）
	assert_eq!(list[0]["key"], "__language", "{body}");

	let leaves = flatten(&list[1..]);
	let keys: Vec<&str> = leaves.iter().filter_map(|s| s["key"].as_str()).collect();
	// 静态项在动态项之前；撞键的动态 dup 被去重，保留静态声明
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

	// 触发实例加载；echo 初始标题 none
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "none", "{body}");

	// 不带 notify 的保存不触发通知
	let (status, _) = request(&app.app, "PUT", &path, Some(json!({"echo": "x"}))).await;
	assert_eq!(status, StatusCode::OK);
	let body = settings_body(&app.app, id).await;
	let leaves = flatten(body["settings"].as_array().expect("settings array"));
	let echo = leaves.iter().find(|s| s["key"] == "echo").expect("echo setting");
	assert_eq!(echo["title"], "none", "无 notify 不应触发: {body}");

	// 多个 notify 按序送达，最后一个成为实例里的最新状态
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

	// 无 notification 能力的源按端点约定 501
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

	// 用户改过 mode；quality 与 url 不动
	let (status, _) = request(
		&app.app,
		"PUT",
		&format!("/api/v1/sources/{id}/settings"),
		Some(json!({"mode": "b"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// 源升级：默认 quality 改 high、base url 换域名、mode 默认不变
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

/// 单语言、单地址的源不合成语言/线路设置（example fixture：languages ["en"]，无 urls）
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

/// languageSelectType == "single" 时多语言源出单选 language 设置，带 content 刷新语义
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

	// 清单的对象形式在 DTO 里归一成语言码，源列表不会解码失败
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
