mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, connect_db, fixture_dir, install_fixture_source, request, test_app};
use serde_json::{Value, json};

/// 装第二个 fixture 源并扫描，返回 (example_id, second_id)
async fn two_sources(app: &Router, tmp: &tempfile::TempDir) -> (i64, i64) {
	install_fixture_source(tmp, "en.second", "Second Source", &fixture_dir().join("main.wasm"));
	let (status, body) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let mut example = None;
	let mut second = None;
	for s in body.as_array().expect("sources array") {
		match s["key"].as_str().expect("source key") {
			"en.example-source" => example = s["id"].as_i64(),
			"en.second" => second = s["id"].as_i64(),
			_ => {}
		}
	}
	(example.expect("example source"), second.expect("second source"))
}

/// 在 example 源上注册「Manga 1」并加入书库，返回 (manga_id, manga_key)
async fn library_manga(app: &Router, example_id: i64) -> (i64, String) {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{example_id}/search"),
		Some(json!({"query": "Manga 1", "page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entry = body["entries"][0].clone();
	let key = entry["key"].as_str().expect("manga key").to_string();
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": example_id, "manga": entry})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let id = body["id"].as_i64().expect("manga id");
	let (status, body) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	(id, key)
}

/// 指定章节号的章节 JSON；不存在返回 Null
async fn chapter_by_number(app: &Router, manga_id: i64, number: f64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
		.as_array()
		.expect("chapters array")
		.iter()
		.find(|c| c["chapter_number"].as_f64() == Some(number))
		.cloned()
		.unwrap_or(Value::Null)
}

async fn set_progress(app: &Router, chapter_id: i64, last_page: i64, is_read: bool) {
	let (status, body) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": last_page, "isRead": is_read})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

async fn make_category(app: &Router, name: &str) -> i64 {
	let (status, body) = request(app, "POST", "/api/v1/categories", Some(json!({"name": name}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["id"].as_i64().expect("category id")
}

async fn manga_detail(app: &Router, id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

async fn migrate(app: &Router, mode: &str, entries: Value) -> (StatusCode, Value) {
	request(
		app,
		"POST",
		"/api/v1/migrate",
		Some(json!({"mode": mode, "entries": entries})),
	)
	.await
}

/// 标准源条目：example 在库「Manga 1」+ 分类 + ch8/ch7 进度，返回 (origin_id, key, cat_id)
async fn stocked_origin(app: &Router, example_id: i64) -> (i64, String, i64) {
	let (origin, key) = library_manga(app, example_id).await;
	let cat = make_category(app, "迁移测试").await;
	let (status, body) = request(
		app,
		"PUT",
		&format!("/api/v1/manga/{origin}/categories"),
		Some(json!({"categoryIds": [cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let ch8 = chapter_by_number(app, origin, 8.0).await;
	set_progress(app, ch8["id"].as_i64().unwrap(), 7, true).await;
	let ch7 = chapter_by_number(app, origin, 7.0).await;
	set_progress(app, ch7["id"].as_i64().unwrap(), 3, false).await;
	(origin, key, cat)
}

#[tokio::test]
async fn migrate_sources_report_counts() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (_origin, _key) = library_manga(&app, example).await;

	let (status, body) = request(&app, "GET", "/api/v1/migrate/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let rows = body.as_array().expect("sources array");
	let example_row = rows.iter().find(|s| s["id"] == example).expect("example row");
	let second_row = rows.iter().find(|s| s["id"] == second).expect("second row");
	assert_eq!(example_row["mangaCount"], 1, "{example_row}");
	assert_eq!(second_row["mangaCount"], 0, "{second_row}");
	assert_eq!(second_row["pinned"], false, "{second_row}");
	assert!(second_row["pinOrder"].is_null(), "{second_row}");

	// 置顶顺序随列表下发：迁移目标组合的默认优先级
	let (status, _) = request(&app, "PATCH", &format!("/api/v1/sources/{second}"), Some(json!({"pinned": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", "/api/v1/migrate/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let rows = body.as_array().expect("sources array");
	let second_row = rows.iter().find(|s| s["id"] == second).expect("second row");
	assert_eq!(second_row["pinned"], true, "{second_row}");
	assert_eq!(second_row["pinOrder"], 0, "{second_row}");
}

#[tokio::test]
async fn lookup_searches_target_source() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, _key) = library_manga(&app, example).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/migrate/lookup?mangaId={origin}&targetSourceId={second}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["query"], "Manga 1", "{body}");
	let candidates = body["candidates"].as_array().expect("candidates");
	assert!(!candidates.is_empty(), "{body}");
	for c in candidates {
		assert!(c["title"].as_str().unwrap().contains("Manga 1"), "{c}");
		assert_eq!(c["sourceId"], second, "{c}");
	}

	// 手动重搜：query 覆盖原标题；匹配不到就是批量流程里的 not found
	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/migrate/lookup?mangaId={origin}&targetSourceId={second}&query=ZZZ"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["candidates"].as_array().expect("candidates").len(), 0, "{body}");

	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/migrate/lookup?mangaId=99999&targetSourceId={second}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/migrate/lookup?mangaId={origin}&targetSourceId=99999"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn copy_keeps_origin_and_carries_state() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, cat) = stocked_origin(&app, example).await;
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let origin_added_at = body["entries"][0]["inLibraryAt"].as_i64().expect("inLibraryAt");

	let (status, body) = migrate(&app, "copy", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["results"][0];
	assert_eq!(result["fromMangaId"], origin, "{result}");
	assert_eq!(result["ok"], true, "{result}");
	assert_eq!(result["warnings"].as_array().expect("warnings").len(), 0, "{result}");
	let target = result["toMangaId"].as_i64().expect("toMangaId");

	// COPY 保留源条目：入库态、分类、进度原样
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], true, "{detail}");
	assert_eq!(detail["categoryIds"], json!([cat]), "{detail}");
	let ch8 = chapter_by_number(&app, origin, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");

	// 目标条目：入库（保留源入库时间）、分类搬走、进度按章节号对齐
	let detail = manga_detail(&app, target).await;
	assert_eq!(detail["inLibrary"], true, "{detail}");
	assert_eq!(detail["sourceId"], second, "{detail}");
	assert_eq!(detail["categoryIds"], json!([cat]), "{detail}");
	let ch8 = chapter_by_number(&app, target, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");
	assert!(ch8["lastReadAt"].is_number(), "{ch8}");
	let ch7 = chapter_by_number(&app, target, 7.0).await;
	assert_eq!(ch7["lastPageRead"], 3, "{ch7}");
	assert_eq!(ch7["isRead"], false, "{ch7}");
	let ch6 = chapter_by_number(&app, target, 6.0).await;
	assert_eq!(ch6["lastPageRead"], 0, "{ch6}");
	assert_eq!(ch6["isRead"], false, "{ch6}");

	// 书库按源过滤：目标源下只有目标条目，inLibraryAt 与源条目一致
	let (status, body) = request(&app, "GET", &format!("/api/v1/library?sourceId={second}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1, "{body}");
	assert_eq!(entries[0]["id"], target, "{body}");
	assert_eq!(entries[0]["inLibraryAt"].as_i64(), Some(origin_added_at), "{body}");
}

#[tokio::test]
async fn migrate_moves_and_wipes_origin() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, cat) = stocked_origin(&app, example).await;

	let (status, body) = migrate(&app, "migrate", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["results"][0];
	assert_eq!(result["ok"], true, "{result}");
	let target = result["toMangaId"].as_i64().expect("toMangaId");

	// MIGRATE 是替换：源条目撤出书库、分类清空、进度行清掉
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], false, "{detail}");
	assert_eq!(detail["categoryIds"], json!([]), "{detail}");
	let ch8 = chapter_by_number(&app, origin, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 0, "{ch8}");
	assert_eq!(ch8["isRead"], false, "{ch8}");
	assert!(ch8["lastReadAt"].is_null(), "{ch8}");

	let detail = manga_detail(&app, target).await;
	assert_eq!(detail["inLibrary"], true, "{detail}");
	assert_eq!(detail["categoryIds"], json!([cat]), "{detail}");
	let ch8 = chapter_by_number(&app, target, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");
}

#[tokio::test]
async fn migrate_with_history_keeps_origin_progress() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;

	let (status, body) = migrate(
		&app,
		"migrateWithHistory",
		json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}]),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["ok"], true, "{body}");

	// 撤入库但保留源进度行（历史页仍可回溯源阅读记录）
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], false, "{detail}");
	assert_eq!(detail["categoryIds"], json!([]), "{detail}");
	let ch8 = chapter_by_number(&app, origin, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");

	let target = body["results"][0]["toMangaId"].as_i64().expect("toMangaId");
	let ch8 = chapter_by_number(&app, target, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");
}

#[tokio::test]
async fn carry_merges_with_existing_target() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;

	// 目标源里同 key 条目已存在且在库，自带一份进度
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": second, "mangaKey": key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let existing = body["id"].as_i64().expect("existing id");
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": existing}))).await;
	assert_eq!(status, StatusCode::OK);
	let tch8 = chapter_by_number(&app, existing, 8.0).await;
	set_progress(&app, tch8["id"].as_i64().unwrap(), 5, true).await;
	let kept_read_at = chapter_by_number(&app, existing, 8.0).await["lastReadAt"].clone();
	assert!(kept_read_at.is_number());

	let (status, body) = migrate(&app, "migrate", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["results"][0];
	assert_eq!(result["ok"], true, "{result}");
	// 复用既有行，不新建
	assert_eq!(result["toMangaId"].as_i64(), Some(existing), "{result}");

	// 合并取较新值：lastPageRead 取大、isRead 取或、lastReadAt 取新
	let ch8 = chapter_by_number(&app, existing, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");
	assert_eq!(ch8["lastReadAt"], kept_read_at, "{ch8}");
	// 源条目 ch7 的进度目标原本没有，照样搬入
	let ch7 = chapter_by_number(&app, existing, 7.0).await;
	assert_eq!(ch7["lastPageRead"], 3, "{ch7}");
}

#[tokio::test]
async fn carry_by_number_and_volume_fallback_with_warnings() {
	let TestApp { app, tmp, db_path, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;
	// 预注册目标，迁移直接复用，手工改造双方章节行以对齐匹配规则
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": second, "mangaKey": key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let target = body["id"].as_i64().expect("target id");

	// 源 ch7 改成目标没有的 99.5：进度对不上要进 warnings
	let och7 = chapter_by_number(&app, origin, 7.0).await["id"].as_i64().unwrap();
	// 源 ch6 改成无章节号只有卷号 3；目标 ch6 同样改造：卷号兜底匹配
	let och6 = chapter_by_number(&app, origin, 6.0).await["id"].as_i64().unwrap();
	let tch6 = chapter_by_number(&app, target, 6.0).await["id"].as_i64().unwrap();
	set_progress(&app, och6, 4, false).await;
	{
		let pool = connect_db(&db_path).await;
		for (id, number, volume) in [(och7, Some(99.5), None), (och6, None, Some(3.0)), (tch6, None, Some(3.0))] {
			sqlx::query("UPDATE chapter SET number = ?, volume = ? WHERE id = ?")
				.bind(number)
				.bind(volume)
				.bind(id)
				.execute(&pool)
				.await
				.expect("craft chapter row");
		}
		pool.close().await;
	}

	let (status, body) = migrate(&app, "copy", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["results"][0];
	assert_eq!(result["ok"], true, "{result}");
	assert_eq!(result["toMangaId"].as_i64(), Some(target), "{result}");
	let warnings = result["warnings"].as_array().expect("warnings");
	assert_eq!(warnings.len(), 1, "{result}");
	assert!(warnings[0].as_str().unwrap().contains("99.5"), "{:?}", warnings);

	// 章节号精确匹配照常；卷号兜底落在目标无章节号的章节上
	let ch8 = chapter_by_number(&app, target, 8.0).await;
	assert_eq!(ch8["isRead"], true, "{ch8}");
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{target}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let crafted = body
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["id"] == tch6)
		.cloned()
		.expect("crafted chapter");
	assert_eq!(crafted["lastPageRead"], 4, "{crafted}");
	// 对不上的 ch7 进度没有落在任何目标章节上
	for c in body.as_array().unwrap() {
		if c["id"] == tch6 || c["chapter_number"] == 8.0 {
			continue;
		}
		assert_eq!(c["lastPageRead"], 0, "{c}");
	}
}

#[tokio::test]
async fn entry_failures_are_reported_per_row() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;

	// 另一部同源漫画只注册不入库
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{example}/search"),
		Some(json!({"query": "Manga 2", "page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let key2 = body["entries"][0]["key"].as_str().unwrap().to_string();
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": example, "mangaKey": key2})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let not_in_library = body["id"].as_i64().expect("id");

	let entries = json!([
		{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key},
		{"fromMangaId": 99999, "toSourceId": second, "toMangaKey": key},
		{"fromMangaId": not_in_library, "toSourceId": second, "toMangaKey": key2},
		{"fromMangaId": origin, "toSourceId": 99999, "toMangaKey": key},
		{"fromMangaId": origin, "toSourceId": example, "toMangaKey": key},
	]);
	let (status, body) = migrate(&app, "migrate", entries).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let results = body["results"].as_array().expect("results");
	assert_eq!(results.len(), 5, "{body}");
	assert_eq!(results[0]["ok"], true, "{:?}", results[0]);
	assert_eq!(results[1]["ok"], false, "{:?}", results[1]);
	assert_eq!(results[1]["reason"], "not_found", "{:?}", results[1]);
	assert_eq!(results[2]["ok"], false, "{:?}", results[2]);
	assert_eq!(results[2]["reason"], "not_in_library", "{:?}", results[2]);
	assert_eq!(results[3]["ok"], false, "{:?}", results[3]);
	assert_eq!(results[3]["reason"], "source_not_found", "{:?}", results[3]);
	assert_eq!(results[4]["ok"], false, "{:?}", results[4]);
	assert_eq!(results[4]["reason"], "same_source", "{:?}", results[4]);

	// 批次级部分失败不回滚：第一条的迁移已生效
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], false, "{detail}");

	// 非法 mode 与空批次是请求级错误
	let (status, body) = migrate(&app, "move", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let (status, body) = migrate(&app, "copy", json!([])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
