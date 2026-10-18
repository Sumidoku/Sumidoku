mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, fixture_dir, install_fixture_source, request, test_app};
use serde_json::{Value, json};
use sumidoku_server::build_app;

/// 装三个 fixture 源（example + 两个同源 wasm 的复制源），返回按 id 排序的列表
async fn install_three(app: &Router, tmp: &tempfile::TempDir) -> Vec<Value> {
	install_fixture_source(tmp, "en.second", "Second Source", &fixture_dir().join("main.wasm"));
	install_fixture_source(tmp, "en.third", "Third Source", &fixture_dir().join("main.wasm"));
	let (status, body) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["installed"].as_array().expect("installed").len(), 3);
	list_sources(app).await
}

async fn list_sources(app: &Router) -> Vec<Value> {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body.as_array().expect("sources array").clone()
}

fn by_key(sources: &[Value], key: &str) -> Value {
	sources
		.iter()
		.find(|s| s["key"] == key)
		.cloned()
		.unwrap_or_else(|| panic!("source {key} installed"))
}

fn id_of(source: &Value) -> i64 {
	source["id"].as_i64().expect("source id")
}

async fn patch_pin(app: &Router, id: i64, pinned: bool) -> (StatusCode, Value) {
	request(app, "PATCH", &format!("/api/v1/sources/{id}"), Some(json!({"pinned": pinned}))).await
}

async fn reorder(app: &Router, ids: &[&str]) -> (StatusCode, Value) {
	request(app, "POST", "/api/v1/sources/reorder-pinned", Some(json!({"ids": ids}))).await
}

#[tokio::test]
async fn pin_unpin_cycle() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	for s in &sources {
		assert_eq!(s["pinned"], false, "{s}");
		assert!(s["pinOrder"].is_null(), "{s}");
	}
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let third = by_key(&sources, "en.third");

	// pin 追加到列表尾：second → 0，first → 1，third → 2
	let (status, body) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");
	let (status, body) = patch_pin(&app, id_of(&first), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 1, "{body}");
	let (status, body) = patch_pin(&app, id_of(&third), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 2, "{body}");

	// 重复 pin 幂等：不换序、不重复入列
	let (status, body) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinOrder"], 0, "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(sources.iter().filter(|s| s["pinned"] == true).count(), 3);

	// unpin 中间的 second：first 前移，third 保持最后
	let (status, body) = patch_pin(&app, id_of(&second), false).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], false, "{body}");
	assert!(body["pinOrder"].is_null(), "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinned"], false);
	// 重复 unpin 幂等
	let (status, body) = patch_pin(&app, id_of(&second), false).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], false, "{body}");

	// 空 body / 缺 pinned 字段为无操作
	let (status, body) = request(&app, "PATCH", &format!("/api/v1/sources/{}", id_of(&first)), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/sources/{}", id_of(&first)),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["pinned"], true, "{body}");

	// 非法值 400，不落库
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/sources/{}", id_of(&first)),
		Some(json!({"pinned": "yes"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert_eq!(body["error"]["code"], "bad_request");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.example-source")["pinned"], true);

	// 源不存在 404
	let (status, _) = patch_pin(&app, 999, true).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reorder_pinned_validates_set() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let third = by_key(&sources, "en.third");
	for s in [&first, &second, &third] {
		let (status, _) = patch_pin(&app, id_of(s), true).await;
		assert_eq!(status, StatusCode::OK);
	}

	// 合法换序：[first, second, third] → [third, first, second]
	let (status, body) = reorder(&app, &["en.third", "en.example-source", "en.second"]).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 2);

	// 集合必须一致：少 id / 多 id / 重复 id / 未 pin 的 key / 不存在的 key / 非法形状全部 400，且顺序不变
	for bad in [
		json!({"ids": ["en.third", "en.example-source"]}),
		json!({"ids": ["en.third", "en.example-source", "en.second", "en.extra"]}),
		json!({"ids": ["en.third", "en.third", "en.example-source"]}),
		json!({"ids": ["en.third", "en.example-source", "en.ghost"]}),
		json!({"ids": "en.third"}),
		json!({}),
	] {
		let (status, body) = request(&app, "POST", "/api/v1/sources/reorder-pinned", Some(bad.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
	}
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.third")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 2);
}

#[tokio::test]
async fn delete_source_prunes_pinned() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	for s in [&first, &second] {
		let (status, _) = patch_pin(&app, id_of(s), true).await;
		assert_eq!(status, StatusCode::OK);
	}

	// 卸载列表头的 first：key 从列表剔除，second 前移为 0
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/sources/{}", id_of(&first)), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 0);

	// 卸载后 reorder 只认剩余集合：stale key 或「已装但未 pin」的 key 都 400
	let (status, body) = reorder(&app, &["en.example-source", "en.second"]).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let (status, body) = reorder(&app, &["en.third"]).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn pins_survive_restart() {
	let TestApp { app, tmp, .. } = test_app().await;
	let sources = install_three(&app, &tmp).await;
	let first = by_key(&sources, "en.example-source");
	let second = by_key(&sources, "en.second");
	let (status, _) = patch_pin(&app, id_of(&second), true).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = patch_pin(&app, id_of(&first), true).await;
	assert_eq!(status, StatusCode::OK);
	drop(app);

	// 重启：新 AppState、新连接池，同一份数据文件
	let app = build_app(base_config(&tmp)).await.expect("rebuild app");
	let sources = list_sources(&app).await;
	assert_eq!(by_key(&sources, "en.second")["pinOrder"], 0);
	assert_eq!(by_key(&sources, "en.example-source")["pinOrder"], 1);
	assert_eq!(by_key(&sources, "en.third")["pinned"], false);
}
