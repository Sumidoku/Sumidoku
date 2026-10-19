//! listing 透传与 source config DTO（issue #138）：
//! GET /sources/{id}/listings/{listing_id} 把真实 listing（id, name, kind）传给
//! get_manga_list；已装源 DTO 暴露 manifest config 节。
//! fixture 源在 crates/wasm/tests/fixtures/listing-src（no_std cdylib），重建方式：
//! `cd crates/wasm/tests/fixtures/listing-src && cargo build --release --target wasm32-unknown-unknown`，
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_listing.wasm
//! 复制为 crates/wasm/tests/fixtures/listing/main.wasm。
mod common;

use axum::http::StatusCode;
use common::{
	TestApp, base_config, baseurl_wasm_path, install_fixture_source, install_fixture_source_with_doc,
	listing_wasm_path, request,
};
use serde_json::json;
use sumidoku_server::build_app;

/// listing 回显源（带 listings 与 config 节）+ baseurl 对照源（无 config 节）双源 app
async fn listing_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let doc = json!({
		"info": {"id": "en.listing", "name": "Listing Source", "version": 1, "languages": ["en"]},
		"listings": [
			{"id": "hot", "name": "Hot Updates", "kind": 1},
			{"id": "fresh"},
			"bare"
		],
		"config": {
			"hidesFiltersWhileSearching": true,
			"supportsTagSearch": true,
			"supportsArtistSearch": true,
			"supportsAuthorSearch": true,
			"maximumParallelRequests": 2
		}
	});
	install_fixture_source_with_doc(&tmp, &doc, &listing_wasm_path());
	install_fixture_source(&tmp, "en.dynbase", "DynBase Source", &baseurl_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn install_and_ids(app: &TestApp) -> (i64, i64) {
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	let find = |key: &str| {
		sources
			.iter()
			.find(|s| s["key"] == key)
			.and_then(|s| s["id"].as_i64())
			.expect("fixture source")
	};
	(find("en.listing"), find("en.dynbase"))
}

/// fixture 把收到的 listing 回显成标题 "{id}|{name}|{kind}"
async fn echo_title(app: &TestApp, source_id: i64, listing_id: &str) -> String {
	let (status, body) = request(
		&app.app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/{listing_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["entries"][0]["title"].as_str().expect("echo title").to_string()
}

#[tokio::test]
async fn manga_list_receives_the_real_listing() {
	let app = listing_app().await;
	let (source_id, _) = install_and_ids(&app).await;

	// 静态带 kind=1：AidokuRunner 按 UInt8 解出 List
	assert_eq!(echo_title(&app, source_id, "hot").await, "hot|Hot Updates|list");
	// 静态缺 name：Runner 语义回填 id
	assert_eq!(echo_title(&app, source_id, "fresh").await, "fresh|fresh|default");
	// 裸字符串条目：id=name=字符串
	assert_eq!(echo_title(&app, source_id, "bare").await, "bare|bare|default");
	// 动态 get_listings 提供的 listing
	assert_eq!(echo_title(&app, source_id, "dyn").await, "dyn|Dynamic Picks|list");
	// 未知 id：透传不报错（Aidoku 没有按 id 查找的路径，源自行决定响应）
	assert_eq!(echo_title(&app, source_id, "ghost").await, "ghost||default");
}

#[tokio::test]
async fn installed_source_dto_exposes_the_manifest_config() {
	let app = listing_app().await;
	let (source_id, plain_id) = install_and_ids(&app).await;

	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	let config = &sources
		.iter()
		.find(|s| s["id"] == source_id)
		.expect("listing source")["config"];
	assert_eq!(config["hidesFiltersWhileSearching"], true, "{config}");
	assert_eq!(config["supportsTagSearch"], true, "{config}");
	assert_eq!(config["supportsArtistSearch"], true, "{config}");
	assert_eq!(config["supportsAuthorSearch"], true, "{config}");
	assert_eq!(config["maximumParallelRequests"], 2, "{config}");

	let plain = &sources
		.iter()
		.find(|s| s["id"] == plain_id)
		.expect("plain source")["config"];
	assert!(plain.is_null(), "{plain}");

	// scan 之外的用户面端点同形：update DTO 也带 config（source_json 统一出口）
	let (status, body) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/sources/{source_id}"),
		Some(json!({"pinned": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["config"]["maximumParallelRequests"], 2, "{body}");
}
