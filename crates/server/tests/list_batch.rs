mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, connect_db, request, test_app};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// sqlx emits one target=sqlx::query event (DEBUG level) per executed statement, so counting events
/// counts statements; the PRAGMA batch at connect time is connection-lifecycle overhead, not endpoint statements
#[derive(Clone, Default)]
struct QueryCounter {
	statements: Arc<std::sync::Mutex<Vec<String>>>,
}

impl QueryCounter {
	fn install() -> Self {
		let counter = Self::default();
		tracing_subscriber::registry()
			.with(tracing_subscriber::EnvFilter::new("sqlx::query=debug"))
			.with(counter.clone())
			.try_init()
			.ok();
		counter
	}

	fn mark(&self) -> usize {
		self.statements.lock().expect("statements lock").len()
	}

	fn since(&self, mark: usize) -> Vec<String> {
		self.statements.lock().expect("statements lock")[mark..].to_vec()
	}
}

/// The event's summary field is the statement digest (sqlx takes the first 4 words of the SQL)
#[derive(Default)]
struct SummaryVisitor(String);

impl tracing::field::Visit for SummaryVisitor {
	fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
		if field.name() == "summary" {
			self.0 = format!("{value:?}");
		}
	}

	fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
		if field.name() == "summary" {
			self.0 = value.to_string();
		}
	}
}

impl<S: tracing::Subscriber> Layer<S> for QueryCounter {
	fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
		let mut visitor = SummaryVisitor::default();
		event.record(&mut visitor);
		if visitor.0.trim_start_matches('"').starts_with("PRAGMA") {
			return;
		}
		self.statements.lock().expect("statements lock").push(visitor.0);
	}
}

async fn seed_source(pool: &sqlx::SqlitePool) -> i64 {
	sqlx::query("INSERT INTO source (key, name, wasm_path) VALUES ('seed', 'Seed', '/seed/main.wasm')")
		.execute(pool)
		.await
		.expect("insert source");
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM source WHERE key = 'seed'")
		.fetch_one(pool)
		.await
		.expect("source id");
	id
}

/// Creates one manga with chapters chapters (increasing fetched_at), returning (manga_id, chapter id list)
async fn seed_manga(
	pool: &sqlx::SqlitePool,
	source_id: i64,
	key: &str,
	chapters: i64,
	fetched_at: i64,
) -> (i64, Vec<i64>) {
	sqlx::query("INSERT INTO manga (source_id, key, title, initialized) VALUES (?, ?, ?, 1)")
		.bind(source_id)
		.bind(key)
		.bind(format!("Manga {key}"))
		.execute(pool)
		.await
		.expect("insert manga");
	let (manga_id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_one(pool)
		.await
		.expect("manga id");
	let mut ids = Vec::new();
	for n in 0..chapters {
		let chapter_key = format!("{key}-ch{n}");
		sqlx::query(
			"INSERT INTO chapter (manga_id, key, source_order, page_count, fetched_at)
			 VALUES (?, ?, ?, 10, ?)",
		)
		.bind(manga_id)
		.bind(&chapter_key)
		.bind(n)
		.bind(fetched_at + n)
		.execute(pool)
		.await
		.expect("insert chapter");
		let (chapter_id,): (i64,) =
			sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
				.bind(manga_id)
				.bind(&chapter_key)
				.fetch_one(pool)
				.await
				.expect("chapter id");
		ids.push(chapter_id);
	}
	(manga_id, ids)
}

async fn put_in_library(pool: &sqlx::SqlitePool, manga_id: i64, in_library: bool, at: i64) {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, 1, ?, ?)",
	)
	.bind(manga_id)
	.bind(in_library)
	.bind(at)
	.execute(pool)
	.await
	.expect("insert manga_user");
}

async fn put_progress(pool: &sqlx::SqlitePool, chapter_id: i64, last_page_read: i64, last_read_at: i64) {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, last_read_at)
		 VALUES (?, 1, ?, ?)",
	)
	.bind(chapter_id)
	.bind(last_page_read)
	.bind(last_read_at)
	.execute(pool)
	.await
	.expect("insert chapter_user");
}

/// Issues one warmup request first (connection pool / statement cache in place), then a measured one, returning (statement digests within the window, response body)
async fn counted_get(app: &Router, counter: &QueryCounter, path: &str) -> (Vec<String>, Value) {
	let (status, _) = request(app, "GET", path, None).await;
	assert_eq!(status, StatusCode::OK, "warmup GET {path}");
	let mark = counter.mark();
	let (status, body) = request(app, "GET", path, None).await;
	assert_eq!(status, StatusCode::OK, "GET {path}");
	(counter.since(mark), body)
}

fn entry_count(body: &Value) -> usize {
	body["entries"].as_array().expect("entries").len()
}

#[tokio::test]
async fn list_endpoints_constant_sql_per_page() {
	let counter = QueryCounter::install();
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let source_id = seed_source(&pool).await;

	// 8 shelved manga, 2 chapters each, progress on each first chapter: library 8 rows, history 8 rows, recent 16 rows
	let mut manga_ids = Vec::new();
	let mut first_chapters = Vec::new();
	for n in 0..8 {
		let (manga_id, chapters) = seed_manga(&pool, source_id, &format!("m{n}"), 2, 10_000 + n * 100).await;
		put_in_library(&pool, manga_id, true, 1000 + n).await;
		put_progress(&pool, chapters[0], 3, 2000 + n).await;
		manga_ids.push(manga_id);
		first_chapters.push(chapters[0]);
	}

	// history: 8 rows vs 1 row, the statement count must be the same (old implementation was 1+8*4 vs 1+1*4)
	let (many, body) = counted_get(&app, &counter, "/api/v1/history").await;
	assert_eq!(entry_count(&body), 8);
	sqlx::query("DELETE FROM chapter_user WHERE chapter_id != ?")
		.bind(first_chapters[0])
		.execute(&pool)
		.await
		.expect("trim chapter_user");
	let (one, body) = counted_get(&app, &counter, "/api/v1/history").await;
	assert_eq!(entry_count(&body), 1);
	assert_eq!(
		many.len(),
		one.len(),
		"GET /api/v1/history: 8 rows vs 1 row statements differ\nmany: {many:?}\none: {one:?}"
	);
	assert!(
		many.len() <= 8,
		"GET /api/v1/history: {} statements for 8 rows is not a small constant: {many:?}",
		many.len()
	);

	// update/recent: 16 rows vs 1 row
	let (many, body) = counted_get(&app, &counter, "/api/v1/update/recent").await;
	assert_eq!(entry_count(&body), 16);
	sqlx::query("DELETE FROM chapter WHERE id != ?")
		.bind(first_chapters[0])
		.execute(&pool)
		.await
		.expect("trim chapter");
	let (one, body) = counted_get(&app, &counter, "/api/v1/update/recent").await;
	assert_eq!(entry_count(&body), 1);
	assert_eq!(
		many.len(),
		one.len(),
		"GET /api/v1/update/recent: 16 rows vs 1 row statements differ\nmany: {many:?}\none: {one:?}"
	);
	assert!(
		many.len() <= 8,
		"GET /api/v1/update/recent: {} statements for 16 rows is not a small constant: {many:?}",
		many.len()
	);

	// library: 8 rows vs 1 row (main query + batched unread + serialization queries)
	let (many, body) = counted_get(&app, &counter, "/api/v1/library").await;
	assert_eq!(entry_count(&body), 8);
	sqlx::query("DELETE FROM manga_user WHERE manga_id != ?")
		.bind(manga_ids[0])
		.execute(&pool)
		.await
		.expect("trim manga_user");
	let (one, body) = counted_get(&app, &counter, "/api/v1/library").await;
	assert_eq!(entry_count(&body), 1);
	assert_eq!(
		many.len(),
		one.len(),
		"GET /api/v1/library: 8 rows vs 1 row statements differ\nmany: {many:?}\none: {one:?}"
	);
	assert!(
		many.len() <= 8,
		"GET /api/v1/library: {} statements for 8 rows is not a small constant: {many:?}",
		many.len()
	);
}

/// List batch serialization must match the single-item path's manga_json field by field:
/// covers all edges — no categories / not in library (with an in_library=0 row) / never shelved (no manga_user row) / no chapters
#[tokio::test]
async fn batch_manga_json_matches_single_item_output() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let source_id = seed_source(&pool).await;

	// category links are deliberately inserted in reverse category_id order: if the two sides' query plans disagree on categoryIds order it shows up immediately
	let mut category_ids = Vec::new();
	for (sort_order, name) in [(0, "cat-a"), (1, "cat-b")] {
		sqlx::query("INSERT INTO category (user_id, name, sort_order) VALUES (1, ?, ?)")
			.bind(name)
			.bind(sort_order)
			.execute(&pool)
			.await
			.expect("insert category");
		let (id,): (i64,) = sqlx::query_as("SELECT id FROM category WHERE name = ?")
			.bind(name)
			.fetch_one(&pool)
			.await
			.expect("category id");
		category_ids.push(id);
	}

	// A: in library, two categories, 3 chapters, progress on the first chapter
	let (a, a_chapters) = seed_manga(&pool, source_id, "a", 3, 10_000).await;
	put_in_library(&pool, a, true, 111).await;
	for category_id in category_ids.iter().rev() {
		sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
			.bind(category_id)
			.bind(a)
			.execute(&pool)
			.await
			.expect("insert category_manga");
	}
	put_progress(&pool, a_chapters[0], 2, 5000).await;

	// B: in library, no categories, no chapters
	let (b, _) = seed_manga(&pool, source_id, "b", 0, 20_000).await;
	put_in_library(&pool, b, true, 222).await;

	// C: leftover in_library=0 row after unshelving, 1 chapter, has progress
	let (c, c_chapters) = seed_manga(&pool, source_id, "c", 1, 30_000).await;
	put_in_library(&pool, c, false, 333).await;
	put_progress(&pool, c_chapters[0], 1, 6000).await;

	// D: never shelved (no manga_user row), 1 chapter, has progress
	let (d, d_chapters) = seed_manga(&pool, source_id, "d", 1, 40_000).await;
	put_progress(&pool, d_chapters[0], 1, 7000).await;

	// the single-item path's output (still manga_json) is the baseline
	let mut singles = std::collections::HashMap::new();
	for id in [a, b, c, d] {
		let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{id}"), None).await;
		assert_eq!(status, StatusCode::OK);
		singles.insert(id, body);
	}
	// reconcile the baseline's own edge fields
	assert_eq!(singles[&b]["chapterCount"], json!(0));
	assert_eq!(singles[&b]["inLibrary"], json!(true));
	assert_eq!(singles[&b]["categoryIds"], json!([]));
	assert_eq!(singles[&c]["inLibrary"], json!(false));
	assert_eq!(singles[&d]["inLibrary"], json!(false));
	assert_eq!(singles[&a]["categoryIds"].as_array().expect("categoryIds").len(), 2);

	// library: entry = single-item object + the three keys inLibrary/inLibraryAt/unreadCount
	let (status, body) = request(&app, "GET", "/api/v1/library?sort=title", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 2);
	let mut expected_a = singles[&a].clone();
	expected_a["inLibrary"] = json!(true);
	expected_a["inLibraryAt"] = json!(111);
	// A's three chapters are all unread by the default-unread count (one flagged row, two never touched)
	expected_a["unreadCount"] = json!(3);
	assert_eq!(entries[0], expected_a);
	let mut expected_b = singles[&b].clone();
	expected_b["inLibrary"] = json!(true);
	expected_b["inLibraryAt"] = json!(222);
	expected_b["unreadCount"] = json!(0);
	assert_eq!(entries[1], expected_b);

	// history: the manga objects are identical to single-item queries, including the two not in library
	let (status, body) = request(&app, "GET", "/api/v1/history", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 3);
	assert_eq!(entries[0]["manga"], singles[&d]);
	assert_eq!(entries[1]["manga"], singles[&c]);
	assert_eq!(entries[2]["manga"], singles[&a]);

	// update/recent: only in-library chapters (A's 3), manga objects identical to single-item queries
	let (status, body) = request(&app, "GET", "/api/v1/update/recent", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 3);
	for entry in entries {
		assert_eq!(entry["manga"], singles[&a]);
	}
}
