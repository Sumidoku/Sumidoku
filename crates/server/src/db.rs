use crate::error::Error;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::str::FromStr;

pub async fn create_pool(database_url: &str) -> Result<SqlitePool, Error> {
	// The parent directory of the file path after sqlite:// must exist first, otherwise rwc cannot create the DB
	let path_part = database_url
		.strip_prefix("sqlite://")
		.unwrap_or(database_url)
		.split('?')
		.next()
		.unwrap_or_default();
	if !path_part.is_empty() && path_part != ":memory:"
		&& let Some(parent) = std::path::Path::new(path_part).parent()
		&& !parent.as_os_str().is_empty()
	{
		std::fs::create_dir_all(parent)?;
	}
	let options = SqliteConnectOptions::from_str(database_url)?
		.create_if_missing(true)
		.journal_mode(SqliteJournalMode::Wal)
		.foreign_keys(true);
	let pool = SqlitePoolOptions::new().connect_with(options).await?;
	sqlx::migrate!("./migrations").run(&pool).await?;
	Ok(pool)
}

/// Write-transaction entry point: when a deferred transaction reads before writing, upgrading to a write lock
/// against a concurrent commit hits SQLITE_BUSY_SNAPSHOT (the snapshot is stale and busy_timeout does not cover it);
/// IMMEDIATE queues for the write lock at BEGIN, so the transaction's read snapshot is necessarily writable
pub async fn begin_write(
	pool: &SqlitePool,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, sqlx::Error> {
	pool.begin_with("BEGIN IMMEDIATE").await
}
