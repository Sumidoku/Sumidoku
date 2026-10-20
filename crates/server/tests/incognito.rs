mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, register_example_manga, request, request_full, source_by_key, test_app};
use serde_json::{Value, json};

async fn chapter_ids(app: &Router, manga_id: i64) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters")
		.iter()
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect()
}

async fn history_len(app: &Router) -> usize {
	let (status, body) = request(app, "GET", "/api/v1/history", None).await;
	assert_eq!(status, StatusCode::OK);
	body["entries"].as_array().expect("history entries").len()
}

async fn patch_progress(app: &Router, chapter_id: i64, body: Value) -> Value {
	let (status, resp) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(body),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	resp
}

/// 注册搜索前两条结果，返回两个 manga id
async fn register_two_manga(app: &Router) -> (i64, i64) {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entries = body["entries"].as_array().expect("entries");
	let mut ids = Vec::new();
	for entry in entries.iter().take(2) {
		let (status, body) = request(
			app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": entry})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		ids.push(body["id"].as_i64().expect("manga id"));
	}
	(ids[0], ids[1])
}

#[tokio::test]
async fn global_incognito_pauses_history() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let chapters = chapter_ids(&app.app, manga_id).await;
	let (ch1, ch2) = (chapters[0], chapters[1]);

	// 基线：默认打戳，历史有条目
	let resp = patch_progress(&app.app, ch1, json!({"lastPageRead": 3})).await;
	assert!(resp["lastReadAt"].is_i64());
	assert_eq!(history_len(&app.app).await, 1);

	// 开全局隐身：进度照存，但不打戳、历史不增
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"incognito.enabled": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let resp = patch_progress(&app.app, ch2, json!({"lastPageRead": 5})).await;
	assert_eq!(resp["lastPageRead"], 5);
	assert!(resp["lastReadAt"].is_null(), "隐身中不应刷新 lastReadAt: {resp}");
	assert_eq!(history_len(&app.app).await, 1, "隐身中的阅读不应进历史");
	// 批量端点同口径
	let (status, body) = request(
		&app.app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": [ch2], "lastPageRead": 9, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 1);
	let (_, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	let ch2_state = body
		.as_array()
		.expect("chapters")
		.iter()
		.find(|c| c["id"] == ch2)
		.expect("ch2")
		.clone();
	assert_eq!(ch2_state["lastPageRead"], 9);
	assert_eq!(ch2_state["isRead"], true);
	assert!(ch2_state["lastReadAt"].is_null(), "批量写入隐身中也不应打戳");

	// 已有的时间戳既不刷新也不清掉（Aidoku：旧历史保持现状）
	let pool = connect_db(&app.db_path).await;
	sqlx::query("UPDATE chapter_user SET last_read_at = 100 WHERE chapter_id = ?")
		.bind(ch1)
		.execute(&pool)
		.await
		.expect("age last_read_at");
	let resp = patch_progress(&app.app, ch1, json!({"lastPageRead": 6})).await;
	assert_eq!(resp["lastPageRead"], 6);
	assert_eq!(resp["lastReadAt"], 100, "隐身中旧时间戳应保持不动");

	// 关掉恢复记录
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"incognito.enabled": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let resp = patch_progress(&app.app, ch2, json!({"lastPageRead": 10})).await;
	assert!(resp["lastReadAt"].is_i64(), "关闭隐身后应恢复打戳");
	assert_eq!(history_len(&app.app).await, 2);
}

#[tokio::test]
async fn per_manga_incognito_pauses_history() {
	let app = test_app().await;
	let (a, b) = register_two_manga(&app.app).await;
	let a_ch = chapter_ids(&app.app, a).await;
	let b_ch = chapter_ids(&app.app, b).await;

	// 单本开关：详情响应带出，未开的为 false
	let (status, body) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/manga/{a}"),
		Some(json!({"incognito": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["incognito"], true);
	let (_, body) = request(&app.app, "GET", &format!("/api/v1/manga/{a}"), None).await;
	assert_eq!(body["incognito"], true);
	let (_, body) = request(&app.app, "GET", &format!("/api/v1/manga/{b}"), None).await;
	assert_eq!(body["incognito"], false);

	// 只影响该漫画：A 不打戳不进历史，B 照常
	let resp = patch_progress(&app.app, a_ch[0], json!({"lastPageRead": 3})).await;
	assert!(resp["lastReadAt"].is_null(), "单本隐身不应打戳: {resp}");
	assert_eq!(history_len(&app.app).await, 0);
	let resp = patch_progress(&app.app, b_ch[0], json!({"lastPageRead": 3})).await;
	assert!(resp["lastReadAt"].is_i64());
	assert_eq!(history_len(&app.app).await, 1);

	// 全局与单本是 OR：两个都开仍然不打戳
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"incognito.enabled": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let resp = patch_progress(&app.app, a_ch[1], json!({"lastPageRead": 1})).await;
	assert!(resp["lastReadAt"].is_null());
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"incognito.enabled": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// 关单本恢复记录
	let (status, body) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/manga/{a}"),
		Some(json!({"incognito": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["incognito"], false);
	let resp = patch_progress(&app.app, a_ch[0], json!({"lastPageRead": 4})).await;
	assert!(resp["lastReadAt"].is_i64(), "关闭单本隐身后应恢复打戳");
}

#[tokio::test]
async fn backup_round_trips_incognito() {
	let a = test_app().await;
	let manga_id = register_example_manga(&a.app).await;
	let (status, _) = request(
		&a.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&a.app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"incognito": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, _, bytes) = request_full(&a.app, "GET", "/api/v1/backup", None).await;
	assert_eq!(status, StatusCode::OK);
	let doc: Value = serde_json::from_slice(&bytes).expect("backup json");
	assert_eq!(doc["library"][0]["incognito"], true, "导出应带单本隐身标志");

	// 恢复到全新实例：标志跟书库条目一起走
	let b = test_app().await;
	let (status, _) = request(&b.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc.clone())).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	let (_, library) = request(&b.app, "GET", "/api/v1/library", None).await;
	let b_manga_id = library["entries"][0]["id"].as_i64().expect("b manga id");
	let (_, body) = request(&b.app, "GET", &format!("/api/v1/manga/{b_manga_id}"), None).await;
	assert_eq!(body["incognito"], true, "恢复后单本隐身应保持");

	// 旧备份没有该字段：按 false 恢复，不报错
	let mut old_doc = doc;
	old_doc["library"][0]
		.as_object_mut()
		.expect("library entry")
		.remove("incognito");
	let c = test_app().await;
	let (status, _) = request(&c.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&c.app, "POST", "/api/v1/backup", Some(old_doc)).await;
	assert_eq!(status, StatusCode::OK, "legacy import failed: {body}");
	let (_, library) = request(&c.app, "GET", "/api/v1/library", None).await;
	let c_manga_id = library["entries"][0]["id"].as_i64().expect("c manga id");
	let (_, body) = request(&c.app, "GET", &format!("/api/v1/manga/{c_manga_id}"), None).await;
	assert_eq!(body["incognito"], false, "旧备份缺字段按未开启恢复");
}
