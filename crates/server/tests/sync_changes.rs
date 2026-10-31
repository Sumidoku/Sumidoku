mod common;

use axum::http::StatusCode;
use common::{TestApp, connect_db, register_example_manga, request, test_app};
use serde_json::json;
use sqlx::SqlitePool;
use sqlx::migrate::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use sumidoku_server::sync::{SyncChange, changes_since};

async fn high_watermark(pool: &SqlitePool) -> i64 {
	common::publish_raw_writes(pool).await;
	sqlx::query_scalar("SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)")
		.fetch_one(pool)
		.await
		.expect("high watermark")
}

async fn since(pool: &SqlitePool, user_id: i64, cursor: i64) -> Vec<SyncChange> {
	common::publish_raw_writes(pool).await;
	changes_since(pool, user_id, cursor, 10_000)
		.await
		.expect("changes")
		.changes
}

fn kinds(events: &[SyncChange]) -> Vec<(&str, bool)> {
	events
		.iter()
		.map(|event| (event.entity_kind.as_str(), event.deleted))
		.collect()
}

fn assert_time_contract(events: &[SyncChange]) {
	for event in events {
		let snapshot_time = event.snapshot["lastModifiedAt"]
			.as_i64()
			.unwrap_or_else(|| panic!("missing snapshot time: {event:?}"));
		if !event.deleted {
			assert_eq!(event.modified_at, snapshot_time, "non-delete time mismatch: {event:?}");
		}
	}
}

async fn local_source_id(pool: &SqlitePool) -> i64 {
	sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
		.fetch_one(pool)
		.await
		.expect("local source")
}

async fn add_manga(pool: &SqlitePool, source_id: i64, key: &str, modified_at: i64) -> i64 {
	sqlx::query_scalar(
		"INSERT INTO manga (source_id, key, title, last_modified_at) VALUES (?, ?, ?, ?) RETURNING id",
	)
	.bind(source_id)
	.bind(key)
	.bind(format!("Manga {key}"))
	.bind(modified_at)
	.fetch_one(pool)
	.await
	.expect("add manga")
}

async fn add_category(pool: &SqlitePool, user_id: i64, name: &str, modified_at: i64) -> i64 {
	let id = sqlx::query_scalar(
		"INSERT INTO category (user_id, name, last_modified_at) VALUES (?, ?, ?) RETURNING id",
	)
	.bind(user_id)
	.bind(name)
	.bind(modified_at)
	.fetch_one(pool)
	.await
	.expect("add category");
	common::publish_category(pool, id).await;
	id
}

async fn manga_modified(pool: &SqlitePool, manga_id: i64) -> i64 {
	sqlx::query_scalar("SELECT last_modified_at FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(pool)
		.await
		.expect("manga modified")
}

#[tokio::test]
async fn migration_17_backfills_zero_timestamp_baseline() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
	let options = SqliteConnectOptions::from_str(&url)
		.expect("database url")
		.create_if_missing(true)
		.foreign_keys(true);
	let pool = SqlitePoolOptions::new()
		.max_connections(1)
		.connect_with(options)
		.await
		.expect("connect pre-migration database");
	let migrator = sqlx::migrate!("./migrations");
	let mut conn = pool.acquire().await.expect("migration connection");
	conn.ensure_migrations_table().await.expect("migration table");
	for migration in migrator.iter().filter(|migration| migration.version < 17) {
		conn.apply(migration).await.expect("apply pre-0017 migration");
	}
	drop(conn);

	sqlx::query("INSERT INTO source (id, key, name, wasm_path) VALUES (10, 'test.source', 'Test', '')")
		.execute(&pool)
		.await
		.expect("add old source");
	sqlx::query(
		"INSERT INTO manga (id, source_id, key, title, next_update_time) VALUES
		 (100, 10, 'shared', 'Shared', 123), (101, 10, 'private', 'No user state', NULL)",
	)
	.execute(&pool)
	.await
	.expect("add old manga");
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at, viewer_flags, sort_key, incognito) VALUES
		 (100, 1, 1, 111, 2, 'first', 1)",
	)
	.execute(&pool)
	.await
	.expect("add old user state");
	sqlx::query(
		"INSERT INTO category (id, user_id, name, sort_order, is_default, include_in_update, include_in_download) VALUES
		 (200, 1, 'One', 3, 1, 2, 2)",
	)
	.execute(&pool)
	.await
	.expect("add old categories");
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (200, 100, 1)")
		.execute(&pool)
		.await
		.expect("add old membership");
	pool.close().await;

	let options = SqliteConnectOptions::from_str(&url)
		.expect("database url")
		.create_if_missing(true)
		.foreign_keys(true);
	let pool = SqlitePoolOptions::new()
		.max_connections(1)
		.connect_with(options)
		.await
		.expect("connect migration database");
	let mut conn = pool.acquire().await.expect("migration connection");
	for migration in migrator.iter().filter(|migration| (17..=18).contains(&migration.version)) {
		conn.apply(migration).await.expect("apply 0017/0018 migration");
	}
	drop(conn);
	for table in ["manga", "manga_user", "category", "category_manga"] {
		let values: Vec<i64> = sqlx::query_scalar(&format!("SELECT last_modified_at FROM {table}"))
			.fetch_all(&pool)
			.await
			.expect("old timestamps");
		assert!(values.iter().all(|value| *value == 0), "{table}: {values:?}");
	}
	let rows: Vec<(i64, String, String, bool, i64)> = sqlx::query_as(
		"SELECT user_id, entity_kind, entity_key, deleted, modified_at FROM sync_change ORDER BY sequence",
	)
	.fetch_all(&pool)
	.await
	.expect("baseline rows");
	assert_eq!(rows.len(), 4, "{rows:?}");
	assert_eq!(
		rows.iter()
			.map(|(user, kind, key, deleted, modified)| (*user, kind.as_str(), key.as_str(), *deleted, *modified))
			.collect::<Vec<_>>(),
		vec![
			(1, "manga", "100", false, 0),
			(1, "manga_user", "100:1", false, 0),
			(1, "category", "200", false, 0),
			(1, "category_manga", "200:100:1", false, 0),
		]
	);
	let admin = since(&pool, 1, 0).await;
	assert_time_contract(&admin);
	assert!(admin.iter().all(|event| event.snapshot["lastModifiedAt"] == 0));
	assert_eq!(admin[0].identity["sourceKey"], "test.source");
	assert_eq!(admin[0].identity["mangaKey"], "shared");
	assert_eq!(admin[0].snapshot["title"], "Shared");
	assert!(admin[0].snapshot.get("nextUpdateTime").is_none());
	assert_eq!(admin[1].snapshot["inLibraryAt"], 111);
	assert_eq!(admin[1].snapshot["viewerFlags"], 2);
	assert_eq!(admin[1].snapshot["sortKey"], "first");
	assert_eq!(admin[1].snapshot["incognito"], 1);
	assert_eq!(admin[2].snapshot["includeInDownload"], 2);
	assert!(admin[2].snapshot.get("includeInUpdate").is_none());
	assert_eq!(admin[3].identity["categoryId"], 200);
	assert_eq!(admin[3].identity["mangaKey"], "shared");
	assert!(since(&pool, 1, high_watermark(&pool).await).await.is_empty());
	pool.close().await;
}

#[tokio::test]
async fn noop_upserts_and_bookkeeping_stay_quiet_with_recursive_triggers() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, "one", 1_000).await;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at, viewer_flags, sort_key, incognito, last_modified_at)
		 VALUES (?, 1, 1, 111, 2, 'sort', 1, 1001)",
	)
	.bind(manga_id)
	.execute(&pool)
	.await
	.expect("add user state");
	let category_id = add_category(&pool, 1, "category", 1_003).await;
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, 1004)")
		.bind(category_id)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("add membership");

	let before: (i64, i64, i64, i64) = sqlx::query_as(
		"SELECT
		 (SELECT last_modified_at FROM manga WHERE id = ?),
		 (SELECT last_modified_at FROM manga_user WHERE manga_id = ? AND user_id = 1),
		 (SELECT last_modified_at FROM category WHERE id = ?),
		 (SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = 1)",
	)
	.bind(manga_id)
	.bind(manga_id)
	.bind(category_id)
	.bind(category_id)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("timestamps");
	assert_eq!(before, (1_000, 1_001, 1_003, 1_004));
	let cursor = high_watermark(&pool).await;
	let mut conn = pool.acquire().await.expect("connection");
	sqlx::query("PRAGMA recursive_triggers = ON")
		.execute(&mut *conn)
		.await
		.expect("enable recursive triggers");
	sqlx::query(
		"INSERT INTO manga (source_id, key, title, description) VALUES (?, 'one', 'Manga one', NULL)
		 ON CONFLICT (source_id, key) DO UPDATE SET title = excluded.title, description = excluded.description",
	)
	.bind(source_id)
	.execute(&mut *conn)
	.await
	.expect("identical manga upsert");
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at, viewer_flags, sort_key, incognito)
		 VALUES (?, 1, 1, 111, 2, 'sort', 1)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET
		 in_library = excluded.in_library, in_library_at = excluded.in_library_at,
		 viewer_flags = excluded.viewer_flags, sort_key = excluded.sort_key, incognito = excluded.incognito",
	)
	.bind(manga_id)
	.execute(&mut *conn)
	.await
	.expect("identical user upsert");
	sqlx::query("INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, 9999)")
		.bind(category_id)
		.bind(manga_id)
		.execute(&mut *conn)
		.await
		.expect("ignored membership");
	sqlx::query("UPDATE category SET name = name, sort_order = sort_order, include_in_download = include_in_download WHERE id = ?")
		.bind(category_id)
		.execute(&mut *conn)
		.await
		.expect("identical category update");
	sqlx::query(
		"UPDATE manga SET next_update_time = 10, next_update_is_fallback = 1,
		 last_fetched_at = 20, chapters_last_fetched_at = 30 WHERE id = ?",
	)
	.bind(manga_id)
	.execute(&mut *conn)
	.await
	.expect("bookkeeping update");
	sqlx::query("UPDATE category SET include_in_update = 2, include_in_download = 2 WHERE id = ?")
		.bind(category_id)
		.execute(&mut *conn)
		.await
		.expect("retired category field update");
	drop(conn);

	assert!(since(&pool, 1, cursor).await.is_empty());
	let after: (i64, i64, i64, i64) = sqlx::query_as(
		"SELECT
		 (SELECT last_modified_at FROM manga WHERE id = ?),
		 (SELECT last_modified_at FROM manga_user WHERE manga_id = ? AND user_id = 1),
		 (SELECT last_modified_at FROM category WHERE id = ?),
		 (SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = 1)",
	)
	.bind(manga_id)
	.bind(manga_id)
	.bind(category_id)
	.bind(category_id)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("timestamps after no-ops");
	assert_eq!(after, before);
	let storage_type: String = sqlx::query_scalar("SELECT typeof(last_modified_at) FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("timestamp storage type");
	assert_eq!(storage_type, "integer");
}

#[tokio::test]
async fn actual_changes_preserve_explicit_times() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, "shared", 0).await;
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, 1, 1)")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("share manga");
	let category_id = add_category(&pool, 1, "before", 0).await;
	let cursor = high_watermark(&pool).await;

	sqlx::query(
		"UPDATE manga SET key = 'renamed', title = 'Fresh', authors = '[\"a\"]', artists = '[\"b\"]',
		 description = 'description', tags = '[\"t\"]', status = 2, content_rating = 3, viewer = 4,
		 cover_url = 'cover', url = 'url', initialized = 1, update_strategy = 1, last_modified_at = 9000000000001
		 WHERE id = ?",
	)
	.bind(manga_id)
	.execute(&pool)
	.await
	.expect("metadata update");
	common::publish_manga(&pool, manga_id).await;
	let first = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&first), [("manga", false)]);
	assert_eq!(first[0].modified_at, 9000000000001);
	assert_eq!(first[0].identity["sourceKey"], "local.local");
	assert_eq!(first[0].identity["mangaKey"], "renamed");
	assert_eq!(first[0].snapshot["authors"], json!(["a"]));
	assert_eq!(first[0].snapshot["description"], "description");
	assert_eq!(manga_modified(&pool, manga_id).await, 9000000000001);

	let cursor = high_watermark(&pool).await;
	sqlx::query(
		"UPDATE manga_user SET in_library = 0, in_library_at = 777, viewer_flags = 3,
		 sort_key = 'custom', incognito = 1, last_modified_at = 9000000000002
		 WHERE manga_id = ? AND user_id = 1",
	)
	.bind(manga_id)
	.execute(&pool)
	.await
	.expect("user state update");
	common::publish_library(&pool, manga_id).await;
	let first = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&first), [("manga", false), ("manga_user", false)]);
	assert_time_contract(&first);
	assert_eq!(first[0].modified_at, 9000000000001);
	assert_eq!(first[1].modified_at, 9000000000002);
	assert_eq!(first[1].snapshot["inLibraryAt"], 777);
	assert_eq!(first[1].snapshot["viewerFlags"], 3);
	assert_eq!(first[1].snapshot["sortKey"], "custom");
	assert_eq!(first[1].snapshot["incognito"], 1);
	assert_eq!(manga_modified(&pool, manga_id).await, 9000000000001);

	let cursor = high_watermark(&pool).await;
	sqlx::query(
		"UPDATE category SET name = 'after', sort_order = 8, is_default = 1,
		 include_in_download = 2, include_in_update = 2, last_modified_at = 9000000000000 WHERE id = ?",
	)
	.bind(category_id)
	.execute(&pool)
	.await
	.expect("category update");
	common::publish_category(&pool, category_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&events), [("category", false)]);
	assert_eq!(events[0].modified_at, 9000000000000);
	assert_eq!(events[0].snapshot["name"], "after");
	assert_eq!(events[0].snapshot["sortOrder"], 8);
	assert_eq!(events[0].snapshot["isDefault"], 1);
	assert_eq!(events[0].snapshot["includeInDownload"], 0);

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE manga SET last_modified_at = 999999 WHERE id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("timestamp-only update");
	assert!(since(&pool, 1, cursor).await.is_empty());
	assert_eq!(manga_modified(&pool, manga_id).await, 999999);
}

#[tokio::test]
async fn library_api_preserves_same_set_and_quietly_repeats_unshelving() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let mut categories = Vec::new();
	for name in ["a", "b", "c"] {
		let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": name}))).await;
		assert_eq!(status, StatusCode::OK, "{body}");
		categories.push(body["id"].as_i64().expect("category id"));
	}
	let [a, b, c]: [i64; 3] = categories.try_into().expect("three categories");
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [a, b]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body.get("lastModifiedAt").is_none());
	let pool = connect_db(&db_path).await;
	let cursor = high_watermark(&pool).await;
	let before: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT category_id, last_modified_at FROM category_manga WHERE manga_id = ? ORDER BY category_id",
	)
	.bind(manga_id)
	.fetch_all(&pool)
	.await
	.expect("membership timestamps");
	let shared_before = manga_modified(&pool, manga_id).await;

	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [b, a]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": [a, b]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(since(&pool, 1, cursor).await.is_empty());
	let after: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT category_id, last_modified_at FROM category_manga WHERE manga_id = ? ORDER BY category_id",
	)
	.bind(manga_id)
	.fetch_all(&pool)
	.await
	.expect("membership timestamps after same set");
	assert_eq!(after, before);
	assert_eq!(manga_modified(&pool, manga_id).await, shared_before);

	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": [b, c]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let events = since(&pool, 1, cursor).await;
	assert_eq!(
		kinds(&events),
		[("manga", false), ("category_manga", true), ("manga", false), ("category_manga", false)]
	);
	assert!(events.iter().any(|event| event.deleted && event.identity["categoryId"] == a));
	assert!(events.iter().any(|event| !event.deleted && event.identity["categoryId"] == c));
	let b_time: i64 = sqlx::query_scalar(
		"SELECT last_modified_at FROM category_manga WHERE category_id = ? AND manga_id = ? AND user_id = 1",
	)
	.bind(b)
	.bind(manga_id)
	.fetch_one(&pool)
	.await
	.expect("preserved membership");
	assert_eq!(b_time, before.into_iter().find(|(id, _)| *id == b).expect("b before").1);

	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": "https://images.example/cover.png"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(kinds(&since(&pool, 1, cursor).await), [("manga", false)]);
	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": "https://images.example/cover.png"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(since(&pool, 1, cursor).await.is_empty());

	let cursor = high_watermark(&pool).await;
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let events = since(&pool, 1, cursor).await;
	assert_eq!(events.len(), 6);
	assert_eq!(events.iter().filter(|event| event.entity_kind == "manga").count(), 3);
	assert_eq!(events.iter().filter(|event| event.entity_kind == "manga_user" && !event.deleted).count(), 1);
	assert_eq!(events.iter().filter(|event| event.entity_kind == "category_manga" && event.deleted).count(), 2);
	let cursor = high_watermark(&pool).await;
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(since(&pool, 1, cursor).await.is_empty());

	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [a]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(kinds(&since(&pool, 1, cursor).await), [("manga", false), ("category_manga", true)]);

	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [b]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(kinds(&since(&pool, 1, cursor).await), [("manga", false), ("category_manga", true)]);
	let cursor = high_watermark(&pool).await;
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(since(&pool, 1, cursor).await.is_empty());
}

#[tokio::test]
async fn cascades_leave_durable_tombstones_without_duplicate_parent_events() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "delete"}))).await;
	assert_eq!(status, StatusCode::OK);
	let category_id = body["id"].as_i64().expect("category id");
	let (status, _) = request(
		&app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [category_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&db_path).await;

	let cursor = high_watermark(&pool).await;
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/categories/{category_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let events = since(&pool, 1, cursor).await;
	assert_eq!(
		kinds(&events),
		[("category", true), ("manga", false), ("category_manga", true)]
	);
	assert_eq!(events[2].identity["sourceKey"], "en.example-source");
	assert_eq!(events[2].identity["mangaKey"], events[1].identity["mangaKey"]);
	assert_eq!(events[2].identity["categoryId"], category_id);

	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "parent"}))).await;
	assert_eq!(status, StatusCode::OK);
	let category_id = body["id"].as_i64().expect("category id");
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
		.bind(category_id)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("restore membership");
	let cursor = high_watermark(&pool).await;
	common::delete_manga(&pool, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_eq!(events[0].entity_kind, "manga");
	assert!(events[0].deleted);
	for kind in ["manga", "manga_user", "category_manga"] {
		assert_eq!(events.iter().filter(|event| event.entity_kind == kind).count(), 1, "{kind}");
	}
	assert!(events.iter().all(|event| event.identity["sourceKey"] == "en.example-source"));
	assert!(events.iter().all(|event| event.identity["mangaKey"].is_string()));
	assert_eq!(events[0].snapshot["id"], manga_id);
}

#[tokio::test]
async fn pagination_uses_sequences_not_timestamps() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let first = add_manga(&pool, source_id, "first", 42).await;
	let second = add_manga(&pool, source_id, "second", 42).await;
	for manga_id in [first, second] {
		sqlx::query(
			"INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, 42)",
		)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("add paginated state");
	}
	let category_id = add_category(&pool, 1, "page", 42).await;
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, 42)")
		.bind(category_id)
		.bind(first)
		.execute(&pool)
		.await
		.expect("add paginated membership");

	common::publish_raw_writes(&pool).await;
	let first_page = changes_since(&pool, 1, 0, 2).await.expect("first page");
	assert_eq!(first_page.changes.len(), 2);
	assert!(first_page.has_more);
	assert_eq!(first_page.next_cursor, first_page.changes[1].sequence);
	let late = add_category(&pool, 1, "late", 1).await;

	let mut cursor = first_page.next_cursor;
	let mut sequences: Vec<i64> = first_page.changes.iter().map(|event| event.sequence).collect();
	loop {
		let page = changes_since(&pool, 1, cursor, 2).await.expect("next page");
		sequences.extend(page.changes.iter().map(|event| event.sequence));
		cursor = page.next_cursor;
		if !page.has_more {
			break;
		}
	}
	let expected: Vec<i64> = sqlx::query_scalar("SELECT sequence FROM sync_change WHERE user_id = 1 ORDER BY sequence")
		.fetch_all(&pool)
		.await
		.expect("expected sequences");
	assert_eq!(sequences, expected);
	let mut unique = sequences.clone();
	unique.sort_unstable();
	unique.dedup();
	assert_eq!(unique.len(), sequences.len());
	assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
	let late_event = since(&pool, 1, 0)
		.await
		.into_iter()
		.find(|event| event.entity_kind == "category" && event.entity_key == late.to_string())
		.expect("late event");
	assert_eq!(late_event.modified_at, 1);
	assert_eq!(cursor, high_watermark(&pool).await);

	let after_snapshot = add_category(&pool, 1, "after snapshot", 1).await;
	let page = changes_since(&pool, 1, cursor, 10).await.expect("later commit");
	assert_eq!(page.changes.len(), 1);
	assert_eq!(page.changes[0].entity_key, after_snapshot.to_string());
	assert_eq!(page.changes[0].modified_at, 1);
	assert!(page.changes[0].sequence > cursor);
	assert!(!page.has_more);
	assert_eq!(page.next_cursor, page.changes[0].sequence);
	assert!(changes_since(&pool, 1, 0, 0).await.is_err());
	assert!(changes_since(&pool, 1, -1, 1).await.is_err());
}

#[tokio::test]
async fn recursive_triggers_emit_actual_changes_once_with_time_contract() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, "recursive", 1_000).await;
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, 1001)")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("add user state");
	let first_category = add_category(&pool, 1, "first", 1_002).await;
	add_category(&pool, 1, "second", 1_003).await;
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, 1004)")
		.bind(first_category)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("add membership");

	let cursor = high_watermark(&pool).await;
	let mut conn = pool.acquire().await.expect("connection");
	sqlx::query("PRAGMA recursive_triggers = ON")
		.execute(&mut *conn)
		.await
		.expect("enable recursive triggers");
	sqlx::query("UPDATE manga SET title = 'recursive title', last_modified_at = 2000 WHERE id = ?")
		.bind(manga_id)
		.execute(&mut *conn)
		.await
		.expect("actual manga update");
	common::publish_manga(&pool, manga_id).await;
	sqlx::query("UPDATE manga_user SET viewer_flags = 1, last_modified_at = 2001 WHERE manga_id = ? AND user_id = 1")
		.bind(manga_id)
		.execute(&mut *conn)
		.await
		.expect("actual user update");
	common::publish_library(&pool, manga_id).await;
	sqlx::query("UPDATE category SET name = 'recursive category', last_modified_at = 2002 WHERE id = ?")
		.bind(first_category)
		.execute(&mut *conn)
		.await
		.expect("actual category update");
	common::publish_category(&pool, first_category).await;
	drop(conn);

	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(
		kinds(&events),
		[
			("manga", false),
			("manga", false),
			("manga_user", false),
			("category", false),
		]
	);
	assert_eq!(events.len(), 4);
	assert_eq!(events[0].modified_at, 2_000);
	assert_eq!(events[1].modified_at, 2_000);
	assert_eq!(events[2].modified_at, 2_001);
	assert_eq!(events[3].modified_at, 2_002);
}

#[tokio::test]
async fn ordinary_writes_deletes_and_recreation_advance_future_timestamps() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let future = 9_000_000_000_000_000_i64;
	let manga_id = 90_001_i64;
	let first_category = 91_001_i64;
	let second_category = 91_002_i64;
	sqlx::query(
		"INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, 'future', 'Future', ?)",
	)
	.bind(manga_id)
	.bind(source_id)
	.bind(future)
	.execute(&pool)
	.await
	.expect("add future manga");
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, ?)")
		.bind(manga_id)
		.bind(future + 10)
		.execute(&pool)
		.await
		.expect("add future user state");
	for (id, name, modified_at) in [(first_category, "first", future + 20), (second_category, "second", future + 30)] {
		sqlx::query("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (?, 1, ?, ?)")
			.bind(id)
			.bind(name)
			.bind(modified_at)
			.execute(&pool)
			.await
			.expect("add future category");
		common::publish_category(&pool, id).await;
	}
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, ?)")
		.bind(first_category)
		.bind(manga_id)
		.bind(future + 40)
		.execute(&pool)
		.await
		.expect("add future membership");

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE manga SET title = 'Changed' WHERE id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("ordinary manga update");
	common::publish_manga(&pool, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&events), [("manga", false)]);
	assert_eq!(events[0].modified_at, future + 1);
	assert_time_contract(&events);
	assert_eq!(manga_modified(&pool, manga_id).await, future + 1);

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE manga_user SET viewer_flags = 1 WHERE manga_id = ? AND user_id = 1")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("ordinary user update");
	common::publish_library(&pool, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("manga", false), ("manga_user", false)]);
	assert_eq!(events[0].modified_at, future + 1);
	assert_eq!(events[1].modified_at, future + 11);

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE category SET name = 'Changed category' WHERE id = ?")
		.bind(first_category)
		.execute(&pool)
		.await
		.expect("ordinary category update");
	common::publish_category(&pool, first_category).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("category", false)]);
	assert_eq!(events[0].modified_at, future + 21);


	let cursor = high_watermark(&pool).await;
	common::delete_membership(&pool, first_category, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("manga", false), ("category_manga", true)]);
	assert_eq!(events[0].modified_at, future + 1);
	assert_eq!(events[1].modified_at, future + 41);
	assert_eq!(events[1].snapshot["lastModifiedAt"], future + 40);

	let cursor = high_watermark(&pool).await;
	sqlx::query(
		"INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at)
		 VALUES (?, ?, 1, 0)",
	)
	.bind(first_category)
	.bind(manga_id)
	.execute(&pool)
	.await
	.expect("recreate membership");
	common::publish_membership(&pool, first_category, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("manga", false), ("category_manga", false)]);
	assert_eq!(events[0].modified_at, future + 1);
	assert_eq!(events[1].modified_at, future + 42);
	common::delete_membership(&pool, first_category, manga_id).await;
	sqlx::query(
		"INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at)
		 VALUES (?, ?, 1, ?)",
	)
	.bind(second_category)
	.bind(manga_id)
	.bind(future + 43)
	.execute(&pool)
	.await
	.expect("second membership");
	common::publish_membership(&pool, second_category, manga_id).await;

	let cursor = high_watermark(&pool).await;
	common::delete_category(&pool, first_category).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("category", true)]);
	assert_eq!(events[0].modified_at, future + 22);
	assert_eq!(events[0].snapshot["lastModifiedAt"], future + 21);

	let cursor = high_watermark(&pool).await;
	sqlx::query("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (?, 1, 'Recreated', 0)")
		.bind(first_category)
		.execute(&pool)
		.await
		.expect("recreate category");
	common::publish_category(&pool, first_category).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("category", false)]);
	assert_eq!(events[0].modified_at, future + 23);

	let cursor = high_watermark(&pool).await;
	common::delete_manga(&pool, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(events.len(), 3);
	let manga_tombstone = events.iter().find(|event| event.entity_kind == "manga").expect("manga tombstone");
	let user_tombstone = events.iter().find(|event| event.entity_kind == "manga_user").expect("user tombstone");
	let membership_tombstone = events
		.iter()
		.find(|event| event.entity_kind == "category_manga")
		.expect("membership tombstone");
	assert_eq!(manga_tombstone.modified_at, future + 2);
	assert_eq!(manga_tombstone.snapshot["lastModifiedAt"], future + 1);
	assert_eq!(user_tombstone.modified_at, future + 12);
	assert_eq!(user_tombstone.snapshot["lastModifiedAt"], future + 11);
	assert_eq!(membership_tombstone.modified_at, future + 44);
	assert_eq!(membership_tombstone.snapshot["lastModifiedAt"], future + 43);

	sqlx::query(
		"INSERT INTO manga (id, source_id, key, title, last_modified_at)
		 VALUES (?, ?, 'future', 'Recreated', 0)",
	)
	.bind(manga_id)
	.bind(source_id)
	.execute(&pool)
	.await
	.expect("recreate manga");
	common::publish_manga(&pool, manga_id).await;
	assert_eq!(manga_modified(&pool, manga_id).await, future + 3);
	let cursor = high_watermark(&pool).await;
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, 0)")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("recreate user state");
	common::publish_library(&pool, manga_id).await;
	let events = since(&pool, 1, cursor).await;
	assert_time_contract(&events);
	assert_eq!(kinds(&events), [("manga", false), ("manga_user", false)]);
	assert_eq!(events[0].modified_at, future + 3);
	assert_eq!(events[1].modified_at, future + 13);
}

#[tokio::test]
async fn future_cursor_is_rejected() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let cursor = high_watermark(&pool).await;
	let page = changes_since(&pool, 1, cursor, 10).await.expect("current cursor");
	assert!(!page.has_more);
	assert_eq!(page.next_cursor, cursor);
	let error = changes_since(&pool, 1, cursor + 1, 10)
		.await
		.expect_err("future cursor must fail");
	assert_eq!(error.code(), "bad_request");
}

#[tokio::test]
async fn sync_change_history_indexes_exist() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let names: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_index_list('sync_change')")
		.fetch_all(&pool)
		.await
		.expect("index names");
	for expected in [
		"sync_change_user_sequence",
		"sync_change_user_entity_sequence",
		"sync_change_entity_key",
		"sync_change_origin_entity_sequence",
	] {
		assert!(names.iter().any(|name| name == expected), "missing {expected}: {names:?}");
	}
	let user_columns: Vec<String> = sqlx::query_scalar(
		"SELECT name FROM pragma_index_info('sync_change_user_entity_sequence') ORDER BY seqno",
	)
	.fetch_all(&pool)
	.await
	.expect("user entity index columns");
	assert_eq!(user_columns, ["user_id", "entity_kind", "entity_key", "sequence"]);
	let entity_columns: Vec<String> = sqlx::query_scalar(
		"SELECT name FROM pragma_index_info('sync_change_entity_key') ORDER BY seqno",
	)
	.fetch_all(&pool)
	.await
	.expect("entity index columns");
	assert_eq!(entity_columns, ["entity_kind", "entity_key"]);
	let origin_columns: Vec<String> = sqlx::query_scalar(
		"SELECT name FROM pragma_index_info('sync_change_origin_entity_sequence') ORDER BY seqno",
	)
	.fetch_all(&pool)
	.await
	.expect("origin index columns");
	assert_eq!(origin_columns, ["origin", "origin_user_id", "entity_kind", "origin_entity_key", "origin_sequence"]);
}
