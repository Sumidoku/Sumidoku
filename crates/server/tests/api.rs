mod common;

use axum::http::StatusCode;
use common::{TestApp, register_example_manga, request, request_full, source_by_key, test_app};
use serde_json::{Value, json};

// 1x1 transparent PNG
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[tokio::test]
async fn http_response_logging_is_info_and_omits_query() {
	let (logs, _guard) = common::capture_logs();
	let TestApp { app, .. } = test_app().await;
	logs.take();

	let (status, _) = request(&app, "GET", "/api/v1/version?token=query-secret", None).await;
	assert_eq!(status, StatusCode::OK);
	let text = logs.take();
	let line = text
		.lines()
		.find(|line| line.contains("finished processing request"))
		.unwrap_or_else(|| panic!("HTTP response log missing at INFO: {text}"));
	for field in ["INFO", "method=GET", "path=/api/v1/version", "status=200", "latency="] {
		assert!(line.contains(field), "missing {field:?} in {line}");
	}
	assert!(!text.contains("query-secret"), "query leaked into logs: {text}");
}

/// A database failure answers a generic 500: the SQLite text (SQL, table names, paths) goes to the log only
#[tokio::test]
async fn database_errors_do_not_leak_into_responses() {
	let (logs, _guard) = common::capture_logs();
	let TestApp { app, db_path, tmp: _tmp } = test_app().await;
	let pool = common::connect_db(&db_path).await;
	sqlx::query("DROP TABLE download").execute(&pool).await.expect("drop table");
	let (status, body) = request(&app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
	assert_eq!(body["error"]["code"], "internal");
	assert_eq!(body["error"]["message"], "internal server error", "{body}");
	let text = common::wait_for_log(&logs, "request failed").await;
	assert!(text.contains("no such table: download"), "the cause must reach the log: {text}");
}

#[tokio::test]
async fn full_flow() {
	let TestApp { app, tmp, .. } = test_app().await;

	// health
	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");

	// version
	let (status, body) = request(&app, "GET", "/api/v1/version", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

	// 404 when not installed
	let (status, body) = request(&app, "GET", "/api/v1/manga/999", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	// scan installs
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!(["en.example-source"]));
	assert_eq!(body["updated"], json!([]));
	assert_eq!(body["skipped"], json!([]));

	// second scan of the unchanged package reports nothing: same version is a silent refresh, not an update
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!([]));
	assert_eq!(body["updated"], json!([]));
	assert_eq!(body["skipped"], json!([]));

	// a version bump makes the next scan report the source as updated
	let manifest_path = tmp.path().join("sources/en.example-source/source.json");
	let mut manifest: Value =
		serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest")).expect("manifest json");
	manifest["info"]["version"] = json!(2);
	std::fs::write(&manifest_path, serde_json::to_vec(&manifest).expect("manifest bytes")).expect("write manifest");
	let (status, body) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["installed"], json!([]));
	assert_eq!(body["updated"], json!(["en.example-source"]));

	// sources list: the built-in local source is persisted by the migration, and its pinned large id always sorts after user sources
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

	// search returns 20 entries
	let search_path = format!("/api/v1/sources/{source_id}/search");
	let (status, body) = request(&app, "POST", &search_path, Some(json!({"page": 1}))).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 20);
	assert_eq!(body["hasNextPage"], true);
	assert_eq!(entries[0]["sourceId"], source_id);
	let manga_key = entries[0]["key"].as_str().expect("manga key").to_string();

	// query filtering
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

	// shelve
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");

	// details
	let manga_path = format!("/api/v1/manga/{manga_id}");
	let (status, body) = request(&app, "GET", &manga_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["initialized"], true);
	assert_eq!(body["chapterCount"], 8);

	// chapters: 8 chapters in source_order
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters array");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0]["chapter_number"], json!(8.0));
	assert!(chapters.iter().all(|c| c["id"].is_i64() && c["pageCount"].is_i64()));
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");

	// pages: 2 pages, Url + Text
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

	// second time goes through the page cache: fields match the source path
	let (status, body) = request(&app, "GET", &pages_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let cached = body.as_array().expect("cached pages");
	assert_eq!(cached.len(), 2);
	assert_eq!(cached[0]["hasDescription"], true, "缓存路径应与源路径字段一致");

	// page_count write-back
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body[0]["pageCount"], 2);

	// the bare ?refresh flag forces a refetch
	let (status, body) = request(&app, "GET", &format!("{pages_path}?refresh"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("refreshed pages").len(), 2);

	// settings hot-restart: setting=true → settings gains a Toggle
	let settings_path = format!("/api/v1/sources/{source_id}/settings");
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &settings_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["settings"].as_array().expect("settings array").len(), 2);
	assert_eq!(body["values"]["setting"], true);

	// setting=false → back to just one
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

	// override one setting
	let (status, _) = request(&app, "PUT", &settings_path, Some(json!({"setting": true}))).await;
	assert_eq!(status, StatusCode::OK);

	// reset: overrides are cleared; the next get_settings re-seeds the source-declared defaults
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

	// clear cache: idempotent success
	let clear_path = format!("/api/v1/sources/{source_id}/cache/clear");
	let (status, body) = request(&app, "POST", &clear_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["ok"], true);

	// source doesn't support basic login → 501 unsupported
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

	// source has no icon → 404
	let (status, body) = request(&app, "GET", &icon_path, None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
	// source doesn't exist → 404
	let (status, _) = request(&app, "GET", "/api/v1/sources/999/icon", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// drop in icon.png and re-scan → 200
	std::fs::write(tmp.path().join("sources/en.example-source/icon.png"), PNG).expect("write icon");
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, headers, bytes) = request_full(&app, "GET", &icon_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=86400");
	assert_eq!(bytes, PNG);

	// icon_path is still in the DB but the file is gone → 404
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

	// {} and an empty body are both no-ops
	let (status, body) = request(&app, "PATCH", &manga_path, Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["update_strategy"], "Never");
	let (status, body) = request(&app, "PATCH", &manga_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["update_strategy"], "Never");

	// invalid values 400 and are not persisted
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

	// when the source provides no viewer/thumbnail, the response stays Unknown/null
	let (_, body) = request(&app.app, "GET", &manga_path, None).await;
	assert_eq!(body["viewer"], "Unknown");
	let (_, body) = request(&app.app, "GET", &chapters_path, None).await;
	assert!(body[0]["thumbnail"].is_null());

	// persisted viewer/thumbnail must appear in the response (2 = RightToLeft)
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

	// a non-zero manga_user.viewer_flags is a user override: the response viewer uses the override (4 = Webtoon), the source value stays in the DB
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
	// the override only affects the response; the source viewer in the DB is untouched
	let (stored,): (i64,) = sqlx::query_as("SELECT viewer FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("stored viewer");
	assert_eq!(stored, 2);
}
