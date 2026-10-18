mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, db_dump, fixture_dir, install_fixture_source, register_example_manga, request, request_bytes, test_app};
use serde_json::{Value, json};

/// 由 tests/fixtures/gen_example_tachibk.py 生成（纯 python 手编 protobuf wire）；
/// 三个文件同一 Backup 消息，覆盖三种容器：gzip（.tachibk 真实形态）、裸 proto、zip
const TACHIBK: &[u8] = include_bytes!("fixtures/example.tachibk");
const RAW_PROTO: &[u8] = include_bytes!("fixtures/example.proto");
const ZIP_TACHIBK: &[u8] = include_bytes!("fixtures/example_zip.tachibk");

const DATE_ADDED_MANGA2: i64 = 1704960000; // 2024-01-11 08:00:00 UTC
const DATE_READ_CH3: i64 = 1706781600; // 2024-02-01 10:00:00 UTC
const DATE_READ_CH7: i64 = 1706868000; // 2024-02-02 10:00:00 UTC
const DATE_READ_MANGA2: i64 = 1706954400; // 2024-02-03 10:00:00 UTC
const DATE_READ_HISTORY_ONLY: i64 = 1707127200; // 2024-02-05 10:00:00 UTC

async fn post_backup(app: &Router, query: &str, bytes: &[u8]) -> (StatusCode, Value) {
	let (status, _, resp) = request_bytes(
		app,
		"POST",
		&format!("/api/v1/import/suwayomi{query}"),
		bytes.to_vec(),
		"application/octet-stream",
	)
	.await;
	(status, serde_json::from_slice(&resp).unwrap_or(Value::Null))
}

fn mapping_query(mapping: &Value) -> String {
	let raw = serde_json::to_string(mapping).expect("mapping json");
	let encoded = raw.replace('{', "%7B").replace('}', "%7D").replace('"', "%22");
	format!("?sourceMapping={encoded}")
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

fn source_group<'a>(body: &'a Value, mihon_id: &str) -> &'a Value {
	body["sources"]
		.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["mihonSourceId"] == mihon_id)
		.unwrap_or_else(|| panic!("source group {mihon_id}: {body}"))
}

#[tokio::test]
async fn import_suwayomi_merges_with_existing_state() {
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

	// ch7：直接 SQL 置一条很旧的 last_read_at，备份的 lastRead 更新，必须被备份覆盖
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

	let (status, body) = post_backup(&app.app, "", TACHIBK).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["dryRun"], false);
	assert_eq!(body["imported"]["library"], 2);
	assert_eq!(body["imported"]["categories"], 3);
	assert_eq!(body["imported"]["progress"], 4);

	let skipped = body["skipped"].as_array().expect("skipped");
	assert_eq!(skipped.len(), 1, "skipped: {skipped:?}");
	assert_eq!(skipped[0]["section"], "library");
	assert_eq!(skipped[0]["mihonSourceId"], "222");
	assert_eq!(skipped[0]["mihonSourceName"], "Ghost Source");
	assert_eq!(skipped[0]["mangaKey"], "ghost-1");
	assert_eq!(skipped[0]["reason"], "no source mapping");

	// 源分组报告：111 唯一命中自动匹配，222 无候选 unmatched
	let auto = source_group(&body, "111");
	assert_eq!(auto["mihonSourceName"], "Example Source");
	assert_eq!(auto["mangaCount"], 3);
	assert_eq!(auto["match"], "auto");
	assert_eq!(auto["aidokuSourceKey"], "en.example-source");
	let ghost = source_group(&body, "222");
	assert_eq!(ghost["match"], "unmatched");
	assert_eq!(ghost["mangaCount"], 1);
	assert_eq!(ghost["candidates"].as_array().expect("candidates").len(), 0);

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

	// 进度合并：max(页码) / OR(已读) / 较新(时间)；备份的 bookmark=true 不导入
	let ch3_after = chapter_by_key(&app.app, manga_id, "3").await;
	assert_eq!(ch3_after["lastPageRead"], 9);
	assert_eq!(ch3_after["isRead"], false);
	assert_eq!(ch3_after["isBookmarked"], false);
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

	// favorite=false 的 manga 3：建行但不入库；只在 history 出现的章节 5 也有进度
	let pool = connect_db(&app.db_path).await;
	let row: Option<(i64, i64)> = sqlx::query_as(
		"SELECT m.id, COALESCE(mu.in_library, 0) FROM manga m
		 LEFT JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1 WHERE m.key = '3'",
	)
	.fetch_optional(&pool)
	.await
	.expect("manga 3 row");
	let (manga3_id, in_library) = row.expect("manga 3 must exist for its history");
	assert_eq!(in_library, 0);
	let ch5: (i64, i64, i64) = sqlx::query_as(
		"SELECT cu.last_page_read, cu.is_read, cu.last_read_at FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 WHERE c.manga_id = ? AND c.key = '5'",
	)
	.bind(manga3_id)
	.fetch_one(&pool)
	.await
	.expect("chapter 5 progress");
	assert_eq!(ch5.0, 0);
	assert_eq!(ch5.1, 0);
	assert_eq!(ch5.2, DATE_READ_HISTORY_ONLY);
	pool.close().await;

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
async fn import_suwayomi_progress_takes_max_or_newer() {
	let app = test_app().await;
	scan(&app.app).await;
	let manga_id = register_example_manga(&app.app).await;
	let pool = connect_db(&app.db_path).await;

	// ch3 已读且页码更低、时间更旧：备份 lastPageRead=5 未读完 → 页码升 5、已读保持、时间取备份
	let ch3 = chapter_by_key(&app.app, manga_id, "3").await;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, 1, 3, 1, 0, 1)",
	)
	.bind(ch3["id"].as_i64().expect("ch3 id"))
	.execute(&pool)
	.await
	.expect("seed ch3");
	// ch7 页码更高但未读、无时间：备份 lastPageRead=0 已读完 → 页码保持 9、已读置真、时间取备份
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

	let (status, body) = post_backup(&app.app, "", TACHIBK).await;
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

/// 同名源装了两个时绝不猜：整组进 unmatched 报告并跳过
#[tokio::test]
async fn import_suwayomi_ambiguous_name_never_guesses() {
	let app = test_app().await;
	install_fixture_source(&app.tmp, "en.example-dup", "Example Source", &fixture_dir().join("main.wasm"));
	scan(&app.app).await;

	let (status, body) = post_backup(&app.app, "", TACHIBK).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["imported"]["library"], 0);
	assert_eq!(body["imported"]["progress"], 0);

	let group = source_group(&body, "111");
	assert_eq!(group["match"], "unmatched");
	let mut candidate_keys: Vec<&str> = group["candidates"]
		.as_array()
		.expect("candidates")
		.iter()
		.map(|c| c["key"].as_str().expect("candidate key"))
		.collect();
	candidate_keys.sort();
	assert_eq!(candidate_keys, ["en.example-dup", "en.example-source"]);

	let skipped = body["skipped"].as_array().expect("skipped");
	assert_eq!(skipped.len(), 4, "skipped: {skipped:?}");
	assert!(skipped.iter().all(|s| s["reason"] == "no source mapping"));
	assert!(skipped.iter().any(|s| s["section"] == "progress" && s["mangaKey"] == "3"));

	let (_, lib) = request(&app.app, "GET", "/api/v1/library", None).await;
	assert_eq!(lib["entries"].as_array().expect("entries").len(), 0);
}

/// 显式 sourceMapping 优先于自动匹配，同名歧义也可解开
#[tokio::test]
async fn import_suwayomi_explicit_mapping() {
	let app = test_app().await;
	install_fixture_source(&app.tmp, "en.example-dup", "Example Source", &fixture_dir().join("main.wasm"));
	scan(&app.app).await;

	let query = mapping_query(&json!({"111": "en.example-dup", "222": "en.example-source"}));
	let (status, body) = post_backup(&app.app, &query, TACHIBK).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["imported"]["library"], 3);
	assert_eq!(body["imported"]["progress"], 5);
	assert_eq!(body["skipped"].as_array().expect("skipped").len(), 0);
	assert_eq!(source_group(&body, "111")["match"], "mapped");
	assert_eq!(source_group(&body, "111")["aidokuSourceKey"], "en.example-dup");
	assert_eq!(source_group(&body, "222")["match"], "mapped");

	// manga 按映射落到对应源：ghost-1 → en.example-source，1/2/3 → en.example-dup
	let pool = connect_db(&app.db_path).await;
	let rows: Vec<(String, String)> = sqlx::query_as(
		"SELECT m.key, s.key FROM manga m JOIN source s ON s.id = m.source_id ORDER BY m.key",
	)
	.fetch_all(&pool)
	.await
	.expect("manga rows");
	pool.close().await;
	let expected: Vec<(String, String)> = ["1", "2", "3"]
		.into_iter()
		.map(|k| (k.to_string(), "en.example-dup".to_string()))
		.chain([("ghost-1".to_string(), "en.example-source".to_string())])
		.collect();
	assert_eq!(rows, expected);
}

#[tokio::test]
async fn import_suwayomi_rejects_unknown_mapping_target() {
	let app = test_app().await;
	scan(&app.app).await;
	let before = db_dump(&app.db_path).await;

	let query = mapping_query(&json!({"111": "en.not-installed"}));
	let (status, body) = post_backup(&app.app, &query, TACHIBK).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert_eq!(body["error"]["code"], "bad_request");
	assert_eq!(db_dump(&app.db_path).await, before);
}

#[tokio::test]
async fn import_suwayomi_dry_run_reports_without_writing() {
	let app = test_app().await;
	scan(&app.app).await;
	let before = db_dump(&app.db_path).await;

	let (status, body) = post_backup(&app.app, "?dryRun=true", TACHIBK).await;
	assert_eq!(status, StatusCode::OK, "dry-run failed: {body}");
	assert_eq!(body["dryRun"], true);
	assert_eq!(body["imported"]["library"], 2);
	assert_eq!(body["imported"]["categories"], 3);
	assert_eq!(body["imported"]["progress"], 4);
	assert_eq!(body["skipped"].as_array().expect("skipped").len(), 1);
	assert_eq!(source_group(&body, "111")["match"], "auto");
	assert_eq!(db_dump(&app.db_path).await, before);

	let (_, lib) = request(&app.app, "GET", "/api/v1/library", None).await;
	assert_eq!(lib["entries"].as_array().expect("entries").len(), 0);
}

/// 同一 Backup 消息走三种容器，导入结果一致
#[tokio::test]
async fn import_suwayomi_accepts_raw_proto_and_zip_container() {
	for (label, bytes) in [("raw proto", RAW_PROTO), ("zip container", ZIP_TACHIBK)] {
		let app = test_app().await;
		scan(&app.app).await;
		let (status, body) = post_backup(&app.app, "", bytes).await;
		assert_eq!(status, StatusCode::OK, "{label}: {body}");
		assert_eq!(body["imported"]["library"], 2, "{label}");
		assert_eq!(body["imported"]["progress"], 4, "{label}");
		assert_eq!(body["skipped"].as_array().expect("skipped").len(), 1, "{label}");

		let (_, lib) = request(&app.app, "GET", "/api/v1/library", None).await;
		let entries = lib["entries"].as_array().expect("entries");
		assert_eq!(entries.len(), 2, "{label}");
		assert!(entries.iter().any(|e| e["title"] == "Second Manga"), "{label}");
	}
}

#[tokio::test]
async fn import_suwayomi_rejects_invalid_input() {
	let app = test_app().await;
	scan(&app.app).await;
	let before = db_dump(&app.db_path).await;

	// zip 里没有 .proto 条目：容器合法但内容不符
	let mut zip_buf = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut zip_buf);
		writer
			.start_file("readme.txt", zip::write::SimpleFileOptions::default())
			.expect("start file");
		use std::io::Write;
		writer.write_all(b"not a backup").expect("write");
		writer.finish().expect("finish zip");
	}

	let mut bad_gzip = vec![0x1f, 0x8b];
	bad_gzip.extend_from_slice(b"not really gzip");

	let mut good_gzip_bad_proto = std::io::Cursor::new(Vec::new());
	{
		use flate2::Compression;
		use flate2::write::GzEncoder;
		use std::io::Write;
		let mut enc = GzEncoder::new(&mut good_gzip_bad_proto, Compression::default());
		enc.write_all(b"\xff\xff\xff\xff").expect("write proto junk");
		enc.finish().expect("finish gzip");
	}

	for (label, body) in [
		("garbage bytes", b"garbage bytes".to_vec()),
		("json is not proto", br#"{"version":1}"#.to_vec()),
		("bad gzip stream", bad_gzip),
		("gzip of invalid proto", good_gzip_bad_proto.into_inner()),
		("zip without proto entry", zip_buf.into_inner()),
	] {
		let (status, resp) = post_backup(&app.app, "", &body).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {resp}");
		assert_eq!(resp["error"]["code"], "bad_request", "{label}");
	}

	assert_eq!(db_dump(&app.db_path).await, before);
}
