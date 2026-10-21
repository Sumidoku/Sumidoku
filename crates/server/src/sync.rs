pub mod apply;
pub mod compact;

use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq)]
pub struct SyncChange {
	pub sequence: i64,
	pub user_id: i64,
	pub entity_kind: String,
	pub entity_key: String,
	pub origin: String,
	pub origin_user_id: i64,
	pub origin_entity_key: String,
	pub origin_sequence: i64,
	pub deleted: bool,
	/// Non-delete events carry the snapshot version; delete events carry deletion time while the snapshot keeps the prior version.
	pub modified_at: i64,
	pub identity: Value,
	pub snapshot: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireChange {
	pub origin: String,
	pub origin_user_id: i64,
	pub origin_sequence: i64,
	pub entity_kind: String,
	pub entity_key: String,
	pub deleted: bool,
	pub modified_at: i64,
	pub identity: Value,
	pub snapshot: Value,
}

impl From<SyncChange> for WireChange {
	fn from(change: SyncChange) -> Self {
		Self {
			origin: change.origin,
			origin_user_id: change.origin_user_id,
			origin_sequence: change.origin_sequence,
			entity_kind: change.entity_kind,
			entity_key: change.origin_entity_key,
			deleted: change.deleted,
			modified_at: change.modified_at,
			identity: change.identity,
			snapshot: change.snapshot,
		}
	}
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
	origin: String,
	origin_user_id: i64,
	origin_entity_key: String,
	origin_sequence: i64,
	deleted: bool,
	modified_at: i64,
	identity: String,
	snapshot: String,
}

fn into_change(row: SyncChangeRow) -> ApiResult<SyncChange> {
	Ok(SyncChange {
		sequence: row.sequence,
		user_id: row.user_id,
		entity_kind: row.entity_kind,
		entity_key: row.entity_key,
		origin: row.origin,
		origin_user_id: row.origin_user_id,
		origin_entity_key: row.origin_entity_key,
		origin_sequence: row.origin_sequence,
		deleted: row.deleted,
		modified_at: row.modified_at,
		identity: serde_json::from_str(&row.identity)?,
		snapshot: serde_json::from_str(&row.snapshot)?,
	})
}

pub async fn instance_id(pool: &SqlitePool) -> ApiResult<String> {
	Ok(sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(pool)
		.await?)
}

pub async fn changes_since(
	pool: &SqlitePool,
	authenticated_user_id: i64,
	cursor: i64,
	limit: i64,
) -> ApiResult<SyncChangePage> {
	changes_since_filtered(pool, authenticated_user_id, None, cursor, limit).await
}

pub async fn changes_since_for_peer(
	pool: &SqlitePool,
	authenticated_user_id: i64,
	peer_origin: &str,
	cursor: i64,
	limit: i64,
) -> ApiResult<SyncChangePage> {
	changes_since_filtered(pool, authenticated_user_id, Some(peer_origin), cursor, limit).await
}

async fn changes_since_filtered(
	pool: &SqlitePool,
	authenticated_user_id: i64,
	peer_origin: Option<&str>,
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
	let base_sequence: i64 = sqlx::query_scalar("SELECT base_sequence FROM sync_checkpoint WHERE id = 1")
		.fetch_one(&mut *tx)
		.await?;
	if base_sequence != 0 && cursor != 0 && cursor < base_sequence {
		return Err(ApiError::resync_required(
			"sync cursor is behind the compacted baseline; retry from cursor zero",
		));
	}
	let high_watermark: i64 = sqlx::query_scalar(
		"SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)",
	)
	.fetch_one(&mut *tx)
	.await?;
	if cursor > high_watermark {
		return Err(ApiError::bad_request("cursor is ahead of the change log"));
	}
	let mut rows = sqlx::query_as::<_, SyncChangeRow>(
		"SELECT sequence, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		 origin_sequence, deleted, modified_at, identity, snapshot
		 FROM sync_change
		 WHERE user_id = ? AND sequence > ? AND (? IS NULL OR origin <> ?)
		 ORDER BY sequence
		 LIMIT ?",
	)
	.bind(authenticated_user_id)
	.bind(cursor)
	.bind(peer_origin)
	.bind(peer_origin)
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
		changes.push(into_change(row)?);
	}
	Ok(SyncChangePage {
		changes,
		next_cursor,
		has_more,
	})
}

pub(crate) async fn changes_after(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	sequence: i64,
) -> ApiResult<Vec<SyncChange>> {
	let rows = sqlx::query_as::<_, SyncChangeRow>(
		"SELECT sequence, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		 origin_sequence, deleted, modified_at, identity, snapshot
		 FROM sync_change WHERE sequence > ? ORDER BY sequence",
	)
	.bind(sequence)
	.fetch_all(&mut **tx)
	.await?;
	rows.into_iter().map(into_change).collect()
}

pub(crate) async fn insert_change(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	change: &SyncChange,
) -> ApiResult<i64> {
	Ok(sqlx::query_scalar(
		"INSERT INTO sync_change
		 (user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key, origin_sequence,
		  deleted, modified_at, identity, snapshot)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING sequence",
	)
	.bind(change.user_id)
	.bind(&change.entity_kind)
	.bind(&change.entity_key)
	.bind(&change.origin)
	.bind(change.origin_user_id)
	.bind(&change.origin_entity_key)
	.bind(change.origin_sequence)
	.bind(change.deleted)
	.bind(change.modified_at)
	.bind(serde_json::to_string(&change.identity)?)
	.bind(serde_json::to_string(&change.snapshot)?)
	.fetch_one(&mut **tx)
	.await?)
}
