//! Instance state only the server writes. It lives apart from the `setting` table, so the settings
//! API never shows it and backups never carry it.

use sqlx::{Executor, Sqlite, SqlitePool};

use crate::error::ApiResult;

pub(crate) const INSTANCE_ID: &str = "sync.instance_id";
pub(crate) const SINGLE_OWNER_REBASELINE: &str = "sync.single_owner_rebaseline";
pub(crate) const BACKUP_LAST_RUN: &str = "backup.last_run";
pub(crate) const LIBRARY_LAST_SCHEDULED_RUN_AT: &str = "library.last_scheduled_run_at";
/// When the last batch of either kind finished, for the summary only: the scheduled clock must not move on a manual run
pub(crate) const LIBRARY_LAST_UPDATE_FINISHED_AT: &str = "library.last_update_finished_at";
pub(crate) const LOCAL_LAST_SCAN_AT: &str = "local.last_scan_at";

/// These names lived in `setting` before migration 0032, so old backups and clients can still send
/// them; the settings write path and backup restore drop them rather than recreate a dead row.
pub(crate) const KEYS: [&str; 6] = [
	INSTANCE_ID,
	SINGLE_OWNER_REBASELINE,
	BACKUP_LAST_RUN,
	LIBRARY_LAST_SCHEDULED_RUN_AT,
	LIBRARY_LAST_UPDATE_FINISHED_AT,
	LOCAL_LAST_SCAN_AT,
];

pub(crate) async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
	sqlx::query_scalar("SELECT value FROM server_state WHERE key = ?")
		.bind(key)
		.fetch_optional(pool)
		.await
}

pub(crate) async fn clear<'e, E: Executor<'e, Database = Sqlite>>(db: E, key: &str) -> ApiResult<()> {
	sqlx::query("DELETE FROM server_state WHERE key = ?")
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn time(pool: &SqlitePool, key: &str) -> ApiResult<Option<i64>> {
	Ok(get(pool, key).await?.and_then(|raw| raw.parse().ok()))
}

pub(crate) async fn set_time(pool: &SqlitePool, key: &str, at: i64) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO server_state (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(key)
	.bind(at.to_string())
	.execute(pool)
	.await?;
	Ok(())
}
