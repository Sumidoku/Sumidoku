use super::{SyncChange, insert_change};
use crate::error::ApiResult;
use sqlx::SqlitePool;
use std::ops::DerefMut;

#[derive(Debug)]
pub struct CompactResult {
	pub base_cursor: i64,
	pub removed: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct BaselineRow {
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

async fn baseline_rows(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	prefer_event_snapshots: bool,
) -> ApiResult<Vec<BaselineRow>> {
	let tx = tx.deref_mut();
	sqlx::query("DROP TABLE IF EXISTS temp.sync_compaction_options")
		.execute(&mut *tx)
		.await?;
	sqlx::query("CREATE TEMP TABLE sync_compaction_options AS SELECT ? AS prefer_event_snapshots")
		.bind(prefer_event_snapshots)
		.execute(&mut *tx)
		.await?;
	sqlx::query("DROP TABLE IF EXISTS temp.sync_compaction_latest")
		.execute(&mut *tx)
		.await?;
	sqlx::query("DROP TABLE IF EXISTS temp.sync_compaction_baseline")
		.execute(&mut *tx)
		.await?;
	sqlx::query(
		"CREATE TEMP TABLE sync_compaction_latest AS
		 SELECT user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key, origin_sequence,
		  deleted, modified_at, identity, snapshot,
		  ROW_NUMBER() OVER (PARTITION BY user_id, entity_kind, entity_key ORDER BY sequence DESC) AS rn
		 FROM sync_change",
	)
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		"CREATE TEMP TABLE sync_compaction_baseline (
		 sort_order INTEGER NOT NULL,
		 user_id INTEGER NOT NULL,
		 entity_kind TEXT NOT NULL,
		 entity_key TEXT NOT NULL,
		 origin TEXT NOT NULL,
		 origin_user_id INTEGER NOT NULL,
		 origin_entity_key TEXT NOT NULL,
		 origin_sequence INTEGER NOT NULL,
		 deleted INTEGER NOT NULL,
		 modified_at INTEGER NOT NULL,
		 identity TEXT NOT NULL,
		 snapshot TEXT NOT NULL
		 )",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 1, fans.user_id, 'manga', CAST(sm.manga_id AS TEXT),
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, sm.last_modified_at,
		  CASE WHEN l.deleted = 0 AND l.modified_at = sm.last_modified_at THEN l.identity ELSE sm.identity END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = sm.last_modified_at THEN l.snapshot ELSE sm.snapshot END
		 FROM sync_manga_state sm
		 JOIN (
		  SELECT manga_id, user_id FROM manga_user
		  UNION
		  SELECT c.manga_id, cu.user_id FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		  UNION
		  SELECT manga_id, user_id FROM category_manga
		 ) fans ON fans.manga_id = sm.manga_id
		 JOIN user u ON u.id = fans.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = fans.user_id
		  AND l.entity_kind = 'manga' AND l.entity_key = CAST(sm.manga_id AS TEXT)
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 2, mu.user_id, 'manga_user', mu.manga_id || ':' || mu.user_id,
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, mu.last_modified_at,
		  CASE WHEN l.deleted = 0 AND l.modified_at = mu.last_modified_at THEN l.identity
		   ELSE json_set(sm.identity, '$.userId', mu.user_id) END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = mu.last_modified_at THEN l.snapshot
		   ELSE json_object(
		    'mangaId', mu.manga_id, 'userId', mu.user_id, 'inLibrary', mu.in_library,
		    'inLibraryAt', mu.in_library_at, 'viewerFlags', mu.viewer_flags, 'sortKey', mu.sort_key,
		    'incognito', mu.incognito, 'lastModifiedAt', mu.last_modified_at
		   ) END
		 FROM manga_user mu
		 JOIN sync_manga_state sm ON sm.manga_id = mu.manga_id
		 JOIN user u ON u.id = mu.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = mu.user_id
		  AND l.entity_kind = 'manga_user' AND l.entity_key = mu.manga_id || ':' || mu.user_id
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 3, c.user_id, 'category', CAST(c.id AS TEXT),
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, c.last_modified_at,
		  CASE WHEN l.deleted = 0 AND l.modified_at = c.last_modified_at THEN l.identity
		   ELSE json_object('categoryId', c.id, 'userId', c.user_id) END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = c.last_modified_at THEN json_set(l.snapshot, '$.includeInDownload', 0)
		   ELSE json_object(
		    'id', c.id, 'userId', c.user_id, 'name', c.name, 'sortOrder', c.sort_order,
		    'isDefault', c.is_default, 'includeInDownload', 0,
		    'lastModifiedAt', c.last_modified_at
		   ) END
		 FROM category c
		 JOIN user u ON u.id = c.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = c.user_id
		  AND l.entity_kind = 'category' AND l.entity_key = CAST(c.id AS TEXT)
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 4, fans.user_id, 'chapter', CAST(scs.chapter_id AS TEXT),
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER),
		  CASE WHEN l.deleted = 0 AND l.modified_at = CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER)
		   THEN l.identity ELSE scs.identity END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER)
		   THEN l.snapshot ELSE scs.snapshot END
		 FROM sync_chapter_state scs
		 JOIN (
		  SELECT c.id AS chapter_id, mu.user_id FROM chapter c JOIN manga_user mu ON mu.manga_id = c.manga_id
		  UNION
		  SELECT chapter_id, user_id FROM chapter_user
		  UNION
		  SELECT c.id AS chapter_id, cm.user_id FROM chapter c JOIN category_manga cm ON cm.manga_id = c.manga_id
		 ) fans ON fans.chapter_id = scs.chapter_id
		 JOIN user u ON u.id = fans.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = fans.user_id
		  AND l.entity_kind = 'chapter' AND l.entity_key = CAST(scs.chapter_id AS TEXT)
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 5, cu.user_id, 'chapter_user', cu.chapter_id || ':' || cu.user_id,
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, cu.last_modified_at,
		  CASE WHEN l.deleted = 0 AND l.modified_at = cu.last_modified_at THEN l.identity
		   ELSE json_set(scs.identity, '$.userId', cu.user_id) END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = cu.last_modified_at THEN l.snapshot
		   ELSE json_object(
		    'chapterId', cu.chapter_id, 'userId', cu.user_id, 'lastPageRead', cu.last_page_read,
		    'isRead', cu.is_read, 'isBookmarked', cu.is_bookmarked, 'lastReadAt', cu.last_read_at,
		    'lastModifiedAt', cu.last_modified_at
		   ) END
		 FROM chapter_user cu
		 JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
		 JOIN user u ON u.id = cu.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = cu.user_id
		  AND l.entity_kind = 'chapter_user' AND l.entity_key = cu.chapter_id || ':' || cu.user_id
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query(
		"INSERT INTO sync_compaction_baseline
		 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		  origin_sequence, deleted, modified_at, identity, snapshot)
		 SELECT 6, cm.user_id, 'category_manga', cm.category_id || ':' || cm.manga_id || ':' || cm.user_id,
		  CASE WHEN l.deleted = 0 THEN l.origin ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_user_id ELSE 0 END,
		  CASE WHEN l.deleted = 0 THEN l.origin_entity_key ELSE '' END,
		  CASE WHEN l.deleted = 0 THEN l.origin_sequence ELSE 0 END, 0, cm.last_modified_at,
		  CASE WHEN l.deleted = 0 AND l.modified_at = cm.last_modified_at THEN l.identity
		   ELSE json_set(sm.identity, '$.categoryId', cm.category_id, '$.userId', cm.user_id) END,
		  CASE WHEN l.deleted = 0 AND l.modified_at = cm.last_modified_at THEN l.snapshot
		   ELSE json_object(
		    'categoryId', cm.category_id, 'mangaId', cm.manga_id, 'userId', cm.user_id,
		    'lastModifiedAt', cm.last_modified_at
		   ) END
		 FROM category_manga cm
		 JOIN sync_manga_state sm ON sm.manga_id = cm.manga_id
		 JOIN user u ON u.id = cm.user_id
		 LEFT JOIN sync_compaction_latest l ON l.rn = 1 AND l.user_id = cm.user_id
		  AND l.entity_kind = 'category_manga'
		  AND l.entity_key = cm.category_id || ':' || cm.manga_id || ':' || cm.user_id
		  AND (SELECT prefer_event_snapshots FROM temp.sync_compaction_options)",
	)
	.execute(&mut *tx)
	.await?;

	for (sort_order, kind, existence) in [
		(1, "manga", "NOT EXISTS (SELECT 1 FROM manga t WHERE CAST(t.id AS TEXT) = l.entity_key)"),
		(2, "manga_user", "NOT EXISTS (SELECT 1 FROM manga_user t WHERE t.manga_id || ':' || t.user_id = l.entity_key AND t.user_id = l.user_id)"),
		(3, "category", "NOT EXISTS (SELECT 1 FROM category t WHERE CAST(t.id AS TEXT) = l.entity_key AND t.user_id = l.user_id)"),
		(4, "chapter", "NOT EXISTS (SELECT 1 FROM chapter t WHERE CAST(t.id AS TEXT) = l.entity_key)"),
		(5, "chapter_user", "NOT EXISTS (SELECT 1 FROM chapter_user t WHERE t.chapter_id || ':' || t.user_id = l.entity_key AND t.user_id = l.user_id)"),
		(6, "category_manga", "NOT EXISTS (SELECT 1 FROM category_manga t WHERE t.category_id || ':' || t.manga_id || ':' || t.user_id = l.entity_key AND t.user_id = l.user_id)"),
	] {
		let snapshot = if kind == "category" {
			"json_set(l.snapshot, '$.includeInDownload', 0)"
		} else {
			"l.snapshot"
		};
		let sql = format!(
			"INSERT INTO sync_compaction_baseline
			 (sort_order, user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
			  origin_sequence, deleted, modified_at, identity, snapshot)
			 SELECT {sort_order}, l.user_id, l.entity_kind, l.entity_key, l.origin, l.origin_user_id,
			  l.origin_entity_key, l.origin_sequence, l.deleted, l.modified_at, l.identity, {snapshot}
			 FROM sync_compaction_latest l
			 WHERE l.rn = 1 AND l.entity_kind = '{kind}' AND l.deleted = 1 AND {existence}"
		);
		sqlx::query(&sql).execute(&mut *tx).await?;
	}

	let rows = sqlx::query_as::<_, BaselineRow>(
		"SELECT user_id, entity_kind, entity_key, origin, origin_user_id, origin_entity_key,
		 origin_sequence, deleted, modified_at, identity, snapshot
		 FROM sync_compaction_baseline ORDER BY sort_order, user_id, entity_key, modified_at, origin, origin_sequence",
	)
	.fetch_all(&mut *tx)
	.await?;
	Ok(rows)
}

pub async fn compact(pool: &SqlitePool) -> ApiResult<CompactResult> {
	let mut tx = crate::db::begin_write(pool).await?;
	let rows = baseline_rows(&mut tx, true).await?;
	let removed = sqlx::query("DELETE FROM sync_change")
		.execute(&mut *tx)
		.await?
		.rows_affected() as i64;
	let mut base_cursor = 0;
	for row in rows {
		let change = SyncChange {
			sequence: 0,
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
		};
		let sequence = insert_change(&mut tx, &change).await?;
		if base_cursor == 0 {
			base_cursor = sequence;
		}
	}
	if base_cursor == 0 {
		base_cursor = sqlx::query_scalar(
			"SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'sync_change'), 0)",
		)
		.fetch_one(&mut *tx)
		.await?;
	}
	sqlx::query("UPDATE sync_checkpoint SET base_sequence = ? WHERE id = 1")
		.bind(base_cursor)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(CompactResult { base_cursor, removed })
}

/// Ownership migration must issue a newer local version for every current row and tombstone.
pub async fn rebaseline(pool: &SqlitePool) -> ApiResult<()> {
	let mut tx = crate::db::begin_write(pool).await?;
	let rows = baseline_rows(&mut tx, false).await?;
	for mut row in rows {
		let previous: Option<i64> = sqlx::query_scalar(
			"SELECT MAX(modified_at) FROM sync_change
			 WHERE user_id = ? AND entity_kind = ? AND entity_key = ?",
		)
		.bind(row.user_id)
		.bind(&row.entity_kind)
		.bind(&row.entity_key)
		.fetch_one(&mut *tx)
		.await?;
		row.modified_at = previous
			.unwrap_or(row.modified_at)
			.max(row.modified_at)
			.checked_add(1)
			.ok_or_else(|| crate::error::ApiError::internal("sync modification time overflow"))?;
		let mut snapshot: serde_json::Value = serde_json::from_str(&row.snapshot)?;
		if !row.deleted {
			super::apply::set_final_version(&mut tx, &row.entity_kind, &row.entity_key, row.modified_at).await?;
			snapshot["lastModifiedAt"] = row.modified_at.into();
		}
		let change = SyncChange {
			sequence: 0,
			user_id: row.user_id,
			entity_kind: row.entity_kind,
			entity_key: row.entity_key,
			origin: String::new(),
			origin_user_id: 0,
			origin_entity_key: String::new(),
			origin_sequence: 0,
			deleted: row.deleted,
			modified_at: row.modified_at,
			identity: serde_json::from_str(&row.identity)?,
			snapshot,
		};
		insert_change(&mut tx, &change).await?;
	}
	sqlx::query("DELETE FROM setting WHERE key = 'sync.single_owner_rebaseline'")
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(())
}
