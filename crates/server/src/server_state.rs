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
pub(crate) const OPDS_LOCATOR_KEY: &str = "opds.locator_key";

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
const PRIVATE_KEYS: [&str; 1] = [OPDS_LOCATOR_KEY];

pub(crate) fn is_internal_key(key: &str) -> bool {
	KEYS.contains(&key) || PRIVATE_KEYS.contains(&key)
}

/// Get or atomically create the per-install secret used to authenticate server-generated OPDS locators.
/// This server-only value is excluded from user settings and backup imports/exports.
pub(crate) async fn opds_locator_key(pool: &SqlitePool) -> ApiResult<Vec<u8>> {
	use base64::Engine as _;
	use rand_core::RngCore as _;
	if let Some(stored) = get(pool, OPDS_LOCATOR_KEY).await? {
		return decode_opds_locator_key(stored);
	}
	let mut generated = [0u8; 32];
	rand_core::OsRng.fill_bytes(&mut generated);
	let generated = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(generated);
	sqlx::query("INSERT OR IGNORE INTO server_state (key, value) VALUES (?, ?)")
		.bind(OPDS_LOCATOR_KEY)
		.bind(generated)
		.execute(pool)
		.await?;
	let stored = get(pool, OPDS_LOCATOR_KEY).await?.ok_or_else(|| crate::error::ApiError::internal("OPDS locator key missing"))?;
	decode_opds_locator_key(stored)
}

fn decode_opds_locator_key(stored: String) -> ApiResult<Vec<u8>> {
	use base64::Engine as _;
	let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(stored)
		.map_err(|_| crate::error::ApiError::internal("OPDS locator key is invalid"))?;
	if key.len() != 32 { return Err(crate::error::ApiError::internal("OPDS locator key has invalid length")); }
	Ok(key)
}

pub(crate) async fn get<'e, E: Executor<'e, Database = Sqlite>>(db: E, key: &str) -> Result<Option<String>, sqlx::Error> {
	sqlx::query_scalar("SELECT value FROM server_state WHERE key = ?")
		.bind(key)
		.fetch_optional(db)
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
