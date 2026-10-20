mod common;

use axum::http::StatusCode;
use common::{TestApp, register_example_manga, request, request_full, source_by_key, test_app};
use serde_json::json;

// 1x1 透明 PNG
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[tokio::test]
async fn full_flow() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;

	// health
	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");

	// version
	let (status, body) = request(&app, "GET", "/api/v1/version", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

	// 未安装时 404
	let (status, body) = request(&app, "GET", "/api/v1/manga/999", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	// scan 安装
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!(["en.example-source"]));
	assert_eq!(body["updated"], json!([]));
	assert_eq!(body["skipped"], json!([]));

	// 再 scan 幂等：老源走 updated
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!([]));
	assert_eq!(body["updated"], json!(["en.example-source"]));

	// sources 列表：内置本地源随迁移落库，钉死大 id 恒排在用户源之后
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let sources = body.as_array().expect("sources array");
	assert_eq!(sources.len(), 2);
	assert_eq!(sources[0]["key"], "local.local");
	assert_eq!(sources[0]["kind"], "local");
	assert_eq!(sources[1]["key"], "en.example-source");
	assert_eq!(sources[1]["name"], "Example Source");
	assert_eq!(sources[1]["enabled"], true);
	assert_eq!(sources[1]["kind"], "aidoku");
	let source_id = sources[1]["id"].as_i64().expect("source id");

	// 搜索 20 条
	let search_path = format!("/api/v1/sources/{source_id}/search");
	let (status, body) = request(&app, "POST", &search_path, Some(json!({"page": 1}))).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 20);
	assert_eq!(body["hasNextPage"], true);
	assert_eq!(entries[0]["sourceId"], source_id);
	let manga_key = entries[0]["key"].as_str().expect("manga key").to_string();

	// query 过滤
	let (status, body) = request(
		&app,
		"POST",
		&search_path,
		Some(json!({"query": "Manga 3", "page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let filtered = body["entries"].as_array().expect("filtered entries");
	assert!(!filtered.is_empty());
	assert!(
		filtered
			.iter()
			.all(|m| m["title"].as_str().unwrap_or_default().contains("Manga 3")),
		"all titles should contain 'Manga 3': {filtered:?}"
	);

	// 入库
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");

	// 详情
	let manga_path = format!("/api/v1/manga/{manga_id}");
	let (status, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["initialized"], true);
	assert_eq!(body["chapterCount"], 8);

	// 章节：8 章按 source_order
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters array");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0]["chapter_number"], json!(8.0));
	assert!(chapters.iter().all(|c| c["id"].is_i64() && c["pageCount"].is_i64()));
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");

	// 页：2 页，Url + Text
	let pages_path = format!("/api/v1/chapter/{chapter_id}/pages");
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	assert_eq!(pages[0]["index"], 0);
	assert_eq!(
		pages[0]["content"]["Url"][0],
		"https://aidoku.app/images/icon.png"
	);
	assert_eq!(pages[1]["index"], 1);
	assert!(
		pages[1]["content"]["Text"]
			.as_str()
			.expect("text page")
			.starts_with("# Title")
	);

	// 第二次走 page 缓存：字段与源路径一致
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let cached = body.as_array().expect("cached pages");
	assert_eq!(cached.len(), 2);
	assert_eq!(cached[0]["hasDescription"], true, "缓存路径应与源路径字段一致");

	// page_count 回写
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["pageCount"], 2);

	// 裸 ?refresh 标志强制重取
	let (status, body) = request(&app, "GET", &format!("{pages_path}?refresh"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("refreshed pages").len(), 2);

	// 设置热重启：setting=true → settings 多出一个 Toggle
	let settings_path = format!("/api/v1/sources/{source_id}/settings");
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 2);
	assert_eq!(body["values"]["setting"], true);

	// setting=false → 只剩一个
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": false}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 1);
	assert_eq!(body["values"]["setting"], false);
}

#[tokio::test]
async fn reset_and_clear_source_state() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, sources) = request(&app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&sources, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let settings_path = format!("/api/v1/sources/{source_id}/settings");

	// 覆盖一个设置项
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": true}))).await;
	assert_eq!(status, StatusCode::OK);

	// 重置：覆盖值清空；下一次 get_settings 会把源声明的默认值重新补种
	let reset_path = format!("/api/v1/sources/{source_id}/settings/reset");
	let (status, body) = request(&app, "POST", &reset_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body["values"].is_object());
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 1);
	assert!(
		body["values"]["setting"].is_boolean(),
		"重置后应回到源声明的默认值: {body}"
	);

	// 清缓存：幂等成功
	let clear_path = format!("/api/v1/sources/{source_id}/cache/clear");
	let (status, body) = request(&app, "POST", &clear_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["ok"], true);

	// 源不支持基本登录 → 501 unsupported
	let login_path = format!("/api/v1/sources/{source_id}/login/basic");
	let (status, body) = request(
		&app,
		"POST",
		&login_path,
		Some(json!({"key": "login", "username": "u", "password": "p"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], "unsupported");
}

#[tokio::test]
async fn source_icon() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let icon_path = format!("/api/v1/sources/{source_id}/icon");

	// 源无 icon → 404
	let (status, body) = request(&app, "GET", &icon_path, None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
	// 源不存在 → 404
	let (status, _) = request(&app, "GET", "/api/v1/sources/999/icon", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// 放入 icon.png 重新 scan → 200
	std::fs::write(tmp.path().join("sources/en.example-source/icon.png"), PNG).expect("write icon");
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, headers, bytes) = request_full(&app, "GET", &icon_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=86400");
	assert_eq!(bytes, PNG);

	// icon_path 还在库里但文件没了 → 404
	std::fs::remove_file(tmp.path().join("sources/en.example-source/icon.png")).expect("remove icon");
	let (status, _) = request(&app, "GET", &icon_path, None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_manga_update_strategy() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let manga_path = format!("/api/v1/manga/{manga_id}");
	let (_, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(body["update_strategy"], "Always");

	let (status, body) = request(&app, "PATCH", &manga_path, Some(json!({"updateStrategy": "Never"}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["update_strategy"], "Never");
	assert!(body["chapterCount"].is_i64());
	let (_, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(body["update_strategy"], "Never");

	// {} 与空 body 都是无操作
	let (status, body) = request(&app, "PATCH", &manga_path, Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["update_strategy"], "Never");
	let (status, body) = request(&app, "PATCH", &manga_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["update_strategy"], "Never");

	// 非法值 400，且不落库
	for bad in [json!({"updateStrategy": "Sometimes"}), json!({"updateStrategy": 5})] {
		let (status, body) = request(&app, "PATCH", &manga_path, Some(bad.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
		assert_eq!(body["error"]["code"], "bad_request");
	}
	let (_, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(body["update_strategy"], "Never");

	let (status, body) = request(&app, "PATCH", &manga_path, Some(json!({"updateStrategy": "Always"}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["update_strategy"], "Always");

	let (status, _) = request(&app, "PATCH", "/api/v1/manga/999", Some(json!({"updateStrategy": "Never"}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn manga_api_exposes_stored_viewer_thumbnail_and_override() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let manga_path = format!("/api/v1/manga/{manga_id}");
	let chapters_path = format!("{manga_path}/chapters");
	let pool = common::connect_db(&app.db_path).await;

	// 源没给 viewer/thumbnail 时响应保持 Unknown/null
	let (_, body) = request(&app.app, "GET", &manga_path, None).await;
	assert_eq!(body["viewer"], "Unknown");
	let (_, body) = request(&app.app, "GET", &chapters_path, None).await;
	assert!(body[0]["thumbnail"].is_null());

	// 落库的 viewer/thumbnail 要出现在响应里（2 = RightToLeft）
	sqlx::query("UPDATE manga SET viewer = 2 WHERE id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("seed manga viewer");
	sqlx::query("UPDATE chapter SET thumbnail = 'https://img.example/c1.png' WHERE manga_id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("seed chapter thumbnail");
	let (_, body) = request(&app.app, "GET", &manga_path, None).await;
	assert_eq!(body["viewer"], "RightToLeft");
	let (_, body) = request(&app.app, "GET", &chapters_path, None).await;
	for chapter in body.as_array().expect("chapters") {
		assert_eq!(chapter["thumbnail"], "https://img.example/c1.png");
	}

	// manga_user.viewer_flags 非零即用户覆盖：响应 viewer 用覆盖值（4 = Webtoon），源值保留在库里
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	sqlx::query("UPDATE manga_user SET viewer_flags = 4 WHERE manga_id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("seed viewer override");
	let (_, body) = request(&app.app, "GET", &manga_path, None).await;
	assert_eq!(body["viewer"], "Webtoon");
	// 覆盖只作用于响应，库里的源 viewer 不动
	let (stored,): (i64,) = sqlx::query_as("SELECT viewer FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("stored viewer");
	assert_eq!(stored, 2);
}
