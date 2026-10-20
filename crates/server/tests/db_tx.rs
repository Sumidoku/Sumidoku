//! 写事务 BEGIN IMMEDIATE 的确定性对账（#222 的 CI flake 机制）：
//! deferred 事务先读后写，升级写锁时若快照已被并发提交改写，必撞
//! SQLITE_BUSY_SNAPSHOT（extended 517，busy_timeout 不覆盖）——update_chapter
//! 偶发 500 的成因；同一交错下 begin_write 的 IMMEDIATE 事务无此窗口。
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

/// 并发提交者：另一连接把种子行改写（下载 worker 状态翻转同款角色）
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
	// IMMEDIATE 事务持锁期间，并发写者只会在 busy_timeout 内排队，不再产出 500 类错误
	let other = pool.clone();
	let waiter = tokio::spawn(async move { concurrent_commit(&other).await });
	sqlx::query("INSERT INTO setting (key, value) VALUES ('k2', 'x')")
		.execute(&mut *tx)
		.await
		.expect("immediate tx write must succeed");
	tx.commit().await.expect("commit");
	waiter.await.expect("concurrent writer settles");
}
