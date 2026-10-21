mod common;

use common::{connect_db, test_app};
use serde_json::json;
use sqlx::SqlitePool;
use sqlx::migrate::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use sumidoku_server::sync::{SyncChange, changes_since};

async fn high_watermark(pool: &SqlitePool) -> i64 {
	sqlx::query_scalar("SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)")
		.fetch_one(pool)
		.await
		.expect("high watermark")
}

async fn since(pool: &SqlitePool, user_id: i64, cursor: i64) -> Vec<SyncChange> {
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
		if event.deleted {
			assert!(event.modified_at > snapshot_time, "stale tombstone: {event:?}");
		} else {
			assert_eq!(event.modified_at, snapshot_time, "non-delete time mismatch: {event:?}");
		}
	}
}

async fn assert_local_origins(pool: &SqlitePool, events: &[SyncChange]) {
	let instance: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(pool)
		.await
		.expect("instance id");
	for event in events {
		assert_eq!(event.origin, instance);
		assert_eq!(event.origin_user_id, event.user_id);
		assert_eq!(event.origin_entity_key, event.entity_key);
		assert_eq!(event.origin_sequence, event.sequence);
	}
}

async fn add_user(pool: &SqlitePool, id: i64) {
	sqlx::query("INSERT INTO user (id, username, role) VALUES (?, ?, 'reader')")
		.bind(id)
		.bind(format!("user{id}"))
		.execute(pool)
		.await
		.expect("add user");
}

async fn local_source_id(pool: &SqlitePool) -> i64 {
	sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
		.fetch_one(pool)
		.await
		.expect("local source")
}

async fn add_manga(pool: &SqlitePool, source_id: i64, modified_at: i64) -> i64 {
	sqlx::query_scalar(
		"INSERT INTO manga (source_id, key, title, last_modified_at) VALUES (?, 'manga', 'Manga', ?) RETURNING id",
	)
	.bind(source_id)
	.bind(modified_at)
	.fetch_one(pool)
	.await
	.expect("add manga")
}

async fn add_library(pool: &SqlitePool, manga_id: i64, user_id: i64, modified_at: i64) {
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, ?, 1, ?)")
		.bind(manga_id)
		.bind(user_id)
		.bind(modified_at)
		.execute(pool)
		.await
		.expect("add library state");
}

async fn add_chapter(pool: &SqlitePool, manga_id: i64, key: &str, modified_at: i64) -> i64 {
	sqlx::query_scalar(
		"INSERT INTO chapter (manga_id, key, title, last_modified_at) VALUES (?, ?, 'Chapter', ?) RETURNING id",
	)
	.bind(manga_id)
	.bind(key)
	.bind(modified_at)
	.fetch_one(pool)
	.await
	.expect("add chapter")
}

async fn add_progress(pool: &SqlitePool, chapter_id: i64, user_id: i64, modified_at: i64) {
	sqlx::query(
		"INSERT INTO chapter_user
		 (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at, last_modified_at)
		 VALUES (?, ?, 3, 1, 1, 50, ?)",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(modified_at)
	.execute(pool)
	.await
	.expect("add progress");
}

async fn chapter_modified(pool: &SqlitePool, chapter_id: i64) -> i64 {
	sqlx::query_scalar("SELECT last_modified_at FROM chapter WHERE id = ?")
		.bind(chapter_id)
		.fetch_one(pool)
		.await
		.expect("chapter modified")
}

#[tokio::test]
async fn migration_18_replays_real_history_with_origins_and_parent_baselines() {
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
	for migration in migrator.iter().filter(|migration| migration.version < 18) {
		conn.apply(migration).await.expect("apply pre-0018 migration");
	}
	drop(conn);

	add_user(&pool, 2).await;
	sqlx::query("INSERT INTO source (id, key, name, wasm_path) VALUES (10, 'test.source', 'Test', '')")
		.execute(&pool)
		.await
		.expect("add old source");
	sqlx::query(
		"INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (100, 10, 'shared', 'Shared', 100)",
	)
	.execute(&pool)
	.await
	.expect("add old manga");
	add_library(&pool, 100, 1, 101).await;
	sqlx::query(
		"INSERT INTO chapter (id, manga_id, key, title, fetched_at) VALUES
		 (200, 100, 'one', 'One', 777), (201, 100, 'two', 'Two', 888)",
	)
	.execute(&pool)
	.await
	.expect("add old chapters");
	sqlx::query(
		"INSERT INTO chapter_user
		 (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at, is_downloaded) VALUES
		 (200, 1, 3, 1, 0, 123, 1), (200, 2, 5, 1, 1, 456, 0)",
	)
	.execute(&pool)
	.await
	.expect("add old progress");
	let old_rows: Vec<(i64, i64, String)> =
		sqlx::query_as("SELECT sequence, user_id, entity_key FROM sync_change ORDER BY sequence")
			.fetch_all(&pool)
			.await
			.expect("old change rows");
	assert_eq!(old_rows.len(), 2);
	let old_high_watermark = high_watermark(&pool).await;
	pool.close().await;

	let pool = sumidoku_server::db::create_pool(&url).await.expect("apply 0018");
	let instance: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("instance id");
	assert_eq!(instance.len(), 32);
	assert!(instance.bytes().all(|byte| byte.is_ascii_hexdigit()));
	for (sequence, user_id, entity_key) in old_rows {
		let origin: (String, i64, String, i64) = sqlx::query_as(
			"SELECT origin, origin_user_id, origin_entity_key, origin_sequence FROM sync_change WHERE sequence = ?",
		)
		.bind(sequence)
		.fetch_one(&pool)
		.await
		.expect("backfilled origin");
		assert_eq!(origin, (instance.clone(), user_id, entity_key, sequence));
	}
	let invalid_origins: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM sync_change
		 WHERE origin <> ? OR origin_user_id <> user_id OR origin_entity_key <> entity_key OR origin_sequence <> sequence",
	)
	.bind(&instance)
	.fetch_one(&pool)
	.await
	.expect("origin consistency");
	assert_eq!(invalid_origins, 0);
	for table in ["chapter", "chapter_user"] {
		let versions: Vec<i64> = sqlx::query_scalar(&format!("SELECT last_modified_at FROM {table}"))
			.fetch_all(&pool)
			.await
			.expect("zero baseline versions");
		assert!(versions.iter().all(|version| *version == 0), "{table}: {versions:?}");
	}

	let admin = since(&pool, 1, old_high_watermark).await;
	assert_eq!(kinds(&admin), [("chapter", false), ("chapter", false), ("chapter_user", false)]);
	assert!(admin.iter().all(|event| event.modified_at == 0));
	let history_only = since(&pool, 2, old_high_watermark).await;
	assert_eq!(kinds(&history_only), [("manga", false), ("chapter", false), ("chapter_user", false)]);
	assert_eq!(history_only[0].modified_at, 100);
	assert_eq!(history_only[1].modified_at, 0);
	assert_eq!(history_only[2].modified_at, 0);
	assert_eq!(history_only[2].snapshot["lastPageRead"], 5);
	assert_eq!(history_only[2].snapshot["lastReadAt"], 456);
	assert!(history_only[2].snapshot.get("isDownloaded").is_none());
	assert!(admin[0].snapshot.get("fetchedAt").is_none());
	assert_time_contract(&admin);
	assert_time_contract(&history_only);
	assert_local_origins(&pool, &admin).await;
	assert_local_origins(&pool, &history_only).await;
	let membership: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = 100 AND user_id = 2")
		.fetch_one(&pool)
		.await
		.expect("history-only membership");
	assert_eq!(membership, 0);
	assert!(!history_only.iter().any(|event| event.entity_kind == "manga_user"));

	let states: Vec<(i64, String, String, String, String)> = sqlx::query_as(
		"SELECT chapter_id, source_key, manga_key, chapter_key, snapshot FROM sync_chapter_state ORDER BY chapter_id",
	)
	.fetch_all(&pool)
	.await
	.expect("chapter states");
	assert_eq!(states.len(), 2);
	assert_eq!(states[0].0, 200);
	assert_eq!((states[0].1.as_str(), states[0].2.as_str(), states[0].3.as_str()), ("test.source", "shared", "one"));
	let snapshot: serde_json::Value = serde_json::from_str(&states[0].4).expect("state snapshot");
	assert_eq!(snapshot["lastModifiedAt"], 0);
	assert!(snapshot.get("fetchedAt").is_none());
	let checkpoint: (i64, i64) = sqlx::query_as("SELECT id, base_sequence FROM sync_checkpoint")
		.fetch_one(&pool)
		.await
		.expect("checkpoint");
	assert_eq!(checkpoint, (1, 0));
	let origin_states: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_origin_state")
		.fetch_one(&pool)
		.await
		.expect("origin states");
	assert_eq!(origin_states, 0);
	let change_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_change")
		.fetch_one(&pool)
		.await
		.expect("change count");
	pool.close().await;

	let pool = sumidoku_server::db::create_pool(&url).await.expect("restart migrated database");
	let restarted: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("restarted instance id");
	assert_eq!(restarted, instance);
	let restarted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_change")
		.fetch_one(&pool)
		.await
		.expect("restarted change count");
	assert_eq!(restarted_count, change_count);
	pool.close().await;
}

#[tokio::test]
async fn local_origin_fill_preserves_remote_metadata_and_mapped_entity_key() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, 100).await;
	add_library(&pool, manga_id, 1, 101).await;
	let local = since(&pool, 1, 0).await;
	assert_local_origins(&pool, &local).await;

	sqlx::query(
		"INSERT INTO sync_change
		 (user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key, origin_sequence,
		  deleted, modified_at, identity, snapshot)
		 VALUES (1, 'manga', 'local-mapped', 'peer-instance', 9, 'remote-key', 77, 0, 0, '{}', '{\"lastModifiedAt\":0}')",
	)
	.execute(&pool)
	.await
	.expect("insert remote change");
	let remote = since(&pool, 1, 0)
		.await
		.into_iter()
		.find(|event| event.origin == "peer-instance")
		.expect("remote change");
	assert_eq!(remote.entity_key, "local-mapped");
	assert_eq!(remote.origin_user_id, 9);
	assert_eq!(remote.origin_entity_key, "remote-key");
	assert_eq!(remote.origin_sequence, 77);
}

#[tokio::test]
async fn chapter_content_noop_rekey_and_recursive_updates_follow_the_time_contract() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, 100).await;
	add_library(&pool, manga_id, 1, 101).await;
	let chapter_id = add_chapter(&pool, manga_id, "one", 1_000).await;
	let cursor = high_watermark(&pool).await;

	let mut conn = pool.acquire().await.expect("connection");
	sqlx::query("PRAGMA recursive_triggers = ON")
		.execute(&mut *conn)
		.await
		.expect("enable recursive triggers");
	sqlx::query(
		"INSERT INTO chapter (manga_id, key, title, fetched_at) VALUES (?, 'one', 'Chapter', 999)
		 ON CONFLICT (manga_id, key) DO UPDATE SET title = excluded.title, fetched_at = excluded.fetched_at",
	)
	.bind(manga_id)
	.execute(&mut *conn)
	.await
	.expect("quiet chapter upsert");
	assert!(since(&pool, 1, cursor).await.is_empty());
	assert_eq!(chapter_modified(&pool, chapter_id).await, 1_000);

	sqlx::query(
		"UPDATE chapter SET key = 'renamed', title = 'Fresh', number = 2.5, volume = 3.5,
		 scanlators = '[\"scan\"]', language = 'en', thumbnail = 'thumb', url = 'url',
		 date_uploaded = 123, source_order = 4, page_count = 20, locked = 1, last_modified_at = 1001
		 WHERE id = ?",
	)
	.bind(chapter_id)
	.execute(&mut *conn)
	.await
	.expect("chapter content update");
	let events = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&events), [("manga", false), ("chapter", false)]);
	assert_time_contract(&events);
	assert_local_origins(&pool, &events).await;
	let chapter = &events[1];
	assert_eq!(chapter.entity_key, chapter_id.to_string());
	assert_eq!(chapter.identity["chapterKey"], "renamed");
	assert_eq!(chapter.identity["mangaKey"], "manga");
	assert_eq!(chapter.snapshot["title"], "Fresh");
	assert_eq!(chapter.snapshot["number"], 2.5);
	assert_eq!(chapter.snapshot["volume"], 3.5);
	assert_eq!(chapter.snapshot["scanlators"], json!(["scan"]));
	assert_eq!(chapter.snapshot["language"], "en");
	assert_eq!(chapter.snapshot["thumbnail"], "thumb");
	assert_eq!(chapter.snapshot["url"], "url");
	assert_eq!(chapter.snapshot["dateUploaded"], 123);
	assert_eq!(chapter.snapshot["sourceOrder"], 4);
	assert_eq!(chapter.snapshot["pageCount"], 20);
	assert_eq!(chapter.snapshot["locked"], 1);
	assert!(chapter.snapshot.get("fetchedAt").is_none());
	let state_key: String = sqlx::query_scalar("SELECT chapter_key FROM sync_chapter_state WHERE chapter_id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("rekeyed state");
	assert_eq!(state_key, "renamed");

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE chapter SET last_modified_at = 1002 WHERE id = ?")
		.bind(chapter_id)
		.execute(&mut *conn)
		.await
		.expect("timestamp-only update");
	sqlx::query("UPDATE chapter SET fetched_at = 1003 WHERE id = ?")
		.bind(chapter_id)
		.execute(&mut *conn)
		.await
		.expect("fetch-only update");
	assert!(since(&pool, 1, cursor).await.is_empty());
	let state_version: i64 = sqlx::query_scalar(
		"SELECT CAST(json_extract(snapshot, '$.lastModifiedAt') AS INTEGER) FROM sync_chapter_state WHERE chapter_id = ?",
	)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("state version");
	assert_eq!(state_version, 1_002);
}

#[tokio::test]
async fn progress_changes_emit_parents_without_library_membership_and_download_stays_quiet() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	add_user(&pool, 2).await;
	let source_id = local_source_id(&pool).await;
	let manga_id = add_manga(&pool, source_id, 100).await;
	let chapter_id = add_chapter(&pool, manga_id, "history", 1_000).await;
	let cursor = high_watermark(&pool).await;
	add_progress(&pool, chapter_id, 2, 2_000).await;
	let events = since(&pool, 2, cursor).await;
	assert_eq!(kinds(&events), [("manga", false), ("chapter", false), ("chapter_user", false)]);
	assert_time_contract(&events);
	assert_eq!(events[0].modified_at, 100);
	assert_eq!(events[1].modified_at, 1_000);
	assert_eq!(events[2].modified_at, 2_000);
	assert_eq!(events[2].snapshot["lastPageRead"], 3);
	assert_eq!(events[2].snapshot["isRead"], 1);
	assert_eq!(events[2].snapshot["isBookmarked"], 1);
	assert_eq!(events[2].snapshot["lastReadAt"], 50);
	assert!(events[2].snapshot.get("isDownloaded").is_none());
	let membership: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = ? AND user_id = 2")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("membership count");
	assert_eq!(membership, 0);

	let cursor = high_watermark(&pool).await;
	let mut conn = pool.acquire().await.expect("connection");
	sqlx::query("PRAGMA recursive_triggers = ON")
		.execute(&mut *conn)
		.await
		.expect("enable recursive triggers");
	sqlx::query(
		"INSERT INTO chapter_user
		 (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at, is_downloaded)
		 VALUES (?, 2, 3, 1, 1, 50, 0)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
		 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at,
		 is_downloaded = excluded.is_downloaded",
	)
	.bind(chapter_id)
	.execute(&mut *conn)
	.await
	.expect("quiet progress upsert");
	assert!(since(&pool, 2, cursor).await.is_empty());
	let progress_time: i64 = sqlx::query_scalar(
		"SELECT last_modified_at FROM chapter_user WHERE chapter_id = ? AND user_id = 2",
	)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("progress timestamp");
	assert_eq!(progress_time, 2_000);

	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 8, is_read = 0, is_bookmarked = 0,
		 last_read_at = 80, is_downloaded = 1, last_modified_at = 2001
		 WHERE chapter_id = ? AND user_id = 2",
	)
	.bind(chapter_id)
	.execute(&mut *conn)
	.await
	.expect("actual progress update");
	let events = since(&pool, 2, cursor).await;
	assert_eq!(kinds(&events), [("manga", false), ("chapter", false), ("chapter_user", false)]);
	assert_eq!(events.len(), 3);
	assert_time_contract(&events);
	assert_eq!(events[2].snapshot["lastPageRead"], 8);
	assert_eq!(events[2].snapshot["isRead"], 0);
	assert_eq!(events[2].snapshot["isBookmarked"], 0);
	assert_eq!(events[2].snapshot["lastReadAt"], 80);
	assert!(events[2].snapshot.get("isDownloaded").is_none());

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE chapter_user SET is_downloaded = 0 WHERE chapter_id = ? AND user_id = 2")
		.bind(chapter_id)
		.execute(&mut *conn)
		.await
		.expect("download-only update");
	sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, 1, 1)")
		.bind(chapter_id)
		.execute(&mut *conn)
		.await
		.expect("download-only insert");
	assert!(since(&pool, 1, cursor).await.is_empty());
	assert!(since(&pool, 2, cursor).await.is_empty());
	let download_insert_time: i64 = sqlx::query_scalar(
		"SELECT last_modified_at FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("download insert timestamp");
	assert_eq!(download_insert_time, 0);

	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM chapter_user WHERE chapter_id = ? AND user_id = 2")
		.bind(chapter_id)
		.execute(&mut *conn)
		.await
		.expect("delete progress");
	let events = since(&pool, 2, cursor).await;
	assert_eq!(kinds(&events), [("manga", false), ("chapter", false), ("chapter_user", true)]);
	assert_time_contract(&events);
	assert_eq!(events[2].snapshot["lastModifiedAt"], 2_001);
	assert_eq!(chapter_modified(&pool, chapter_id).await, 1_000);
}

#[tokio::test]
async fn chapter_and_progress_writes_use_history_for_monotonic_versions() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let source_id = local_source_id(&pool).await;
	let future = 9_000_000_000_000_000_i64;
	let manga_id = add_manga(&pool, source_id, future).await;
	add_library(&pool, manga_id, 1, future + 1).await;
	let source = add_chapter(&pool, manga_id, "source", future + 10).await;
	let target = add_chapter(&pool, manga_id, "target", future + 30).await;
	add_progress(&pool, source, 1, future + 20).await;
	add_progress(&pool, target, 1, future + 100).await;

	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM chapter_user WHERE chapter_id = ? AND user_id = 1")
		.bind(target)
		.execute(&pool)
		.await
		.expect("delete target progress");
	let deleted = since(&pool, 1, cursor).await;
	let target_tombstone = deleted.iter().find(|event| event.entity_kind == "chapter_user").expect("target tombstone");
	assert_eq!(target_tombstone.modified_at, future + 101);
	assert_eq!(target_tombstone.snapshot["lastModifiedAt"], future + 100);

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE chapter_user SET chapter_id = ? WHERE chapter_id = ? AND user_id = 1")
		.bind(target)
		.bind(source)
		.execute(&pool)
		.await
		.expect("move progress to deleted key");
	let events = since(&pool, 1, cursor).await;
	assert_eq!(
		kinds(&events),
		[
			("manga", false),
			("chapter", false),
			("chapter_user", true),
			("manga", false),
			("chapter", false),
			("chapter_user", false),
		]
	);
	assert_time_contract(&events);
	assert_eq!(events[2].entity_key, format!("{source}:1"));
	assert_eq!(events[5].entity_key, format!("{target}:1"));
	assert_eq!(events[2].modified_at, future + 102);
	assert_eq!(events[2].snapshot["lastModifiedAt"], future + 20);
	assert_eq!(events[5].modified_at, future + 102);
	assert_eq!(events[5].snapshot["lastModifiedAt"], future + 102);

	let cursor = high_watermark(&pool).await;
	sqlx::query("UPDATE chapter SET title = 'Changed' WHERE id = ?")
		.bind(target)
		.execute(&pool)
		.await
		.expect("ordinary chapter update");
	let events = since(&pool, 1, cursor).await;
	assert_eq!(kinds(&events), [("manga", false), ("chapter", false)]);
	assert_eq!(events[1].modified_at, future + 31);

	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM chapter WHERE id = ?")
		.bind(target)
		.execute(&pool)
		.await
		.expect("delete future chapter");
	let events = since(&pool, 1, cursor).await;
	let chapter_tombstone = events.iter().find(|event| event.entity_kind == "chapter").expect("chapter tombstone");
	assert_eq!(chapter_tombstone.modified_at, future + 32);
	assert_eq!(chapter_tombstone.snapshot["lastModifiedAt"], future + 31);

	sqlx::query("INSERT INTO chapter (id, manga_id, key, title, last_modified_at) VALUES (?, ?, 'target', 'Recreated', 0)")
		.bind(target)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("recreate chapter");
	assert_eq!(chapter_modified(&pool, target).await, future + 33);
}

async fn seed_cascade(pool: &SqlitePool) -> (i64, i64, i64) {
	add_user(pool, 2).await;
	let source_id = local_source_id(pool).await;
	let manga_id = add_manga(pool, source_id, 100).await;
	add_library(pool, manga_id, 1, 101).await;
	let chapter_id = add_chapter(pool, manga_id, "chapter", 1_000).await;
	add_progress(pool, chapter_id, 1, 2_000).await;
	add_progress(pool, chapter_id, 2, 2_001).await;
	(source_id, manga_id, chapter_id)
}

fn assert_cascade_identities(events: &[SyncChange], chapter_id: i64) {
	for event in events {
		assert_eq!(event.identity["sourceKey"], "local.local");
		assert_eq!(event.identity["mangaKey"], "manga");
		if matches!(event.entity_kind.as_str(), "chapter" | "chapter_user") {
			assert_eq!(event.identity["chapterKey"], "chapter");
			assert_eq!(event.identity["chapterId"], chapter_id);
		}
	}
}

#[tokio::test]
async fn chapter_delete_captures_progress_once_before_cascade() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let (_, manga_id, chapter_id) = seed_cascade(&pool).await;
	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM chapter WHERE id = ?")
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("delete chapter");
	for user_id in [1, 2] {
		let events = since(&pool, user_id, cursor).await;
		assert_eq!(kinds(&events), [("manga", false), ("chapter", true), ("chapter_user", true)]);
		assert_time_contract(&events);
		assert_cascade_identities(&events, chapter_id);
	}
	let manga_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("manga survives");
	assert_eq!(manga_exists, 1);
	let states: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_chapter_state WHERE chapter_id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("state removed");
	assert_eq!(states, 0);
}

#[tokio::test]
async fn manga_delete_captures_chapter_and_progress_before_parent_disappears() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let (_, manga_id, chapter_id) = seed_cascade(&pool).await;
	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM manga WHERE id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("delete manga");
	let admin = since(&pool, 1, cursor).await;
	assert_eq!(
		kinds(&admin),
		[("manga", true), ("chapter", true), ("chapter_user", true), ("manga_user", true)]
	);
	let history_only = since(&pool, 2, cursor).await;
	assert_eq!(kinds(&history_only), [("manga", true), ("chapter", true), ("chapter_user", true)]);
	for events in [&admin, &history_only] {
		assert_time_contract(events);
		assert_cascade_identities(events, chapter_id);
		assert_local_origins(&pool, events).await;
	}
}

#[tokio::test]
async fn source_delete_captures_chapter_and_progress_before_parent_disappears() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let (source_id, _, chapter_id) = seed_cascade(&pool).await;
	let cursor = high_watermark(&pool).await;
	sqlx::query("DELETE FROM source WHERE id = ?")
		.bind(source_id)
		.execute(&pool)
		.await
		.expect("delete source");
	let admin = since(&pool, 1, cursor).await;
	assert_eq!(
		kinds(&admin),
		[("manga", true), ("chapter", true), ("chapter_user", true), ("manga_user", true)]
	);
	let history_only = since(&pool, 2, cursor).await;
	assert_eq!(kinds(&history_only), [("manga", true), ("chapter", true), ("chapter_user", true)]);
	for events in [&admin, &history_only] {
		assert_time_contract(events);
		assert_cascade_identities(events, chapter_id);
	}
}
