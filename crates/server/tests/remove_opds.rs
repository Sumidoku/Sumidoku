mod common;

use common::{base_config, request};
use serde_json::{Value, json};
use sqlx::migrate::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;
use sumidoku_server::build_app;

async fn connect(url: &str) -> SqlitePool {
	let options = SqliteConnectOptions::from_str(url)
		.expect("database url")
		.create_if_missing(true)
		.foreign_keys(true);
	SqlitePoolOptions::new()
		.max_connections(1)
		.connect_with(options)
		.await
		.expect("connect database")
}

async fn apply(pool: &SqlitePool, first: i64, last: i64) {
	let migrator = sqlx::migrate!("./migrations");
	let mut conn = pool.acquire().await.expect("migration connection");
	conn.ensure_migrations_table().await.expect("migrations table");
	for migration in migrator
		.iter()
		.filter(|migration| migration.version >= first && migration.version <= last)
	{
		conn.apply(migration).await.expect("apply migration");
	}
}

async fn seed_content(pool: &SqlitePool, source_id: i64, manga_id: i64, chapter_id: i64) {
	sqlx::query("INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, '\"secret\"')")
		.bind(source_id)
		.bind(format!("credential-{manga_id}"))
		.execute(pool)
		.await
		.expect("source setting");
	sqlx::query("INSERT INTO source_cookie (source_id, domain, name, value) VALUES (?, 'example.test', ?, 'secret')")
		.bind(source_id)
		.bind(format!("session-{manga_id}"))
		.execute(pool)
		.await
		.expect("source cookie");
	sqlx::query("INSERT INTO manga (id, source_id, key, title) VALUES (?, ?, ?, 'Manga')")
		.bind(manga_id)
		.bind(source_id)
		.bind(format!("manga-{manga_id}"))
		.execute(pool)
		.await
		.expect("manga");
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, 1, 1)")
		.bind(manga_id)
		.execute(pool)
		.await
		.expect("library state");
	sqlx::query("INSERT INTO chapter (id, manga_id, key) VALUES (?, ?, 'chapter-1')")
		.bind(chapter_id)
		.bind(manga_id)
		.execute(pool)
		.await
		.expect("chapter");
	sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read) VALUES (?, 1, 3, 1)")
		.bind(chapter_id)
		.execute(pool)
		.await
		.expect("chapter progress");
	sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, 0, '{}')")
		.bind(chapter_id)
		.execute(pool)
		.await
		.expect("page");
	sqlx::query("INSERT INTO download (chapter_id, user_id, status) VALUES (?, 1, 'finished')")
		.bind(chapter_id)
		.execute(pool)
		.await
		.expect("download");
}

async fn seed_sources(pool: &SqlitePool) {
	for (id, key, kind) in [(100, "opds.catalog", "opds"), (200, "en.aidoku", "aidoku")] {
		sqlx::query("INSERT INTO source (id, key, name, kind, wasm_path) VALUES (?, ?, 'Source', ?, '')")
			.bind(id)
			.bind(key)
			.bind(kind)
			.execute(pool)
			.await
			.expect("source");
	}
	seed_content(pool, 100, 1000, 10000).await;
	seed_content(pool, 200, 2000, 20000).await;
}

#[tokio::test]
async fn migration_20_removes_opds_state_and_preserves_aidoku() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
	let pool = connect(&url).await;
	apply(&pool, 1, 19).await;
	seed_sources(&pool).await;
	sqlx::query("INSERT INTO category (id, user_id, name) VALUES (500, 1, 'Keep')")
		.execute(&pool)
		.await
		.expect("category");
	for manga_id in [1000, 2000] {
		sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (500, ?, 1)")
			.bind(manga_id)
			.execute(&pool)
			.await
			.expect("category membership");
	}
	for (group_id, name) in [(700, "Mixed"), (701, "OPDS only")] {
		sqlx::query("INSERT INTO filter_group (id, user_id, name) VALUES (?, 1, ?)")
			.bind(group_id)
			.bind(name)
			.execute(&pool)
			.await
			.expect("filter group");
	}
	for (group_id, source_id) in [(700, 100), (700, 200), (701, 100)] {
		sqlx::query("INSERT INTO filter_group_rule (group_id, type, value) VALUES (?, 'source', ?)")
			.bind(group_id)
			.bind(source_id.to_string())
			.execute(&pool)
			.await
			.expect("filter rule");
	}
	sqlx::query("INSERT INTO setting (key, value) VALUES ('browse.pinned_sources', ?)")
		.bind(json!(["opds.catalog", "en.aidoku", "opds.catalog", "stale", "local.local"]).to_string())
		.execute(&pool)
		.await
		.expect("pinned sources");
	let before: Vec<(i64, String, String, bool)> = sqlx::query_as(
		"SELECT sequence, entity_kind, entity_key, deleted FROM sync_change ORDER BY sequence",
	)
	.fetch_all(&pool)
	.await
	.expect("existing sync history");
	let before_max = before.last().expect("seeded sync history").0;
	let opds_keys = ["1000", "1000:1", "10000", "10000:1", "500:1000:1"];
	let expected: Vec<(i64, String, String, bool)> = before
		.iter()
		.filter(|row| !opds_keys.contains(&row.2.as_str()))
		.cloned()
		.collect();

	apply(&pool, 20, 20).await;

	let sources: Vec<String> = sqlx::query_scalar("SELECT key FROM source ORDER BY key")
		.fetch_all(&pool)
		.await
		.expect("remaining sources");
	assert_eq!(sources, ["en.aidoku", "local.local"]);
	let pinned: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'browse.pinned_sources'")
		.fetch_one(&pool)
		.await
		.expect("pinned sources");
	assert_eq!(serde_json::from_str::<Value>(&pinned).expect("pinned json"), json!(["en.aidoku", "stale", "local.local"]));

	for (table, column, opds_id, aidoku_id) in [
		("source_setting", "source_id", 100, 200),
		("source_cookie", "source_id", 100, 200),
		("manga", "source_id", 100, 200),
		("manga_user", "manga_id", 1000, 2000),
		("category_manga", "manga_id", 1000, 2000),
		("chapter", "manga_id", 1000, 2000),
		("chapter_user", "chapter_id", 10000, 20000),
		("page", "chapter_id", 10000, 20000),
		("download", "chapter_id", 10000, 20000),
	] {
		let opds: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?"))
			.bind(opds_id)
			.fetch_one(&pool)
			.await
			.expect("opds cascade count");
		let aidoku: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?"))
			.bind(aidoku_id)
			.fetch_one(&pool)
			.await
			.expect("aidoku row count");
		assert_eq!((opds, aidoku), (0, 1), "{table}.{column}");
	}
	let groups: Vec<i64> = sqlx::query_scalar("SELECT id FROM filter_group ORDER BY id")
		.fetch_all(&pool)
		.await
		.expect("filter groups");
	assert_eq!(groups, [700]);
	let rules: Vec<String> = sqlx::query_scalar("SELECT value FROM filter_group_rule WHERE group_id = 700 ORDER BY value")
		.fetch_all(&pool)
		.await
		.expect("filter rules");
	assert_eq!(rules, ["200"]);
	let states: Vec<i64> = sqlx::query_scalar("SELECT chapter_id FROM sync_chapter_state ORDER BY chapter_id")
		.fetch_all(&pool)
		.await
		.expect("chapter sync states");
	assert_eq!(states, [20000]);

	let preserved: Vec<(i64, String, String, bool)> = sqlx::query_as(
		"SELECT sequence, entity_kind, entity_key, deleted FROM sync_change WHERE sequence <= ? ORDER BY sequence",
	)
	.bind(before_max)
	.fetch_all(&pool)
	.await
	.expect("preserved sync history");
	assert_eq!(preserved, expected);
	let removed: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM sync_change WHERE entity_key IN ('1000', '1000:1', '10000', '10000:1', '500:1000:1')",
	)
	.fetch_one(&pool)
	.await
	.expect("removed sync history");
	assert_eq!(removed, 0);
	for table in ["sync_change", "sync_alias"] {
		let aidoku: i64 = sqlx::query_scalar(&format!(
			"SELECT COUNT(*) FROM {table} WHERE entity_key IN ('2000', '2000:1', '20000', '20000:1', '500:2000:1')",
		))
		.fetch_one(&pool)
		.await
		.expect("aidoku sync state");
		assert!(aidoku > 0, "{table}");
	}
	pool.close().await;
}

#[tokio::test]
async fn upgrade_rebaseline_reused_ids_and_cover_sweep_stay_safe() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
	let pool = connect(&url).await;
	apply(&pool, 1, 19).await;
	seed_sources(&pool).await;
	seed_content(&pool, 100, 1001, 10001).await;
	sqlx::query("DELETE FROM manga WHERE id = 1001")
		.execute(&pool)
		.await
		.expect("historical OPDS tombstone");
	sqlx::query("INSERT INTO manga (id, source_id, key, title) VALUES (1001, 200, 'aidoku-before-upgrade', 'Existing Aidoku')")
		.execute(&pool)
		.await
		.expect("Aidoku reuses the historical id before upgrade");
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (1001, 1, 1)")
		.execute(&pool)
		.await
		.expect("Aidoku library state");
	let (identity, snapshot): (String, String) = sqlx::query_as(
		"SELECT identity, snapshot FROM sync_change WHERE entity_kind = 'manga' AND entity_key = '1000' ORDER BY sequence DESC LIMIT 1",
	)
	.fetch_one(&pool)
	.await
	.expect("live OPDS change");
	let (historical_identity, historical_snapshot): (String, String) = sqlx::query_as(
		"SELECT identity, snapshot FROM sync_change WHERE entity_kind = 'manga' AND entity_key = '1001' AND deleted = 1 AND json_extract(identity, '$.sourceKey') = 'opds.catalog' ORDER BY sequence DESC LIMIT 1",
	)
	.fetch_one(&pool)
	.await
	.expect("historical OPDS change");
	let (aidoku_identity, aidoku_snapshot, aidoku_modified): (String, String, i64) = sqlx::query_as(
		"SELECT identity, snapshot, modified_at FROM sync_change WHERE entity_kind = 'manga' AND entity_key = '1001' AND deleted = 0 AND json_extract(identity, '$.sourceKey') = 'en.aidoku' ORDER BY sequence DESC LIMIT 1",
	)
	.fetch_one(&pool)
	.await
	.expect("existing Aidoku change");
	for (origin, origin_key, entity_key, deleted, modified_at, sequence, identity, snapshot) in [
		("peer-a", "remote-opds-manga", "1000", true, 9_000_000_000_000i64, 900, &identity, &snapshot),
		("peer-b", "remote-historical-opds", "1001", true, 9_000_000_000_001i64, 901, &historical_identity, &historical_snapshot),
		("peer-c", "remote-aidoku", "1001", false, aidoku_modified, 100, &aidoku_identity, &aidoku_snapshot),
	] {
		sqlx::query(
			"INSERT INTO sync_change
			 (user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot, origin, origin_user_id, origin_entity_key, origin_sequence)
			 VALUES (1, 'manga', ?, ?, ?, ?, ?, ?, 1, ?, ?)",
		)
		.bind(entity_key)
		.bind(deleted)
		.bind(modified_at)
		.bind(identity)
		.bind(snapshot)
		.bind(origin)
		.bind(origin_key)
		.bind(sequence)
		.execute(&pool)
		.await
		.expect("peer event");
	}
	sqlx::query(
		"INSERT INTO sync_alias (origin, origin_user_id, user_id, entity_kind, origin_entity_key, entity_key) VALUES
		 ('peer-a', 1, 1, 'manga', 'remote-opds-manga', '1000'),
		 ('peer-b', 1, 1, 'manga', 'remote-historical-opds', '1001'),
		 ('peer-c', 1, 1, 'manga', 'remote-aidoku', '1001')",
	)
	.execute(&pool)
	.await
	.expect("peer aliases");
	let covers = tmp.path().join("covers");
	std::fs::create_dir_all(&covers).expect("covers dir");
	for (id, bytes) in [("1000", b"old-opds".as_slice()), ("1001", b"historical-opds".as_slice())] {
		std::fs::write(covers.join(id), bytes).expect("old cover");
		std::fs::write(covers.join(format!("{id}.content-type")), "image/jpeg").expect("old cover sidecar");
	}
	std::fs::write(covers.join("2000"), b"live-aidoku").expect("live cover");
	std::fs::write(covers.join("2000.content-type"), "image/png").expect("live cover sidecar");
	std::fs::write(covers.join("keep"), b"keep").expect("nonnumeric cover");
	let mut config = base_config(&tmp);
	config.database_url = url;
	config.downloads_dir = tmp.path().join("downloads");
	let app = build_app(config).await.expect("upgraded app");

	for id in ["1000", "1001"] {
		assert!(!covers.join(id).exists());
		assert!(!covers.join(format!("{id}.content-type")).exists());
	}
	assert_eq!(std::fs::read(covers.join("2000")).expect("live cover"), b"live-aidoku");
	assert_eq!(std::fs::read(covers.join("2000.content-type")).expect("live sidecar"), b"image/png");
	assert_eq!(std::fs::read(covers.join("keep")).expect("nonnumeric cover"), b"keep");
	let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key LIKE 'migration.remove_opds_cover.%'")
		.fetch_one(&pool)
		.await
		.expect("cover markers");
	assert_eq!(markers, 0);
	let remaining_aliases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_alias WHERE origin IN ('peer-a', 'peer-b', 'peer-c')")
		.fetch_one(&pool)
		.await
		.expect("aliases without OPDS events remain harmless");
	assert_eq!(remaining_aliases, 3);
	let aidoku_alias: String = sqlx::query_scalar(
		"SELECT entity_key FROM sync_alias WHERE origin = 'peer-c' AND origin_entity_key = 'remote-aidoku'",
	)
	.fetch_one(&pool)
	.await
	.expect("Aidoku alias survives");
	assert_eq!(aidoku_alias, "1001");
	let retired: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.retired_sources'")
		.fetch_one(&pool)
		.await
		.expect("retired sources");
	assert_eq!(serde_json::from_str::<Value>(&retired).expect("retired source json"), json!(["opds.catalog"]));
	let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_change WHERE json_extract(identity, '$.sourceKey') = 'opds.catalog'")
		.fetch_one(&pool)
		.await
		.expect("OPDS events after rebaseline");
	assert_eq!(events, 0);

	sqlx::query("INSERT INTO manga (id, source_id, key, title) VALUES (1000, 200, 'aidoku-reused', 'New Aidoku')")
		.execute(&pool)
		.await
		.expect("new manga reuses the other id");
	let change = json!({
		"origin": "peer-a",
		"originUserId": 1,
		"originSequence": 900,
		"entityKind": "manga",
		"entityKey": "remote-opds-manga",
		"deleted": true,
		"modifiedAt": 9_999_999_999_999i64,
		"identity": serde_json::from_str::<Value>(&identity).expect("identity json"),
		"snapshot": serde_json::from_str::<Value>(&snapshot).expect("snapshot json")
	});
	let historical_change = json!({
		"origin": "peer-b",
		"originUserId": 1,
		"originSequence": 901,
		"entityKind": "manga",
		"entityKey": "remote-historical-opds",
		"deleted": true,
		"modifiedAt": 9_999_999_999_999i64,
		"identity": serde_json::from_str::<Value>(&historical_identity).expect("historical identity json"),
		"snapshot": serde_json::from_str::<Value>(&historical_snapshot).expect("historical snapshot json")
	});
	let mut rekey_identity = serde_json::from_str::<Value>(&aidoku_identity).expect("Aidoku identity json");
	let mut rekey_snapshot = serde_json::from_str::<Value>(&aidoku_snapshot).expect("Aidoku snapshot json");
	rekey_identity["mangaKey"] = json!("aidoku-rekeyed");
	rekey_snapshot["key"] = json!("aidoku-rekeyed");
	rekey_snapshot["lastModifiedAt"] = json!(9_999_999_999_999i64);
	let rekey = json!({
		"origin": "peer-c",
		"originUserId": 1,
		"originSequence": 101,
		"entityKind": "manga",
		"entityKey": "remote-aidoku",
		"deleted": false,
		"modifiedAt": 9_999_999_999_999i64,
		"identity": rekey_identity,
		"snapshot": rekey_snapshot
	});
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/sync/changes",
		Some(json!({"origin": "peer-a", "changes": [change, historical_change, rekey]})),
	)
	.await;
	assert_eq!(status, axum::http::StatusCode::OK, "{body}");
	let results = body["results"].as_array().expect("apply results");
	assert_eq!(results.len(), 3);
	for result in &results[..2] {
		assert_eq!(result["status"], "skipped");
		assert_eq!(result["reason"], "source_unavailable");
	}
	assert_eq!(results[2]["status"], "applied");
	let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM manga WHERE id IN (1000, 1001) ORDER BY id")
		.fetch_all(&pool)
		.await
		.expect("reused and rekeyed manga");
	assert_eq!(keys, ["aidoku-reused", "aidoku-rekeyed"]);
	let aidoku_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE title = 'Existing Aidoku'")
		.fetch_one(&pool)
		.await
		.expect("rekey did not create a stub");
	assert_eq!(aidoku_rows, 1);
	drop(app);
	pool.close().await;
}

#[tokio::test]
async fn removed_opds_routes_return_404_instead_of_the_spa_or_405() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	std::fs::create_dir_all(&config.webui_dir).expect("webui dir");
	std::fs::write(config.webui_dir.join("index.html"), "<html>spa</html>").expect("webui index");
	let app = build_app(config).await.expect("build app");

	for path in ["/opds/v1.2", "/opds/v2"] {
		let (status, body) = request(&app, "GET", path, None).await;
		assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{path}");
		assert_eq!(body["error"]["code"], "not_found");
	}
	use tower::ServiceExt;
	let request = axum::http::Request::builder()
		.method("POST")
		.uri("/api/v1/sources/opds")
		.header("content-type", "application/json")
		.body(axum::body::Body::from(
			serde_json::to_vec(&json!({"name": "Catalog", "url": "http://catalog.test/opds"}))
				.expect("request json"),
		))
		.expect("request");
	let response = app.clone().oneshot(request).await.expect("response");
	assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("response body");
	let body: Value = serde_json::from_slice(&bytes).expect("error json");
	assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn startup_removes_only_the_opds_cbz_cache() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let cache = tmp.path().join("opds-cbz");
	std::fs::create_dir_all(cache.join("nested")).expect("cache dir");
	std::fs::write(cache.join("nested/book.cbz"), b"cache").expect("cache file");
	let covers = tmp.path().join("covers");
	let downloads = tmp.path().join("downloads");
	std::fs::create_dir_all(&covers).expect("covers dir");
	std::fs::create_dir_all(&downloads).expect("downloads dir");
	std::fs::write(covers.join("keep"), b"cover").expect("cover sentinel");
	std::fs::write(downloads.join("keep"), b"download").expect("download sentinel");
	let mut config = base_config(&tmp);
	config.downloads_dir = downloads.clone();
	let app = build_app(config).await.expect("build app");

	assert!(!cache.exists());
	assert_eq!(std::fs::read(covers.join("keep")).expect("cover sentinel"), b"cover");
	assert_eq!(std::fs::read(downloads.join("keep")).expect("download sentinel"), b"download");
	drop(app);

	let fresh = tempfile::tempdir().expect("fresh tempdir");
	let mut config = base_config(&fresh);
	config.downloads_dir = fresh.path().join("downloads");
	let _app = build_app(config).await.expect("missing cache is ignored");
}

#[tokio::test]
async fn cover_cleanup_failure_keeps_the_migration_marker_and_aborts_startup() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let url = format!("sqlite://{}/migration.db?mode=rwc", tmp.path().display());
	let pool = connect(&url).await;
	apply(&pool, 1, 20).await;
	sqlx::query("INSERT INTO setting (key, value) VALUES ('migration.remove_opds_cover.1000', '')")
		.execute(&pool)
		.await
		.expect("cover marker");
	let covers = tmp.path().join("covers");
	std::fs::create_dir_all(covers.join("1000")).expect("cover path is a directory");
	let mut config = base_config(&tmp);
	config.database_url = url;
	config.downloads_dir = tmp.path().join("downloads");

	assert!(build_app(config).await.is_err());
	let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setting WHERE key = 'migration.remove_opds_cover.1000'")
		.fetch_one(&pool)
		.await
		.expect("marker remains for retry");
	assert_eq!(markers, 1);
	assert!(covers.join("1000").is_dir());
	pool.close().await;
}
