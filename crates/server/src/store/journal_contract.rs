//! Pins the `sync_change` rows each store operation produces. Peers depend on this output, so when
//! journal writes move out of the SQL triggers this file must keep passing unchanged. Regenerate the
//! fixture only for an intended protocol change: `UPDATE_SYNC_JOURNAL=1 cargo nextest run journal_contract`.

use std::path::Path;

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::chapter::ChapterFields;
use super::library::LibraryEntry;
use super::manga::{FetchTimes, MangaFields};
use super::{category, chapter, library, manga, progress};
use crate::model::ChapterProgress;

const USER: i64 = 1;

macro_rules! conn {
	($pool:expr) => {
		$pool.acquire().await.expect("connection")
	};
}

/// Times come from the clock, so only their order is pinned, by `assert_versions_advance`
fn mask_times(value: &mut Value) {
	match value {
		Value::Object(map) => {
			for (key, item) in map.iter_mut() {
				if key == "lastModifiedAt" && item.is_number() {
					*item = json!("<time>");
				} else {
					mask_times(item);
				}
			}
		}
		Value::Array(items) => items.iter_mut().for_each(mask_times),
		_ => {}
	}
}

#[derive(sqlx::FromRow)]
struct JournalRow {
	sequence: i64,
	user_id: i64,
	entity_kind: String,
	entity_key: String,
	deleted: bool,
	modified_at: i64,
	identity: String,
	snapshot: String,
	origin: String,
	origin_user_id: i64,
	origin_entity_key: String,
	origin_sequence: i64,
}

async fn rows_after(pool: &SqlitePool, sequence: i64) -> Vec<JournalRow> {
	sqlx::query_as(
		"SELECT sequence, user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot,
		 origin, origin_user_id, origin_entity_key, origin_sequence
		 FROM sync_change WHERE sequence > ? ORDER BY sequence",
	)
	.bind(sequence)
	.fetch_all(pool)
	.await
	.expect("journal rows")
}

/// Each entity's versions only move forward, which is what peers order events by
fn assert_versions_advance(rows: &[JournalRow], step: &str) {
	let mut seen = std::collections::HashMap::new();
	for row in rows {
		let key = (row.entity_kind.clone(), row.entity_key.clone());
		if let Some(previous) = seen.insert(key, row.modified_at) {
			assert!(row.modified_at >= previous, "{step}: {} {} went back", row.entity_kind, row.entity_key);
		}
	}
}

struct Recorder {
	pool: SqlitePool,
	instance: String,
	sequence: i64,
	steps: Vec<Value>,
}

impl Recorder {
	async fn step(&mut self, name: &str) {
		let rows = rows_after(&self.pool, self.sequence).await;
		assert_versions_advance(&rows, name);
		let mut events = Vec::new();
		for row in &rows {
			let mut identity: Value = serde_json::from_str(&row.identity).expect("identity json");
			let mut snapshot: Value = serde_json::from_str(&row.snapshot).expect("snapshot json");
			mask_times(&mut identity);
			mask_times(&mut snapshot);
			assert_eq!(row.origin, self.instance, "{name}: local origin");
			assert_eq!(row.origin_sequence, row.sequence, "{name}: local origin sequence");
			assert_eq!(row.origin_entity_key, row.entity_key, "{name}: local origin key");
			assert_eq!(row.origin_user_id, row.user_id, "{name}: local origin user");
			events.push(json!({
				"userId": row.user_id,
				"kind": row.entity_kind,
				"key": row.entity_key,
				"deleted": row.deleted,
				"identity": identity,
				"snapshot": snapshot,
			}));
		}
		self.sequence = rows.last().map_or(self.sequence, |row| row.sequence);
		self.steps.push(json!({ "step": name, "events": events }));
	}
}

fn manga_fields<'a>(title: &'a str) -> MangaFields<'a> {
	MangaFields {
		title,
		authors: r#"["Author"]"#,
		artists: "[]",
		description: Some("Description"),
		tags: r#"["Tag"]"#,
		status: 1,
		content_rating: 0,
		viewer: 0,
		cover_url: None,
		url: Some("https://example.invalid/m"),
		initialized: true,
		update_strategy: 0,
	}
}

fn chapter_fields<'a>(key: &'a str, order: i64) -> ChapterFields<'a> {
	ChapterFields {
		key,
		title: Some("Chapter"),
		number: Some(order as f64),
		volume: None,
		scanlators: "[]",
		language: Some("en"),
		thumbnail: None,
		url: None,
		date_uploaded: Some(1000),
		source_order: order,
		page_count: 3,
		locked: false,
	}
}

async fn insert_manga(pool: &SqlitePool, source_id: i64, key: &str) -> i64 {
	let times = FetchTimes { next_update_time: None, last_fetched_at: None, chapters_last_fetched_at: None, now: 1000 };
	manga::upsert_from_source(&mut conn!(pool), source_id, key, &manga_fields("Manga"), &times).await.expect("upsert manga");
	sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_one(pool)
		.await
		.expect("manga id")
}

async fn insert_chapter(pool: &SqlitePool, manga_id: i64, key: &str, order: i64) -> i64 {
	chapter::upsert_from_source(&mut conn!(pool), manga_id, &chapter_fields(key, order), 1000).await.expect("upsert chapter");
	sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(key)
		.fetch_one(pool)
		.await
		.expect("chapter id")
}

fn progress_at(page: i64) -> ChapterProgress {
	ChapterProgress { last_page_read: page, is_read: false, is_bookmarked: false, last_read_at: Some(2000) }
}

async fn record_operations(pool: SqlitePool) -> Vec<Value> {
	let instance = crate::sync::instance_id(&pool).await.expect("instance id");
	let sequence = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM sync_change")
		.fetch_one(&pool)
		.await
		.expect("start sequence");
	let mut log = Recorder { pool: pool.clone(), instance, sequence, steps: Vec::new() };
	let source: i64 = sqlx::query_scalar("INSERT INTO source (key, name, wasm_path) VALUES ('test.contract', 'Test', '') RETURNING id")
		.fetch_one(&pool)
		.await
		.expect("source");
	log.step("source inserted").await;

	let manga_id = insert_manga(&pool, source, "m1").await;
	log.step("manga inserted, not in library").await;
	library::shelve(&mut conn!(pool), manga_id, USER, 1000).await.expect("shelve");
	log.step("manga shelved").await;
	let times = FetchTimes { next_update_time: None, last_fetched_at: None, chapters_last_fetched_at: None, now: 1000 };
	manga::upsert_from_source(&mut conn!(pool), source, "m1", &manga_fields("Renamed"), &times).await.expect("upsert");
	log.step("manga refreshed with a change").await;
	manga::upsert_from_source(&mut conn!(pool), source, "m1", &manga_fields("Renamed"), &times).await.expect("upsert");
	log.step("manga refreshed unchanged").await;
	manga::set_chapters_fetched_at(&mut conn!(pool), manga_id, 3000).await.expect("fetch time");
	log.step("manga fetch time stamped").await;
	manga::set_cover_url(&mut conn!(pool), manga_id, "https://example.invalid/c").await.expect("cover");
	log.step("manga cover changed").await;

	let first = insert_chapter(&pool, manga_id, "c1", 0).await;
	log.step("chapter inserted").await;
	let second = insert_chapter(&pool, manga_id, "c2", 1).await;
	let third = insert_chapter(&pool, manga_id, "c3", 2).await;
	log.step("two chapters inserted").await;
	chapter::rekey(&mut conn!(pool), third, "c3b").await.expect("rekey");
	log.step("chapter rekeyed").await;
	chapter::set_page_info(&mut conn!(pool), first, 9, None).await.expect("page info");
	log.step("chapter page count changed").await;

	progress::set(&mut conn!(pool), first, USER, &progress_at(1)).await.expect("progress");
	log.step("progress set").await;
	progress::merge(&mut conn!(pool), first, USER, &progress_at(5)).await.expect("merge");
	log.step("progress merged forward").await;
	progress::merge(&mut conn!(pool), first, USER, &progress_at(2)).await.expect("merge");
	log.step("progress merged backward").await;
	progress::set(&mut conn!(pool), second, USER, &progress_at(3)).await.expect("progress");
	log.step("second chapter progress").await;
	progress::set_downloaded(&mut conn!(pool), first, USER, true).await.expect("downloaded");
	log.step("chapter marked downloaded").await;
	progress::reset_manga(&mut conn!(pool), manga_id, USER).await.expect("reset");
	log.step("bulk progress reset").await;

	let first_category = category::create(&mut conn!(pool), USER, "First", 0, false).await.expect("category");
	log.step("category created").await;
	let second_category = category::create(&mut conn!(pool), USER, "Second", 1, false).await.expect("category");
	log.step("second category created").await;
	category::rename(&mut conn!(pool), first_category, USER, "Renamed").await.expect("rename");
	log.step("category renamed").await;
	category::set_sort_order(&mut conn!(pool), second_category, USER, 5).await.expect("order");
	log.step("category reordered").await;

	let mut tx = pool.begin().await.expect("transaction");
	category::set_manga_categories(&mut tx, manga_id, USER, &[first_category, second_category]).await.expect("set");
	tx.commit().await.expect("commit");
	log.step("membership set to two").await;
	let mut tx = pool.begin().await.expect("transaction");
	category::set_manga_categories(&mut tx, manga_id, USER, &[second_category]).await.expect("set");
	tx.commit().await.expect("commit");
	log.step("membership reduced to one").await;
	category::delete_owned(&mut conn!(pool), second_category, USER).await.expect("delete category");
	log.step("category with a member deleted").await;

	library::set_incognito(&mut conn!(pool), manga_id, USER, true).await.expect("incognito");
	log.step("manga incognito set").await;
	let entry = LibraryEntry {
		manga_id,
		user_id: USER,
		in_library: true,
		in_library_at: Some(1000),
		viewer_flags: 2,
		incognito: false,
		sort_key: "key",
	};
	library::upsert(&mut conn!(pool), &entry).await.expect("upsert entry");
	log.step("library row replaced").await;
	library::unshelve(&mut conn!(pool), manga_id, USER).await.expect("unshelve");
	log.step("manga unshelved").await;
	library::shelve(&mut conn!(pool), manga_id, USER, 1000).await.expect("shelve");
	log.step("manga reshelved").await;

	chapter::delete(&mut conn!(pool), second).await.expect("delete chapter");
	log.step("chapter with progress deleted").await;

	let other = insert_manga(&pool, source, "m2").await;
	library::shelve(&mut conn!(pool), other, USER, 1000).await.expect("shelve");
	let other_chapter = insert_chapter(&pool, other, "c1", 0).await;
	progress::set(&mut conn!(pool), other_chapter, USER, &progress_at(1)).await.expect("progress");
	category::add_manga(&mut conn!(pool), first_category, other, USER).await.expect("add");
	log.step("second manga set up").await;
	manga::delete(&mut conn!(pool), other).await.expect("delete manga");
	log.step("manga cascade delete").await;

	super::source::delete(&mut conn!(pool), source).await.expect("delete source");
	log.step("source cascade delete").await;
	log.steps
}

/// Writes inside one millisecond still get distinct versions, or peers could not order them
#[tokio::test]
async fn rapid_writes_get_strictly_increasing_versions() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let pool = crate::db::create_pool(&format!("sqlite://{}/rapid.db?mode=rwc", tmp.path().display()))
		.await
		.expect("pool");
	let source: i64 = sqlx::query_scalar("INSERT INTO source (key, name, wasm_path) VALUES ('test.rapid', 'Test', '') RETURNING id")
		.fetch_one(&pool)
		.await
		.expect("source");
	let manga_id = insert_manga(&pool, source, "m1").await;
	library::shelve(&mut conn!(pool), manga_id, USER, 1000).await.expect("shelve");
	let chapter_id = insert_chapter(&pool, manga_id, "c1", 0).await;
	let category_id = category::create(&mut conn!(pool), USER, "Start", 0, false).await.expect("category");
	let start: i64 = sqlx::query_scalar("SELECT MAX(sequence) FROM sync_change").fetch_one(&pool).await.expect("sequence");

	let mut tx = pool.begin().await.expect("transaction");
	for round in 0..20_i64 {
		category::rename(&mut tx, category_id, USER, &format!("Name {round}")).await.expect("rename");
		manga::set_cover_url(&mut tx, manga_id, &format!("https://example.invalid/{round}")).await.expect("cover");
		chapter::set_page_info(&mut tx, chapter_id, round + 10, None).await.expect("pages");
		progress::set(&mut tx, chapter_id, USER, &progress_at(round + 1)).await.expect("progress");
		library::set_incognito(&mut tx, manga_id, USER, round % 2 == 0).await.expect("incognito");
	}
	tx.commit().await.expect("commit");

	let rows = rows_after(&pool, start).await;
	assert!(rows.len() >= 100, "{} rows", rows.len());
	// A child write re-emits its parent's snapshot unchanged at the same version, which peers treat as
	// a duplicate. Two different payloads at one version are what must never happen.
	let mut latest: std::collections::HashMap<(String, String), (i64, String)> = std::collections::HashMap::new();
	for row in &rows {
		let key = (row.entity_kind.clone(), row.entity_key.clone());
		if let Some((version, snapshot)) = latest.insert(key, (row.modified_at, row.snapshot.clone())) {
			assert!(row.modified_at >= version, "{} {} went back", row.entity_kind, row.entity_key);
			assert!(
				row.modified_at > version || row.snapshot == snapshot,
				"{} {} changed without a new version",
				row.entity_kind,
				row.entity_key
			);
		}
	}
	let distinct_versions = |kind: &str| {
		rows.iter().filter(|row| row.entity_kind == kind).map(|row| row.modified_at).collect::<std::collections::HashSet<_>>().len()
	};
	assert_eq!(distinct_versions("category"), 20);
	assert_eq!(distinct_versions("chapter_user"), 20);
	assert_eq!(distinct_versions("manga_user"), 20);
}

#[tokio::test]
async fn store_operations_keep_their_journal_output() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let pool = crate::db::create_pool(&format!("sqlite://{}/journal.db?mode=rwc", tmp.path().display()))
		.await
		.expect("pool");
	let recorded = record_operations(pool).await;
	let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sync_journal_contract.json");
	if std::env::var_os("UPDATE_SYNC_JOURNAL").is_some() {
		let text = serde_json::to_string_pretty(&recorded).expect("fixture json");
		std::fs::write(&path, format!("{text}\n")).expect("write fixture");
		return;
	}
	let expected: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).expect("read fixture")).expect("parse fixture");
	assert_eq!(recorded.len(), expected.len(), "step count");
	for (recorded, expected) in recorded.iter().zip(&expected) {
		assert_eq!(recorded, expected, "{}", expected["step"]);
	}
}
