mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, connect_db, fixture_dir, install_fixture_source, request, test_app};
use serde_json::{Value, json};

/// Installs a second fixture source and scans, returning (example_id, second_id)
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

/// Registers "Manga 1" on the example source and shelves it, returning (manga_id, manga_key)
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

/// Chapter JSON for the given chapter number; Null when absent
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

/// Standard origin entry: example's shelved "Manga 1" + category + ch8/ch7 progress, returning (origin_id, key, cat_id)
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

	// pin order comes down with the list: the default priority for composing migration targets
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

	// manual re-search: query overrides the original title; no match is the not found of the batch flow
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
	let pool = connect_db(&tmp.path().join("test.db")).await;
	let tracking: (i64, i64) = sqlx::query_as(
		"SELECT last_modified_at,
		 (SELECT COUNT(*) FROM sync_change WHERE entity_kind = 'manga_user' AND entity_key = ?)
		 FROM manga_user WHERE manga_id = ? AND user_id = 1",
	)
	.bind(format!("{target}:1"))
	.bind(target)
	.fetch_one(&pool)
	.await
	.expect("migrated tracking");
	assert!(tracking.0 >= 1_000_000_000_000);
	assert_eq!(tracking.1, 1);
	pool.close().await;

	// COPY keeps the origin entry: library state, categories, progress all as-is
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], true, "{detail}");
	assert_eq!(detail["categoryIds"], json!([cat]), "{detail}");
	let ch8 = chapter_by_number(&app, origin, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");

	// target entry: shelved (keeping the origin's shelve time), categories moved over, progress aligned by chapter number
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

	// library filtered by source: only the target entry under the target source, inLibraryAt matches the origin's
	let (status, body) = request(&app, "GET", &format!("/api/v1/library?sourceId={second}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1, "{body}");
	assert_eq!(entries[0]["id"], target, "{body}");
	assert_eq!(entries[0]["inLibraryAt"].as_i64(), Some(origin_added_at), "{body}");

	let before = common::db_dump(&tmp.path().join("test.db")).await;
	let (status, body) = migrate(&app, "copy", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::OK, "identical copy failed: {body}");
	assert_eq!(common::db_dump(&tmp.path().join("test.db")).await, before);
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

	// MIGRATE is a replacement: the origin entry leaves the library, categories cleared, progress rows wiped
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

	// unshelved but the origin's progress rows are kept (the history page can still trace back the origin's reading records)
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

/// Same source with a different key is the forceMigrate fallback when a breaking change has no
/// self-migration capability: the new key starts a new row, progress moves by chapter number; same
/// key is same_source (see entry_failures)
#[tokio::test]
async fn same_source_different_key_migrates() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, _second) = two_sources(&app, &tmp).await;
	let (origin, _key, cat) = stocked_origin(&app, example).await;

	// another entry on the same source serves as the "new home after the rekey"
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{example}/search"),
		Some(json!({"query": "Manga 2", "page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let new_key = body["entries"][0]["key"].as_str().expect("new key").to_string();

	let (status, body) = migrate(
		&app,
		"migrate",
		json!([{"fromMangaId": origin, "toSourceId": example, "toMangaKey": new_key}]),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let result = &body["results"][0];
	assert_eq!(result["ok"], true, "{result}");
	let target = result["toMangaId"].as_i64().expect("toMangaId");
	assert_ne!(target, origin, "{result}");

	let detail = manga_detail(&app, target).await;
	assert_eq!(detail["inLibrary"], true, "{detail}");
	assert_eq!(detail["sourceId"], example, "{detail}");
	assert_eq!(detail["categoryIds"], json!([cat]), "{detail}");
	let ch8 = chapter_by_number(&app, target, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");

	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], false, "{detail}");
}

#[tokio::test]
async fn carry_merges_with_existing_target() {
	let TestApp { app, tmp, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;

	// an entry with the same key already exists in the target source and is shelved, with its own progress
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
	// reuses the existing row, no new one created
	assert_eq!(result["toMangaId"].as_i64(), Some(existing), "{result}");

	// merge keeps the newer values: lastPageRead takes the max, isRead takes the OR, lastReadAt takes the newer
	let ch8 = chapter_by_number(&app, existing, 8.0).await;
	assert_eq!(ch8["lastPageRead"], 7, "{ch8}");
	assert_eq!(ch8["isRead"], true, "{ch8}");
	assert_eq!(ch8["lastReadAt"], kept_read_at, "{ch8}");
	// the target had no ch7 progress of its own; the origin's is carried in anyway
	let ch7 = chapter_by_number(&app, existing, 7.0).await;
	assert_eq!(ch7["lastPageRead"], 3, "{ch7}");
}

#[tokio::test]
async fn carry_by_number_and_volume_fallback_with_warnings() {
	let TestApp { app, tmp, db_path, .. } = test_app().await;
	let (example, second) = two_sources(&app, &tmp).await;
	let (origin, key, _cat) = stocked_origin(&app, example).await;
	// pre-register the target so the migration reuses it directly; hand-craft both sides' chapter rows to line up with the matching rules
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": second, "mangaKey": key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let target = body["id"].as_i64().expect("target id");

	// change the origin's ch7 to 99.5, which the target lacks: unmatchable progress goes into warnings
	let och7 = chapter_by_number(&app, origin, 7.0).await["id"].as_i64().unwrap();
	// change the origin's ch6 to have no chapter number, only volume 3; craft the target's ch6 the same way: volume fallback matching
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

	// exact chapter-number matching works as usual; the volume fallback lands on the target chapter without a chapter number
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
	// the unmatched ch7 progress didn't land on any target chapter
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

	// another manga on the same source, registered but not shelved
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

	// partial failure at batch level doesn't roll back: the first entry's migration has taken effect
	let detail = manga_detail(&app, origin).await;
	assert_eq!(detail["inLibrary"], false, "{detail}");

	// an illegal mode and an empty batch are request-level errors
	let (status, body) = migrate(&app, "move", json!([{"fromMangaId": origin, "toSourceId": second, "toMangaKey": key}])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let (status, body) = migrate(&app, "copy", json!([])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
