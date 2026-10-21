use crate::error::{ApiError, ApiResult};
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq)]
pub struct SyncChange {
	pub sequence: i64,
	pub user_id: i64,
	pub entity_kind: String,
	pub entity_key: String,
	pub deleted: bool,
	/// Non-delete events carry the snapshot version; delete events carry deletion time while the snapshot keeps the prior version.
	pub modified_at: i64,
	pub identity: Value,
	pub snapshot: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncChangePage {
	pub changes: Vec<SyncChange>,
	pub next_cursor: i64,
	pub has_more: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct SyncChangeRow {
	sequence: i64,
	user_id: i64,
	entity_kind: String,
	entity_key: String,
	deleted: bool,
	modified_at: i64,
	identity: String,
	snapshot: String,
}

pub async fn changes_since(
	pool: &SqlitePool,
	authenticated_user_id: i64,
	cursor: i64,
	limit: i64,
) -> ApiResult<SyncChangePage> {
	if cursor < 0 {
		return Err(ApiError::bad_request("cursor must not be negative"));
	}
	if limit < 1 {
		return Err(ApiError::bad_request("limit must be greater than zero"));
	}
	let limit_usize = usize::try_from(limit).map_err(|_| ApiError::bad_request("limit is too large"))?;
	let fetch_limit = limit.checked_add(1).ok_or_else(|| ApiError::bad_request("limit is too large"))?;
	let mut tx = pool.begin_with("BEGIN").await?;
	let high_watermark: i64 = sqlx::query_scalar(
		"SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)",
	)
	.fetch_one(&mut *tx)
	.await?;
	if cursor > high_watermark {
		return Err(ApiError::bad_request("cursor is ahead of the change log"));
	}
	let mut rows = sqlx::query_as::<_, SyncChangeRow>(
		"SELECT sequence, user_id, entity_kind, entity_key, deleted, modified_at, identity, snapshot
		 FROM sync_change
		 WHERE user_id = ? AND sequence > ?
		 ORDER BY sequence
		 LIMIT ?",
	)
	.bind(authenticated_user_id)
	.bind(cursor)
	.bind(fetch_limit)
	.fetch_all(&mut *tx)
	.await?;
	tx.commit().await?;

	let has_more = rows.len() > limit_usize;
	rows.truncate(limit_usize);
	let next_cursor = if has_more {
		rows.last().map(|row| row.sequence).unwrap_or(cursor)
	} else {
		high_watermark
	};
	let mut changes = Vec::with_capacity(rows.len());
	for row in rows {
		changes.push(SyncChange {
			sequence: row.sequence,
			user_id: row.user_id,
			entity_kind: row.entity_kind,
			entity_key: row.entity_key,
			deleted: row.deleted,
			modified_at: row.modified_at,
			identity: serde_json::from_str(&row.identity)?,
			snapshot: serde_json::from_str(&row.snapshot)?,
		});
	}
	Ok(SyncChangePage {
		changes,
		next_cursor,
		has_more,
	})
}
