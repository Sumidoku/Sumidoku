//! Deterministic reconciliation of BEGIN IMMEDIATE write transactions (the CI flake mechanism of #222):
//! a deferred transaction reads first and writes later; if its snapshot has been rewritten by a
//! concurrent commit when it upgrades to a write lock, it always hits SQLITE_BUSY_SNAPSHOT
//! (extended 517, not covered by busy_timeout) — the cause of the intermittent 500s in
//! update_chapter; begin_write's IMMEDIATE transaction has no such window under the same interleaving.
mod common;

use sumidoku_server::db::{begin_write, create_pool};

async fn migrated_pool() -> (tempfile::TempDir, sqlx::SqlitePool) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let url = format!("sqlite://{}", tmp.path().join("t.db").display());
	let pool = create_pool(&url).await.expect("create pool");
	sqlx::query("INSERT INTO setting (key, value) VALUES ('k', '0')")
		.execute(&pool)
		.await
		.expect("seed row");
	(tmp, pool)
}

/// Concurrent committer: another connection rewrites the seed row (the same role as the download worker's state flip)
async fn concurrent_commit(pool: &sqlx::SqlitePool) {
	sqlx::query("UPDATE setting SET value = '1' WHERE key = 'k'")
		.execute(pool)
		.await
		.expect("concurrent commit");
}

#[tokio::test]
async fn deferred_read_then_write_hits_busy_snapshot() {
	let (_tmp, pool) = migrated_pool().await;
	let mut tx = pool.begin().await.expect("deferred tx");
	let _: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'k'")
		.fetch_optional(&mut *tx)
		.await
		.expect("read in tx");
	concurrent_commit(&pool).await;
	let err = sqlx::query("INSERT INTO setting (key, value) VALUES ('k2', 'x')")
		.execute(&mut *tx)
		.await
		.expect_err("write on a stale snapshot must fail");
	let code = err
		.as_database_error()
		.and_then(|e| e.code())
		.expect("sqlite error code");
	assert_eq!(code.as_ref(), "517", "SQLITE_BUSY_SNAPSHOT");
}

#[tokio::test]
async fn immediate_write_tx_survives_the_same_interleaving() {
	let (_tmp, pool) = migrated_pool().await;
	let mut tx = begin_write(&pool).await.expect("immediate tx");
	let _: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'k'")
		.fetch_optional(&mut *tx)
		.await
		.expect("read in tx");
	// While the IMMEDIATE transaction holds its lock, concurrent writers just queue within busy_timeout instead of producing 500-class errors
	let other = pool.clone();
	let waiter = tokio::spawn(async move { concurrent_commit(&other).await });
	sqlx::query("INSERT INTO setting (key, value) VALUES ('k2', 'x')")
		.execute(&mut *tx)
		.await
		.expect("immediate tx write must succeed");
	tx.commit().await.expect("commit");
	waiter.await.expect("concurrent writer settles");
}
