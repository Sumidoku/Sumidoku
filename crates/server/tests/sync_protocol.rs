mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{base_config, connect_db, request, request_with_headers, test_app};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::path::PathBuf;
use sumidoku_server::build_app;

struct Peer {
	app: Router,
	#[allow(dead_code)]
	tmp: tempfile::TempDir,
	db_path: PathBuf,
	origin: String,
}

async fn peer() -> Peer {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let origin: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("instance id");
	pool.close().await;
	Peer {
		app: app.app,
		tmp: app.tmp,
		db_path: app.db_path,
		origin,
	}
}

async fn pull(peer: &Peer, sender: &Peer, since: i64) -> (StatusCode, Value) {
	request(
		&peer.app,
		"GET",
		&format!(
			"/api/v1/sync/changes?origin={}&since={since}&limit=500",
			sender.origin
		),
		None,
	)
	.await
}

async fn push(peer: &Peer, sender: &Peer, changes: Value) -> (StatusCode, Value) {
	request(
		&peer.app,
		"POST",
		"/api/v1/sync/changes",
		Some(json!({"origin": sender.origin, "changes": changes})),
	)
	.await
}

async fn exchange(from: &Peer, to: &Peer) -> Value {
	let (status, body) = pull(from, to, 0).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["origin"], from.origin);
	assert_eq!(body["userId"], 1);
	let (status, applied) = push(to, from, body["changes"].clone()).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert!(applied["results"].as_array().expect("results").iter().all(|result| {
		matches!(result["status"].as_str(), Some("applied" | "duplicate" | "stale"))
	}));
	body
}

async fn local_source_id(pool: &SqlitePool) -> i64 {
	sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
		.fetch_one(pool)
		.await
		.expect("local source")
}

async fn seed_initial(peer: &Peer, manga_id: i64, category_id: i64, chapter_id: i64, suffix: &str, with_progress: bool) {
	let pool = connect_db(&peer.db_path).await;
	let source_id = local_source_id(&pool).await;
	sqlx::query("INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, ?, ?, 100)")
		.bind(manga_id)
		.bind(source_id)
		.bind(format!("manga-{suffix}"))
		.bind(format!("Manga {suffix}"))
		.execute(&pool)
		.await
		.expect("seed manga");
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, 101)")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("seed library");
	sqlx::query(
		"INSERT INTO category (id, user_id, name, sort_order, include_in_download, last_modified_at)
		 VALUES (?, 1, ?, 2, 2, 102)",
	)
	.bind(category_id)
	.bind(format!("Category {suffix}"))
	.execute(&pool)
	.await
	.expect("seed category");
	sqlx::query(
		"INSERT INTO chapter (id, manga_id, key, title, number, page_count, last_modified_at)
		 VALUES (?, ?, ?, ?, 1.5, 20, 103)",
	)
	.bind(chapter_id)
	.bind(manga_id)
	.bind(format!("chapter-{suffix}"))
	.bind(format!("Chapter {suffix}"))
	.execute(&pool)
	.await
	.expect("seed chapter");
	if with_progress {
		sqlx::query(
			"INSERT INTO chapter_user
			 (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at, is_downloaded, last_modified_at)
			 VALUES (?, 1, 7, 1, 0, 70, 1, 104)",
		)
		.bind(chapter_id)
		.execute(&pool)
		.await
		.expect("seed progress");
	}
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (?, ?, 1, 105)")
		.bind(category_id)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("seed membership");
	pool.close().await;
}

async fn id_for_key(pool: &SqlitePool, table: &str, key: &str) -> i64 {
	let sql = match table {
		"manga" => "SELECT m.id FROM manga m JOIN source s ON s.id = m.source_id WHERE s.key = 'local.local' AND m.key = ?",
		"chapter" => "SELECT id FROM chapter WHERE key = ?",
		_ => panic!("unsupported table"),
	};
	sqlx::query_scalar(sql).bind(key).fetch_one(pool).await.expect("mapped id")
}

#[tokio::test]
async fn child_first_category_stub_does_not_duplicate_existing_name() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 700, 700, "remote", false).await;
	seed_initial(&b, 900, 500, 900, "remote", false).await;
	let (_, initial) = pull(&a, &b, 0).await;
	let membership = event_of(&initial, "category_manga");
	let category = event_of(&initial, "category");

	let (status, applied) = push(&b, &a, json!([membership, category])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert!(applied["results"].as_array().expect("results").iter().all(|result| {
		matches!(result["status"].as_str(), Some("applied" | "duplicate" | "stale"))
	}));
	let pool = connect_db(&b.db_path).await;
	let names: Vec<String> = sqlx::query_scalar("SELECT name FROM category WHERE user_id = 1 AND name = 'Category remote'")
		.fetch_all(&pool)
		.await
		.expect("same-named categories");
	assert_eq!(names.len(), 1, "stub must merge into the existing named category: {names:?}");
	let membership_category: i64 = sqlx::query_scalar("SELECT category_id FROM category_manga WHERE user_id = 1")
		.fetch_one(&pool)
		.await
		.expect("membership category");
	let stored_category: i64 = sqlx::query_scalar("SELECT id FROM category WHERE user_id = 1 AND name = 'Category remote'")
		.fetch_one(&pool)
		.await
		.expect("stored category");
	assert_eq!(membership_category, stored_category);
	let a_pool = connect_db(&a.db_path).await;
	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM category_manga WHERE category_id = 700")
		.execute(&a_pool)
		.await
		.expect("delete remote membership");
	exchange_since(&a, &b, cursor).await;
	let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category_manga WHERE user_id = 1")
		.fetch_one(&pool)
		.await
		.expect("remaining memberships");
	assert_eq!(remaining, 0, "the tombstone must find the merged category");
	a_pool.close().await;
	pool.close().await;
}

#[tokio::test]
async fn category_id_collision_does_not_merge_unrelated_categories() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "remote", false).await;
	seed_initial(&b, 900, 500, 900, "local", false).await;

	exchange(&a, &b).await;

	let pool = connect_db(&b.db_path).await;
	let rows: Vec<(i64, String)> = sqlx::query_as(
		"SELECT id, name FROM category WHERE user_id = 1 AND name LIKE 'Category %' ORDER BY name",
	)
	.fetch_all(&pool)
	.await
	.expect("categories after exchange");
	assert_eq!(rows.len(), 2, "unrelated categories must not merge: {rows:?}");
	assert_eq!(rows[0].1, "Category local");
	assert_eq!(rows[1].1, "Category remote");
	assert_ne!(rows[0].0, rows[1].0);
	pool.close().await;
}

#[tokio::test]
async fn initial_bidirectional_exchange_converges_and_suppresses_origin_echo() {
	let a = peer().await;
	let b = peer().await;
	assert_ne!(a.origin, b.origin);
	seed_initial(&a, 100, 500, 700, "a", true).await;
	seed_initial(&b, 900, 900, 900, "b", false).await;

	let pulled = exchange(&a, &b).await;
	assert!(pulled["changes"].as_array().expect("changes").iter().all(|change| {
		change["origin"] == a.origin && change["entityKey"].is_string() && change["snapshot"].get("isDownloaded").is_none()
	}));
	exchange(&b, &a).await;

	for peer in [&a, &b] {
		let pool = connect_db(&peer.db_path).await;
		for key in ["manga-a", "manga-b"] {
			let id = id_for_key(&pool, "manga", key).await;
			let library: i64 = sqlx::query_scalar("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = 1")
				.bind(id)
				.fetch_one(&pool)
				.await
				.expect("library converged");
			assert_eq!(library, 1);
			let membership: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category_manga WHERE manga_id = ? AND user_id = 1")
				.bind(id)
				.fetch_one(&pool)
				.await
				.expect("membership converged");
			assert_eq!(membership, 1);
		}
		for key in ["chapter-a", "chapter-b"] {
			let chapter_id = id_for_key(&pool, "chapter", key).await;
			let title: String = sqlx::query_scalar("SELECT title FROM chapter WHERE id = ?")
				.bind(chapter_id)
				.fetch_one(&pool)
				.await
				.expect("chapter converged");
			assert_eq!(title, if key == "chapter-a" { "Chapter a" } else { "Chapter b" });
		}
		let chapter_a = id_for_key(&pool, "chapter", "chapter-a").await;
		let progress: (i64, bool, bool, Option<i64>, bool) = sqlx::query_as(
			"SELECT last_page_read, is_read, is_bookmarked, last_read_at, is_downloaded
			 FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
		)
		.bind(chapter_a)
		.fetch_one(&pool)
		.await
		.expect("progress converged");
		assert_eq!(progress, (7, true, false, Some(70), peer.origin == a.origin));
		let categories: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category WHERE user_id = 1 AND name IN ('Category a', 'Category b')")
			.fetch_one(&pool)
			.await
			.expect("categories converged");
		assert_eq!(categories, 2);
		pool.close().await;
	}

	let a_pool = connect_db(&a.db_path).await;
	let a_chapter = id_for_key(&a_pool, "chapter", "chapter-a").await;
	let b_pool = connect_db(&b.db_path).await;
	let b_chapter = id_for_key(&b_pool, "chapter", "chapter-a").await;
	assert_eq!(a_chapter, 700);
	assert_ne!(a_chapter, b_chapter);
	a_pool.close().await;
	b_pool.close().await;

	let (status, echo) = pull(&a, &b, 0).await;
	assert_eq!(status, StatusCode::OK);
	assert!(echo["changes"].as_array().expect("echo changes").iter().all(|change| change["origin"] != b.origin));
}

fn manga_change(origin: &str, sequence: i64, source_key: &str, manga_key: &str) -> Value {
	json!({
		"origin": origin,
		"originUserId": 1,
		"originSequence": sequence,
		"entityKind": "manga",
		"entityKey": "remote-manga",
		"deleted": false,
		"modifiedAt": 100,
		"identity": {"sourceKey": source_key, "mangaKey": manga_key, "mangaId": 77},
		"snapshot": {
			"id": 77,
			"key": manga_key,
			"title": "Remote",
			"authors": [],
			"artists": [],
			"description": null,
			"tags": [],
			"status": 0,
			"contentRating": 0,
			"viewer": 0,
			"coverUrl": null,
			"url": null,
			"initialized": 0,
			"updateStrategy": 0,
			"lastModifiedAt": 100
		}
	})
}

#[tokio::test]
async fn malformed_batch_is_rejected_before_any_write() {
	let a = peer().await;
	let sender = peer().await;
	let valid = manga_change(&sender.origin, 1, "local.local", "valid");
	let mut invalid = manga_change(&sender.origin, 2, "local.local", "invalid");
	invalid["entityKind"] = json!("source");
	let (status, body) = push(&a, &sender, json!([valid, invalid])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	let pool = connect_db(&a.db_path).await;
	let manga: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key IN ('valid', 'invalid')")
		.fetch_one(&pool)
		.await
		.expect("no partial write");
	assert_eq!(manga, 0);
	let dedupe: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_origin_state")
		.fetch_one(&pool)
		.await
		.expect("no dedupe write");
	assert_eq!(dedupe, 0);
	let mut mismatch = manga_change(&sender.origin, 3, "local.local", "natural");
	mismatch["snapshot"]["key"] = json!("different");
	let (status, _) = push(&a, &sender, json!([mismatch])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let mismatched: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key IN ('natural', 'different')")
		.fetch_one(&pool)
		.await
		.expect("no mismatched write");
	assert_eq!(mismatched, 0);
	pool.close().await;

	let (status, _) = request(&a.app, "GET", "/api/v1/sync/changes?since=0", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let (status, _) = request(&a.app, "GET", "/api/v1/sync/changes?origin=peer&since=0&limit=0", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let (status, _) = push(&a, &a, json!([])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn missing_source_skip_does_not_advance_and_can_be_retried() {
	let a = peer().await;
	let sender = peer().await;
	let change = manga_change(&sender.origin, 1, "missing.source", "retry");
	let (status, body) = push(&a, &sender, json!([change.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "skipped");
	assert_eq!(body["results"][0]["reason"], "source_unavailable");
	let pool = connect_db(&a.db_path).await;
	let dedupe: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_origin_state")
		.fetch_one(&pool)
		.await
		.expect("dedupe unchanged");
	assert_eq!(dedupe, 0);
	sqlx::query("INSERT INTO source (key, name, wasm_path) VALUES ('missing.source', 'Missing', '')")
		.execute(&pool)
		.await
		.expect("install missing source");
	let (status, body) = push(&a, &sender, json!([change])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "applied");
	let key: String = sqlx::query_scalar("SELECT m.key FROM manga m JOIN source s ON s.id = m.source_id WHERE s.key = 'missing.source'")
		.fetch_one(&pool)
		.await
		.expect("retried manga");
	assert_eq!(key, "retry");
	let origin_sequence: i64 = sqlx::query_scalar("SELECT origin_sequence FROM sync_origin_state")
		.fetch_one(&pool)
		.await
		.expect("dedupe advanced");
	assert_eq!(origin_sequence, 1);
	pool.close().await;
}

async fn high_watermark(pool: &SqlitePool) -> i64 {
	sqlx::query_scalar("SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)")
		.fetch_one(pool)
		.await
		.expect("high watermark")
}

async fn exchange_since(from: &Peer, to: &Peer, since: i64) -> Value {
	let (status, body) = pull(from, to, since).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let (status, applied) = push(to, from, body["changes"].clone()).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	body
}

fn event_of(body: &Value, kind: &str) -> Value {
	body["changes"]
		.as_array()
		.expect("changes")
		.iter()
		.find(|change| change["entityKind"] == kind)
		.unwrap_or_else(|| panic!("missing {kind} in {body}"))
		.clone()
}

fn set_version(change: &mut Value, sequence: i64, modified_at: i64) {
	change["originSequence"] = json!(sequence);
	change["modifiedAt"] = json!(modified_at);
	change["snapshot"]["lastModifiedAt"] = json!(modified_at);
}

#[tokio::test]
async fn duplicate_stale_rekey_delete_and_recreate_use_original_identity() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "shared", false).await;
	let (status, body) = pull(&a, &b, 0).await;
	assert_eq!(status, StatusCode::OK);
	let original = event_of(&body, "manga");
	exchange(&a, &b).await;
	let pool = connect_db(&b.db_path).await;
	let local_id = id_for_key(&pool, "manga", "manga-shared").await;

	let mut update = original.clone();
	set_version(&mut update, 1_000, 1_000);
	update["snapshot"]["title"] = json!("New");
	let (status, body) = push(&b, &a, json!([update.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "applied");
	let (status, body) = push(&b, &a, json!([update.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "duplicate");

	let mut older = update.clone();
	set_version(&mut older, 1_001, 10);
	older["snapshot"]["title"] = json!("Older");
	let (status, body) = push(&b, &a, json!([older])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "stale");
	let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE id = ?")
		.bind(local_id)
		.fetch_one(&pool)
		.await
		.expect("title after stale");
	assert_eq!(title, "New");

	let mut rekey = update.clone();
	set_version(&mut rekey, 1_002, 1_002);
	rekey["identity"]["mangaKey"] = json!("rekeyed");
	rekey["snapshot"]["key"] = json!("rekeyed");
	rekey["snapshot"]["title"] = json!("Rekeyed");
	let (status, body) = push(&b, &a, json!([rekey.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "applied");
	let mapped_id = id_for_key(&pool, "manga", "rekeyed").await;
	assert_eq!(mapped_id, local_id);

	let mut tombstone = rekey.clone();
	set_version(&mut tombstone, 1_003, 2_000);
	tombstone["deleted"] = json!(true);
	tombstone["snapshot"]["lastModifiedAt"] = json!(1_002);
	let (status, body) = push(&b, &a, json!([tombstone.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "applied");
	let mut old_update = rekey.clone();
	set_version(&mut old_update, 1_004, 1_500);
	old_update["snapshot"]["title"] = json!("Old recreation");
	let (status, body) = push(&b, &a, json!([old_update])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "stale");
	let absent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE id = ?")
		.bind(local_id)
		.fetch_one(&pool)
		.await
		.expect("old update stays deleted");
	assert_eq!(absent, 0);

	let mut recreate = rekey;
	set_version(&mut recreate, 1_005, 2_001);
	recreate["snapshot"]["title"] = json!("Recreated");
	let (status, body) = push(&b, &a, json!([recreate])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "applied");
	let recreated: (i64, String) = sqlx::query_as("SELECT id, title FROM manga WHERE key = 'rekeyed'")
		.fetch_one(&pool)
		.await
		.expect("newer recreation");
	assert_eq!(recreated, (local_id, "Recreated".to_string()));
	let dedupe: i64 = sqlx::query_scalar(
		"SELECT origin_sequence FROM sync_origin_state
		 WHERE origin = ? AND origin_user_id = 1 AND user_id = 1 AND entity_kind = 'manga' AND origin_entity_key = ?",
	)
	.bind(&a.origin)
	.bind(original["entityKey"].as_str().expect("origin entity key"))
	.fetch_one(&pool)
	.await
	.expect("latest origin sequence");
	assert_eq!(dedupe, 1_005);
	pool.close().await;
}

async fn seed_conflict(peer: &Peer, id: i64, title: &str) {
	let pool = connect_db(&peer.db_path).await;
	let source_id = local_source_id(&pool).await;
	sqlx::query("INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, 'shared', ?, 1000)")
		.bind(id)
		.bind(source_id)
		.bind(title)
		.execute(&pool)
		.await
		.expect("conflict manga");
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (?, 1, 1, 1000)")
		.bind(id)
		.execute(&pool)
		.await
		.expect("conflict library");
	pool.close().await;
}

#[tokio::test]
async fn equal_versions_use_origin_as_deterministic_tie_break() {
	let a = peer().await;
	let b = peer().await;
	seed_conflict(&a, 100, "A").await;
	seed_conflict(&b, 900, "B").await;
	exchange(&a, &b).await;
	exchange(&b, &a).await;
	let expected = if a.origin > b.origin { "A" } else { "B" };
	for peer in [&a, &b] {
		let pool = connect_db(&peer.db_path).await;
		let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE key = 'shared'")
			.fetch_one(&pool)
			.await
			.expect("conflict winner");
		assert_eq!(title, expected);
		let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE key = 'shared'")
			.fetch_one(&pool)
			.await
			.expect("single conflict row");
		assert_eq!(rows, 1);
		pool.close().await;
	}
}

#[tokio::test]
async fn stale_event_preserves_mapping_for_later_rekey() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "shared", false).await;
	seed_initial(&b, 900, 900, 900, "shared", false).await;
	let b_pool = connect_db(&b.db_path).await;
	sqlx::query("UPDATE manga SET title = 'Local newer', last_modified_at = 200 WHERE id = 900")
		.execute(&b_pool)
		.await
		.expect("local newer edit");
	let (_, initial) = pull(&a, &b, 0).await;
	let mut stale = event_of(&initial, "manga");
	set_version(&mut stale, 100, 100);
	let (status, applied) = push(&b, &a, json!([stale])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "stale");

	let mut rekey = event_of(&initial, "manga");
	rekey["identity"]["mangaKey"] = json!("rekeyed");
	rekey["snapshot"]["key"] = json!("rekeyed");
	rekey["snapshot"]["title"] = json!("Remote rekey");
	set_version(&mut rekey, 101, 300);
	let (status, applied) = push(&b, &a, json!([rekey])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "applied");
	let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM manga WHERE key IN ('shared', 'rekeyed')")
		.fetch_all(&b_pool)
		.await
		.expect("rekeyed manga");
	assert_eq!(rows, [(900, "rekeyed".to_string())]);
	b_pool.close().await;
}

#[tokio::test]
async fn progress_merge_keeps_actual_version_when_event_is_missing() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "remote", true).await;
	let (_, initial) = pull(&a, &b, 0).await;
	exchange(&a, &b).await;
	let b_pool = connect_db(&b.db_path).await;
	let chapter_id = id_for_key(&b_pool, "chapter", "chapter-remote").await;
	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 1, is_read = 0, is_bookmarked = 0,
		 last_read_at = 10, last_modified_at = 100 WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.execute(&b_pool)
	.await
	.expect("local progress with newer version");
	sqlx::query("DELETE FROM sync_change WHERE entity_kind = 'chapter_user' AND entity_key = ?")
		.bind(format!("{chapter_id}:1"))
		.execute(&b_pool)
		.await
		.expect("remove current event mapping");

	let mut incoming = event_of(&initial, "chapter_user");
	incoming["snapshot"]["lastPageRead"] = json!(5);
	incoming["snapshot"]["isRead"] = json!(true);
	incoming["snapshot"]["isBookmarked"] = json!(false);
	incoming["snapshot"]["lastReadAt"] = json!(20);
	set_version(&mut incoming, 50, 50);
	let (status, applied) = push(&b, &a, json!([incoming])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	let merged: (i64, i64, bool, bool, i64) = sqlx::query_as(
		"SELECT last_modified_at, last_page_read, is_read, is_bookmarked, last_read_at
		 FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_one(&b_pool)
	.await
	.expect("merged progress");
	assert_eq!(merged, (100, 5, true, false, 20));

	let mut bookmark = event_of(&initial, "chapter_user");
	bookmark["snapshot"]["isBookmarked"] = json!(true);
	set_version(&mut bookmark, 51, 75);
	let (status, applied) = push(&b, &a, json!([bookmark])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	let bookmarked: (bool, i64) = sqlx::query_as(
		"SELECT is_bookmarked, last_modified_at FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_one(&b_pool)
	.await
	.expect("bookmark version");
	assert_eq!(bookmarked, (false, 100));

	let mut tombstone = event_of(&initial, "chapter_user");
	set_version(&mut tombstone, 52, 75);
	tombstone["deleted"] = json!(true);
	tombstone["snapshot"]["lastModifiedAt"] = json!(74);
	let (status, applied) = push(&b, &a, json!([tombstone])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	let remaining: (bool, i64) = sqlx::query_as(
		"SELECT is_bookmarked, last_modified_at FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_one(&b_pool)
	.await
	.expect("progress survives older tombstone");
	assert_eq!(remaining, (false, 100));
	b_pool.close().await;
}

#[tokio::test]
async fn compaction_preserves_older_origin_aliases() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 700, 700, "remote", false).await;
	exchange(&a, &b).await;
	let a_pool = connect_db(&a.db_path).await;
	let b_pool = connect_db(&b.db_path).await;
	let category_id: i64 = sqlx::query_scalar("SELECT id FROM category WHERE user_id = 1 AND name = 'Category remote'")
		.fetch_one(&b_pool)
		.await
		.expect("mapped category");
	sqlx::query("UPDATE category SET name = 'Local edit', last_modified_at = 300 WHERE id = ?")
		.bind(category_id)
		.execute(&b_pool)
		.await
		.expect("local category edit");
	let cursor = high_watermark(&a_pool).await;
	sqlx::query("UPDATE category SET name = 'Remote newer', last_modified_at = 400 WHERE id = 700")
		.execute(&a_pool)
		.await
		.expect("remote category edit");
	let (status, compacted) = request(&b.app, "POST", "/api/v1/sync/compact", None).await;
	assert_eq!(status, StatusCode::OK, "{compacted}");
	exchange_since(&a, &b, cursor).await;

	let names: Vec<String> = sqlx::query_scalar("SELECT name FROM category WHERE user_id = 1 ORDER BY name")
		.fetch_all(&b_pool)
		.await
		.expect("categories after old-origin update");
	assert_eq!(names, ["Remote newer".to_string()]);
	a_pool.close().await;
	b_pool.close().await;
}

#[tokio::test]
async fn progress_merge_is_commutative_and_unread_cannot_regress_read() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "progress", true).await;
	exchange(&a, &b).await;
	let a_pool = connect_db(&a.db_path).await;
	let b_pool = connect_db(&b.db_path).await;
	let a_chapter = id_for_key(&a_pool, "chapter", "chapter-progress").await;
	let b_chapter = id_for_key(&b_pool, "chapter", "chapter-progress").await;
	let a_cursor = high_watermark(&a_pool).await;
	let b_cursor = high_watermark(&b_pool).await;
	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 5, is_read = 1, is_bookmarked = 0,
		 last_read_at = 50, last_modified_at = 1000 WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(a_chapter)
	.execute(&a_pool)
	.await
	.expect("A progress edit");
	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 9, is_read = 0, is_bookmarked = 1,
		 last_read_at = 80, last_modified_at = 1100 WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(b_chapter)
	.execute(&b_pool)
	.await
	.expect("B progress edit");
	let pulled = exchange_since(&a, &b, a_cursor).await;
	let progress_event = event_of(&pulled, "chapter_user");
	exchange_since(&b, &a, b_cursor).await;
	for (pool, chapter_id) in [(&a_pool, a_chapter), (&b_pool, b_chapter)] {
		let progress: (i64, bool, bool, Option<i64>) = sqlx::query_as(
			"SELECT last_page_read, is_read, is_bookmarked, last_read_at FROM chapter_user
			 WHERE chapter_id = ? AND user_id = 1",
		)
		.bind(chapter_id)
		.fetch_one(pool)
		.await
		.expect("merged progress");
		assert_eq!(progress, (9, true, true, Some(80)));
	}

	let (status, body) = push(&b, &a, json!([progress_event.clone()])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "duplicate");
	let mut out_of_order = progress_event;
	out_of_order["originSequence"] = json!(out_of_order["originSequence"].as_i64().expect("sequence") - 1);
	let (status, body) = push(&b, &a, json!([out_of_order])).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["results"][0]["status"], "stale");

	let a_cursor = high_watermark(&a_pool).await;
	let b_cursor = high_watermark(&b_pool).await;
	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 0, is_read = 0, is_bookmarked = 0,
		 last_read_at = NULL, last_modified_at = 1200 WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(a_chapter)
	.execute(&a_pool)
	.await
	.expect("A mark unread");
	exchange_since(&a, &b, a_cursor).await;
	exchange_since(&b, &a, b_cursor).await;
	for (pool, chapter_id) in [(&a_pool, a_chapter), (&b_pool, b_chapter)] {
		let progress: (i64, bool, bool, Option<i64>) = sqlx::query_as(
			"SELECT last_page_read, is_read, is_bookmarked, last_read_at FROM chapter_user
			 WHERE chapter_id = ? AND user_id = 1",
		)
		.bind(chapter_id)
		.fetch_one(pool)
		.await
		.expect("read does not regress");
		assert_eq!(progress, (9, true, false, Some(80)));
		let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.fetch_one(pool)
			.await
			.expect("one progress row");
		assert_eq!(rows, 1);
	}
	a_pool.close().await;
	b_pool.close().await;
}

#[tokio::test]
async fn child_first_creates_quiet_stubs_and_later_parents_fill_them() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "late", true).await;
	let (status, body) = pull(&a, &b, 0).await;
	assert_eq!(status, StatusCode::OK);
	let progress = event_of(&body, "chapter_user");
	let membership = event_of(&body, "category_manga");
	let (status, response) = push(&b, &a, json!([progress])).await;
	assert_eq!(status, StatusCode::OK, "{response}");
	assert_eq!(response["results"][0]["status"], "applied");
	let (status, response) = push(&b, &a, json!([membership])).await;
	assert_eq!(status, StatusCode::OK, "{response}");
	assert_eq!(response["results"][0]["status"], "applied");

	let pool = connect_db(&b.db_path).await;
	let manga_id = id_for_key(&pool, "manga", "manga-late").await;
	let chapter_id = id_for_key(&pool, "chapter", "chapter-late").await;
	let versions: (i64, String, i64, Option<String>) = sqlx::query_as(
		"SELECT m.last_modified_at, m.title, c.last_modified_at, c.title
		 FROM manga m JOIN chapter c ON c.manga_id = m.id WHERE m.id = ? AND c.id = ?",
	)
	.bind(manga_id)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("stub versions");
	assert_eq!(versions, (0, String::new(), 0, None));
	let library: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = ? AND user_id = 1")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("no stub library membership");
	assert_eq!(library, 0);
	let category_version: (String, i64) = sqlx::query_as("SELECT name, last_modified_at FROM category WHERE id = 500 AND user_id = 1")
		.fetch_one(&pool)
		.await
		.expect("category stub");
	assert_eq!(category_version, (String::new(), 0));
	let kinds: Vec<String> = sqlx::query_scalar("SELECT entity_kind FROM sync_change WHERE user_id = 1 ORDER BY sequence")
		.fetch_all(&pool)
		.await
		.expect("child-only journal");
	assert_eq!(kinds, ["chapter_user", "category_manga"]);

	for kind in ["manga", "chapter", "category", "manga_user"] {
		let event = event_of(&body, kind);
		let (status, response) = push(&b, &a, json!([event])).await;
		assert_eq!(status, StatusCode::OK, "{kind}: {response}");
		assert_eq!(response["results"][0]["status"], "applied");
	}
	let filled: (String, i64, String, i64, String) = sqlx::query_as(
		"SELECT m.title, m.last_modified_at, c.title, c.last_modified_at, cat.name
		 FROM manga m JOIN chapter c ON c.manga_id = m.id JOIN category cat ON cat.id = 500
		 WHERE m.id = ? AND c.id = ?",
	)
	.bind(manga_id)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("filled parents");
	assert_eq!(filled, ("Manga late".to_string(), 100, "Chapter late".to_string(), 103, "Category late".to_string()));
	let progress: (i64, bool, Option<i64>) = sqlx::query_as(
		"SELECT last_page_read, is_read, last_read_at FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
	)
	.bind(chapter_id)
	.fetch_one(&pool)
	.await
	.expect("progress survives late parents");
	assert_eq!(progress, (7, true, Some(70)));
	let library: i64 = sqlx::query_scalar("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = 1")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("authoritative library event");
	assert_eq!(library, 1);
	pool.close().await;
}

#[tokio::test]
async fn category_library_and_chapter_deletions_converge() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "delete", true).await;
	exchange(&a, &b).await;
	let a_pool = connect_db(&a.db_path).await;
	let b_pool = connect_db(&b.db_path).await;
	let b_manga = id_for_key(&b_pool, "manga", "manga-delete").await;
	let b_chapter = id_for_key(&b_pool, "chapter", "chapter-delete").await;

	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM category WHERE id = 500")
		.execute(&a_pool)
		.await
		.expect("delete category");
	exchange_since(&a, &b, cursor).await;
	let category: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category WHERE id = 500 AND user_id = 1")
		.fetch_one(&b_pool)
		.await
		.expect("category removed");
	assert_eq!(category, 0);
	let membership: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category_manga WHERE manga_id = ? AND user_id = 1")
		.bind(b_manga)
		.fetch_one(&b_pool)
		.await
		.expect("membership removed");
	assert_eq!(membership, 0);

	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM manga_user WHERE manga_id = 100 AND user_id = 1")
		.execute(&a_pool)
		.await
		.expect("remove library");
	exchange_since(&a, &b, cursor).await;
	let library: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = ? AND user_id = 1")
		.bind(b_manga)
		.fetch_one(&b_pool)
		.await
		.expect("library removed");
	assert_eq!(library, 0);
	let manga: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE id = ?")
		.bind(b_manga)
		.fetch_one(&b_pool)
		.await
		.expect("manga remains");
	assert_eq!(manga, 1);

	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM chapter WHERE id = 700")
		.execute(&a_pool)
		.await
		.expect("delete chapter");
	exchange_since(&a, &b, cursor).await;
	let chapter: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter WHERE id = ?")
		.bind(b_chapter)
		.fetch_one(&b_pool)
		.await
		.expect("chapter removed");
	assert_eq!(chapter, 0);
	let progress: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE chapter_id = ? AND user_id = 1")
		.bind(b_chapter)
		.fetch_one(&b_pool)
		.await
		.expect("progress removed");
	assert_eq!(progress, 0);
	a_pool.close().await;
	b_pool.close().await;
}

#[tokio::test]
async fn source_cascade_converges_without_leaving_children() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "cascade", true).await;
	exchange(&a, &b).await;
	let a_pool = connect_db(&a.db_path).await;
	let b_pool = connect_db(&b.db_path).await;
	let b_manga = id_for_key(&b_pool, "manga", "manga-cascade").await;
	let b_chapter = id_for_key(&b_pool, "chapter", "chapter-cascade").await;
	let source_id = local_source_id(&a_pool).await;
	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM source WHERE id = ?")
		.bind(source_id)
		.execute(&a_pool)
		.await
		.expect("delete source");
	exchange_since(&a, &b, cursor).await;
	for (table, id) in [("manga", b_manga), ("chapter", b_chapter)] {
		let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE id = ?"))
			.bind(id)
			.fetch_one(&b_pool)
			.await
			.expect("cascade row removed");
		assert_eq!(count, 0, "{table}");
	}
	let user_state: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga_user WHERE manga_id = ? AND user_id = 1")
		.bind(b_manga)
		.fetch_one(&b_pool)
		.await
		.expect("library cascade removed");
	assert_eq!(user_state, 0);
	let progress: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE chapter_id = ? AND user_id = 1")
		.bind(b_chapter)
		.fetch_one(&b_pool)
		.await
		.expect("progress cascade removed");
	assert_eq!(progress, 0);
	let membership: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category_manga WHERE manga_id = ? AND user_id = 1")
		.bind(b_manga)
		.fetch_one(&b_pool)
		.await
		.expect("membership cascade removed");
	assert_eq!(membership, 0);
	let category: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category WHERE id = 500 AND user_id = 1")
		.fetch_one(&b_pool)
		.await
		.expect("category itself remains");
	assert_eq!(category, 1);
	a_pool.close().await;
	b_pool.close().await;
}

#[tokio::test]
async fn compaction_replays_baseline_and_preserves_origin_mapping() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "a", true).await;
	seed_initial(&b, 900, 900, 900, "b", false).await;
	exchange(&a, &b).await;
	exchange(&b, &a).await;
	let a_pool = connect_db(&a.db_path).await;
	sqlx::query("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (999, 1, 'Gone', 200)")
		.execute(&a_pool)
		.await
		.expect("seed tombstone category");
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id, last_modified_at) VALUES (999, 100, 1, 201)")
		.execute(&a_pool)
		.await
		.expect("seed tombstone membership");
	sqlx::query("DELETE FROM category WHERE id = 999")
		.execute(&a_pool)
		.await
		.expect("create category tombstones");
	let (status, b_body) = pull(&b, &a, 0).await;
	assert_eq!(status, StatusCode::OK);
	let mut authoritative = b_body["changes"]
		.as_array()
		.expect("B changes")
		.iter()
		.find(|change| change["entityKind"] == "manga" && change["identity"]["mangaKey"] == "manga-b")
		.expect("B authoritative manga")
		.clone();
	set_version(&mut authoritative, 4_000, 4_000);
	authoritative["snapshot"]["title"] = json!("B before compact");
	let (status, applied) = push(&a, &b, json!([authoritative])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "applied");
	let origin_states: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_origin_state")
		.fetch_one(&a_pool)
		.await
		.expect("origin states before compaction");
	assert!(origin_states > 0);
	let old_cursor = high_watermark(&a_pool).await;
	assert!(old_cursor > 0);
	let mapped_before = id_for_key(&a_pool, "manga", "manga-b").await;

	let (status, compacted) = request(&a.app, "POST", "/api/v1/sync/compact", None).await;
	assert_eq!(status, StatusCode::OK, "{compacted}");
	assert!(compacted["removed"].as_i64().expect("removed") > 0);
	assert!(compacted["baseCursor"].as_i64().expect("base cursor") > old_cursor);
	let origin_states_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_origin_state")
		.fetch_one(&a_pool)
		.await
		.expect("origin states after compaction");
	assert_eq!(origin_states_after, origin_states);
	let (status, stale) = pull(&a, &b, old_cursor).await;
	assert_eq!(status, StatusCode::CONFLICT, "{stale}");
	assert_eq!(stale["error"]["code"], "resync_required");
	let c = peer().await;
	let (status, baseline) = pull(&a, &c, 0).await;
	assert_eq!(status, StatusCode::OK, "{baseline}");
	let changes = baseline["changes"].as_array().expect("baseline changes");
	assert!(!changes.is_empty());
	let rank = |kind: &str| match kind {
		"manga" => 1,
		"manga_user" => 2,
		"category" => 3,
		"chapter" => 4,
		"chapter_user" => 5,
		"category_manga" => 6,
		_ => 99,
	};
	assert!(changes.windows(2).all(|pair| {
		rank(pair[0]["entityKind"].as_str().expect("kind")) <= rank(pair[1]["entityKind"].as_str().expect("kind"))
	}));
	assert!(changes.iter().any(|change| {
		change["entityKind"] == "category" && change["entityKey"] == "999" && change["deleted"] == true
	}));
	assert!(changes.iter().any(|change| {
		change["entityKind"] == "manga"
			&& change["identity"]["mangaKey"] == "manga-b"
			&& change["origin"] == b.origin
			&& change["entityKey"] == "900"
	}));

	let (status, replay) = push(&c, &a, baseline["changes"].clone()).await;
	assert_eq!(status, StatusCode::OK, "{replay}");
	assert!(replay["results"].as_array().expect("replay results").iter().all(|result| {
		matches!(result["status"].as_str(), Some("applied" | "duplicate" | "stale"))
	}));
	let c_pool = connect_db(&c.db_path).await;
	for key in ["manga-a", "manga-b"] {
		id_for_key(&c_pool, "manga", key).await;
	}
	let tombstone_category: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM category WHERE id = 999")
		.fetch_one(&c_pool)
		.await
		.expect("tombstone does not recreate category");
	assert_eq!(tombstone_category, 0);
	let counts_before: (i64, i64, i64) = sqlx::query_as(
		"SELECT (SELECT COUNT(*) FROM manga), (SELECT COUNT(*) FROM chapter), (SELECT COUNT(*) FROM sync_change)",
	)
	.fetch_one(&c_pool)
	.await
	.expect("counts before repeated replay");
	let (status, replay) = push(&c, &a, baseline["changes"].clone()).await;
	assert_eq!(status, StatusCode::OK, "{replay}");
	let counts_after: (i64, i64, i64) = sqlx::query_as(
		"SELECT (SELECT COUNT(*) FROM manga), (SELECT COUNT(*) FROM chapter), (SELECT COUNT(*) FROM sync_change)",
	)
	.fetch_one(&c_pool)
	.await
	.expect("counts after repeated replay");
	assert_eq!(counts_after, counts_before);

	let delta_cursor = high_watermark(&a_pool).await;
	sqlx::query("UPDATE manga SET title = 'After compact', last_modified_at = 5000 WHERE id = 100")
		.execute(&a_pool)
		.await
		.expect("post-compaction delta");
	exchange_since(&a, &c, delta_cursor).await;
	let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE key = 'manga-a'")
		.fetch_one(&c_pool)
		.await
		.expect("delta after baseline");
	assert_eq!(title, "After compact");

	let (status, b_changes) = pull(&b, &a, 0).await;
	assert_eq!(status, StatusCode::OK);
	let mut remote_update = event_of(&b_changes, "manga");
	set_version(&mut remote_update, 5_000, 5_000);
	remote_update["snapshot"]["title"] = json!("B after compact");
	let (status, applied) = push(&a, &b, json!([remote_update])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "applied");
	let mapped_after = id_for_key(&a_pool, "manga", "manga-b").await;
	assert_eq!(mapped_after, mapped_before);
	let title: String = sqlx::query_scalar("SELECT title FROM manga WHERE id = ?")
		.bind(mapped_after)
		.fetch_one(&a_pool)
		.await
		.expect("origin mapping after compaction");
	assert_eq!(title, "B after compact");
	a_pool.close().await;
	c_pool.close().await;
}

async fn login(app: &Router, username: &str, password: &str) -> String {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": username, "password": password})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	format!("Bearer {}", body["token"].as_str().expect("token"))
}

async fn authed(peer: &Peer, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(&peer.app, method, path, body, &[("authorization", token)]).await;
	(status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn pull_authed(peer: &Peer, sender: &Peer, token: &str, since: i64) -> (StatusCode, Value) {
	authed(
		peer,
		token,
		"GET",
		&format!("/api/v1/sync/changes?origin={}&since={since}&limit=500", sender.origin),
		None,
	)
	.await
}

async fn push_authed(peer: &Peer, sender: &Peer, token: &str, changes: Value) -> (StatusCode, Value) {
	authed(
		peer,
		token,
		"POST",
		"/api/v1/sync/changes",
		Some(json!({"origin": sender.origin, "changes": changes})),
	)
	.await
}

async fn auth_peer() -> Peer {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some("secret".into());
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build auth app");
	let pool = connect_db(&db_path).await;
	let origin: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("instance id");
	let hash = sumidoku_server::auth::hash_password("hunter2").expect("hash password");
	sqlx::query("INSERT INTO user (id, username, password_hash, role) VALUES (2, 'bob', ?, 'reader')")
		.bind(hash)
		.execute(&pool)
		.await
		.expect("insert reader");
	pool.close().await;
	Peer {
		app,
		tmp,
		db_path,
		origin,
	}
}

#[tokio::test]
async fn reader_and_admin_sync_only_their_own_state() {
	let a = auth_peer().await;
	let sender = peer().await;
	seed_initial(&a, 100, 500, 700, "auth", true).await;
	let pool = connect_db(&a.db_path).await;
	sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, last_modified_at) VALUES (100, 2, 1, 300)")
		.execute(&pool)
		.await
		.expect("reader library");
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, last_read_at, last_modified_at)
		 VALUES (700, 2, 2, 0, 20, 301)",
	)
	.execute(&pool)
	.await
	.expect("reader progress");
	sqlx::query("INSERT INTO category (id, user_id, name, last_modified_at) VALUES (600, 1, 'Admin only', 302), (601, 2, 'Bob only', 303)")
		.execute(&pool)
		.await
		.expect("user categories");
	let admin = login(&a.app, "admin", "secret").await;
	let bob = login(&a.app, "bob", "hunter2").await;

	let (status, bob_changes) = pull_authed(&a, &sender, &bob, 0).await;
	assert_eq!(status, StatusCode::OK, "{bob_changes}");
	assert_eq!(bob_changes["userId"], 2);
	let bob_categories: Vec<&str> = bob_changes["changes"]
		.as_array()
		.expect("reader changes")
		.iter()
		.filter(|change| change["entityKind"] == "category")
		.filter_map(|change| change["snapshot"]["name"].as_str())
		.collect();
	assert_eq!(bob_categories, ["Bob only"]);
	let (status, admin_changes) = pull_authed(&a, &sender, &admin, 0).await;
	assert_eq!(status, StatusCode::OK, "{admin_changes}");
	assert_eq!(admin_changes["userId"], 1);
	let admin_categories: Vec<&str> = admin_changes["changes"]
		.as_array()
		.expect("admin changes")
		.iter()
		.filter(|change| change["entityKind"] == "category")
		.filter_map(|change| change["snapshot"]["name"].as_str())
		.collect();
	assert_eq!(admin_categories, ["Category auth", "Admin only"]);

	let mut remote = event_of(&bob_changes, "manga_user");
	remote["origin"] = json!(sender.origin);
	remote["originUserId"] = json!(1);
	remote["originSequence"] = json!(5_000);
	remote["entityKey"] = json!("remote-bob-state");
	remote["identity"]["userId"] = json!(1);
	remote["snapshot"]["inLibrary"] = json!(0);
	set_version(&mut remote, 5_000, 5_000);
	let (status, applied) = push_authed(&a, &sender, &bob, json!([remote])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "applied");
	let states: Vec<(i64, i64)> = sqlx::query_as("SELECT user_id, in_library FROM manga_user WHERE manga_id = 100 ORDER BY user_id")
		.fetch_all(&pool)
		.await
		.expect("user library states");
	assert_eq!(states, [(1, 1), (2, 0)]);
	let admin_remote: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_change WHERE user_id = 1 AND origin = ?")
		.bind(&sender.origin)
		.fetch_one(&pool)
		.await
		.expect("no admin remote event");
	assert_eq!(admin_remote, 0);
	let reader_dedupe: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM sync_origin_state WHERE user_id = 2 AND origin = ? AND origin_entity_key = 'remote-bob-state'",
	)
	.bind(&sender.origin)
	.fetch_one(&pool)
	.await
	.expect("reader dedupe");
	assert_eq!(reader_dedupe, 1);

	let (status, _) = authed(&a, &bob, "POST", "/api/v1/sync/compact", None).await;
	assert_eq!(status, StatusCode::FORBIDDEN);
	let (status, compacted) = authed(&a, &admin, "POST", "/api/v1/sync/compact", None).await;
	assert_eq!(status, StatusCode::OK, "{compacted}");
	pool.close().await;
}

#[tokio::test]
async fn compaction_does_not_reuse_tombstone_origin_for_recreated_stub() {
	let a = peer().await;
	let b = peer().await;
	seed_initial(&a, 100, 500, 700, "stub", true).await;
	let (status, initial) = pull(&a, &b, 0).await;
	assert_eq!(status, StatusCode::OK);
	exchange(&a, &b).await;
	let a_pool = connect_db(&a.db_path).await;
	let cursor = high_watermark(&a_pool).await;
	sqlx::query("DELETE FROM manga WHERE id = 100")
		.execute(&a_pool)
		.await
		.expect("delete parent before stub recreation");
	exchange_since(&a, &b, cursor).await;
	let mut child = event_of(&initial, "chapter_user");
	set_version(&mut child, 5_000, 9_000_000_000_000_000);
	let (status, applied) = push(&b, &a, json!([child])).await;
	assert_eq!(status, StatusCode::OK, "{applied}");
	assert_eq!(applied["results"][0]["status"], "applied");
	let b_pool = connect_db(&b.db_path).await;
	let stub_version: i64 = sqlx::query_scalar("SELECT last_modified_at FROM manga WHERE key = 'manga-stub'")
		.fetch_one(&b_pool)
		.await
		.expect("recreated stub");
	assert_eq!(stub_version, 0);
	let (status, compacted) = request(&b.app, "POST", "/api/v1/sync/compact", None).await;
	assert_eq!(status, StatusCode::OK, "{compacted}");
	let c = peer().await;
	let (status, baseline) = pull(&b, &c, 0).await;
	assert_eq!(status, StatusCode::OK, "{baseline}");
	for kind in ["manga", "chapter"] {
		let current = baseline["changes"]
			.as_array()
			.expect("baseline changes")
			.iter()
			.find(|change| change["entityKind"] == kind && !change["deleted"].as_bool().unwrap_or(false))
			.unwrap_or_else(|| panic!("missing current {kind}: {baseline}"));
		assert_eq!(current["origin"], b.origin, "{kind} reused tombstone origin");
	}
	a_pool.close().await;
	b_pool.close().await;
}
