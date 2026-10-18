mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, db_dump, register_example_manga, request, request_bytes, test_app};
use serde_json::{Value, json};

/// 由 tests/fixtures/gen_example_aib.py 生成（python plistlib，binary plist）；
/// 内容：库 3 条（1 条未安装源）、进度 4 条（1 条未安装源）、分类 2 个（string/object 两种形态）
const AIB: &[u8] = include_bytes!("fixtures/example.aib");

const DATE_ADDED_MANGA2: i64 = 1704960000; // 2024-01-11 08:00:00 UTC
const DATE_READ_CH3: i64 = 1706781600; // 2024-02-01 10:00:00 UTC
const DATE_READ_CH7: i64 = 1706868000; // 2024-02-02 10:00:00 UTC
const DATE_READ_MANGA2: i64 = 1706954400; // 2024-02-03 10:00:00 UTC

async fn post_aib(app: &Router, query: &str) -> (StatusCode, Value) {
	let (status, _, bytes) = request_bytes(
		app,
		"POST",
		&format!("/api/v1/import/aidoku{query}"),
		AIB.to_vec(),
		"application/octet-stream",
	)
	.await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

async fn scan(app: &Router) {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
}

/// 按章节 key 从漫画章节列表取行
async fn chapter_by_key(app: &Router, manga_id: i64, key: &str) -> Value {
	let (_, chapters) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	chapters
		.as_array()
		.expect("chapters")
		.iter()
		.find(|c| c["key"] == key)
		.unwrap_or_else(|| panic!("chapter {key}"))
		.clone()
}

#[tokio::test]
async fn import_aidoku_merges_with_existing_state() {
	let app = test_app().await;
	scan(&app.app).await;
	let manga_id = register_example_manga(&app.app).await;

	// 预置分类 Evening：改三态，导入不许覆盖已存在分类的定义
	let (status, body) = request(&app.app, "POST", "/api/v1/categories", Some(json!({"name": "Evening"}))).await;
	assert_eq!(status, StatusCode::OK);
	let evening_id = body["id"].as_i64().expect("category id");
	let (status, _) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/categories/{evening_id}"),
		Some(json!({"includeInUpdate": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [evening_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (_, lib_before) = request(&app.app, "GET", "/api/v1/library", None).await;
	let added_before = lib_before["entries"][0]["inLibraryAt"].as_i64().expect("inLibraryAt");

	// ch3：已有进度页码更高（9 > 备份的 5），max 必须保留 9；PATCH 的 lastReadAt 比备份新
	let ch3 = chapter_by_key(&app.app, manga_id, "3").await;
	let ch3_id = ch3["id"].as_i64().expect("ch3 id");
	let (status, _) = request(
		&app.app,
		"PATCH",
		&format!("/api/v1/chapter/{ch3_id}"),
		Some(json!({"lastPageRead": 9})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let ch3_before = chapter_by_key(&app.app, manga_id, "3").await;

	// ch7：直接 SQL 置一条很旧的 last_read_at，备份的 dateRead 更新，必须被备份覆盖
	let ch7 = chapter_by_key(&app.app, manga_id, "7").await;
	let ch7_id = ch7["id"].as_i64().expect("ch7 id");
	let pool = connect_db(&app.db_path).await;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, 1, 0, 0, 0, 1)",
	)
	.bind(ch7_id)
	.execute(&pool)
	.await
	.expect("seed ch7 progress");
	pool.close().await;

	let (status, body) = post_aib(&app.app, "").await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["dryRun"], false);
	assert_eq!(body["imported"]["library"], 2);
	assert_eq!(body["imported"]["categories"], 3);
	assert_eq!(body["imported"]["progress"], 3);
	let skipped = body["skipped"].as_array().expect("skipped");
	assert_eq!(skipped.len(), 2, "skipped: {skipped:?}");
	assert!(skipped.iter().all(|s| s["reason"] == "source not installed"));
	let library_skip = skipped
		.iter()
		.find(|s| s["section"] == "library")
		.expect("library skip");
	assert_eq!(library_skip["sourceKey"], "en.ghost-source");
	assert_eq!(library_skip["mangaKey"], "ghost-1");
	let progress_skip = skipped
		.iter()
		.find(|s| s["section"] == "progress")
		.expect("progress skip");
	assert_eq!(progress_skip["sourceKey"], "en.ghost-source");
	assert_eq!(progress_skip["mangaKey"], "ghost-1");
	assert_eq!(progress_skip["chapterKey"], "9");

	// 书库：manga 1 已存在，标题不被备份元数据覆盖；manga 2 按备份元数据新建
	let (_, lib) = request(&app.app, "GET", "/api/v1/library", None).await;
	let entries = lib["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 2);
	let manga1 = entries
		.iter()
		.find(|e| e["id"] == manga_id)
		.expect("manga 1 entry");
	assert_eq!(manga1["title"], "");
	assert_eq!(manga1["inLibraryAt"], added_before);
	let manga2 = entries
		.iter()
		.find(|e| e["title"] == "Second Manga")
		.expect("manga 2 entry");
	assert_eq!(manga2["authors"], json!(["Author B"]));
	assert_eq!(manga2["inLibraryAt"], DATE_ADDED_MANGA2);
	let manga2_id = manga2["id"].as_i64().expect("manga 2 id");

	// 进度合并：max(页码) / OR(已读) / 较新(时间)
	let ch3_after = chapter_by_key(&app.app, manga_id, "3").await;
	assert_eq!(ch3_after["lastPageRead"], 9);
	assert_eq!(ch3_after["isRead"], false);
	assert_eq!(ch3_after["lastReadAt"], ch3_before["lastReadAt"]);
	let ch7_after = chapter_by_key(&app.app, manga_id, "7").await;
	assert_eq!(ch7_after["isRead"], true);
	assert_eq!(ch7_after["lastReadAt"], DATE_READ_CH7);

	// 导入中新建的 manga 2：章节行按备份 chapters 元数据补，进度落库
	let m2_ch1 = chapter_by_key(&app.app, manga2_id, "1").await;
	assert_eq!(m2_ch1["title"], "Ch 1");
	assert_eq!(m2_ch1["lastPageRead"], 2);
	assert_eq!(m2_ch1["isRead"], false);
	assert_eq!(m2_ch1["lastReadAt"], DATE_READ_MANGA2);

	// 分类：已存在的 Evening 三态不动；Reading/Favorites 新建；成员按书库条目补
	let (_, cats) = request(&app.app, "GET", "/api/v1/categories", None).await;
	let cats = cats.as_array().expect("categories");
	let evening = cats.iter().find(|c| c["name"] == "Evening").expect("Evening");
	assert_eq!(evening["includeInUpdate"], 1);
	assert_eq!(evening["mangaCount"], 1);
	let reading = cats.iter().find(|c| c["name"] == "Reading").expect("Reading");
	assert_eq!(reading["mangaCount"], 2);
	let favorites = cats
		.iter()
		.find(|c| c["name"] == "Favorites")
		.expect("Favorites");
	assert_eq!(favorites["mangaCount"], 1);
}

/// max / OR / 较新 三个方向的定点断言：改覆盖语义会在这里变红
#[tokio::test]
async fn import_aidoku_progress_takes_max_or_newer() {
	let app = test_app().await;
	scan(&app.app).await;
	let manga_id = register_example_manga(&app.app).await;
	let pool = connect_db(&app.db_path).await;

	// ch3 已读且页码更低、时间更旧：备份 progress=5 未读完 → 页码升 5、已读保持、时间取备份
	let ch3 = chapter_by_key(&app.app, manga_id, "3").await;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, 1, 3, 1, 0, 1)",
	)
	.bind(ch3["id"].as_i64().expect("ch3 id"))
	.execute(&pool)
	.await
	.expect("seed ch3");
	// ch7 页码更高但未读、无时间：备份 progress=-1 已读完 → 页码保持 9、已读置真、时间取备份
	let ch7 = chapter_by_key(&app.app, manga_id, "7").await;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, 1, 9, 0, 0, NULL)",
	)
	.bind(ch7["id"].as_i64().expect("ch7 id"))
	.execute(&pool)
	.await
	.expect("seed ch7");
	pool.close().await;

	let (status, body) = post_aib(&app.app, "").await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");

	let ch3_after = chapter_by_key(&app.app, manga_id, "3").await;
	assert_eq!(ch3_after["lastPageRead"], 5);
	assert_eq!(ch3_after["isRead"], true);
	assert_eq!(ch3_after["lastReadAt"], DATE_READ_CH3);
	let ch7_after = chapter_by_key(&app.app, manga_id, "7").await;
	assert_eq!(ch7_after["lastPageRead"], 9);
	assert_eq!(ch7_after["isRead"], true);
	assert_eq!(ch7_after["lastReadAt"], DATE_READ_CH7);
}

#[tokio::test]
async fn import_aidoku_dry_run_reports_without_writing() {
	let app = test_app().await;
	scan(&app.app).await;
	let before = db_dump(&app.db_path).await;

	let (status, body) = post_aib(&app.app, "?dryRun=true").await;
	assert_eq!(status, StatusCode::OK, "dry-run failed: {body}");
	assert_eq!(body["dryRun"], true);
	assert_eq!(body["imported"]["library"], 2);
	assert_eq!(body["imported"]["categories"], 3);
	assert_eq!(body["imported"]["progress"], 3);
	assert_eq!(body["skipped"].as_array().expect("skipped").len(), 2);
	assert_eq!(db_dump(&app.db_path).await, before);

	let (_, lib) = request(&app.app, "GET", "/api/v1/library", None).await;
	assert_eq!(lib["entries"].as_array().expect("entries").len(), 0);
}

#[tokio::test]
async fn import_aidoku_rejects_invalid_input() {
	let app = test_app().await;
	scan(&app.app).await;
	let before = db_dump(&app.db_path).await;

	for (label, body) in [
		("not a plist", b"garbage bytes".to_vec()),
		("json is not plist", br#"{"version":1}"#.to_vec()),
	] {
		let (status, resp) = {
			let (status, _, bytes) = request_bytes(
				&app.app,
				"POST",
				"/api/v1/import/aidoku",
				body,
				"application/octet-stream",
			)
			.await;
			(status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
		};
		assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {resp}");
		assert_eq!(resp["error"]["code"], "bad_request", "{label}");
	}

	// 合法 plist 但顶层不是 Backup 字典：shape 不符同样 400
	let mut buf = Vec::new();
	plist::Value::Array(vec![plist::Value::from(1)])
		.to_writer_binary(&mut buf)
		.expect("write plist");
	let (status, _, bytes) = request_bytes(
		&app.app,
		"POST",
		"/api/v1/import/aidoku",
		buf,
		"application/octet-stream",
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let resp: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	assert_eq!(resp["error"]["code"], "bad_request");

	assert_eq!(db_dump(&app.db_path).await, before);
}
