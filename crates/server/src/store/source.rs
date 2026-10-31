//! Deletes from `source`, which cascade to everything a peer tracks under it. Other source columns
//! are local configuration and are not journaled.

use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::sync::journal;

pub async fn delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let manga_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? ORDER BY id")
		.bind(id)
		.fetch_all(&mut *conn)
		.await?;
	let now = journal::now_ms();
	journal::append!(
		conn,
		"SELECT fans.user_id, 'manga', CAST(sm.manga_id AS TEXT), 1, MAX(?2, sm.last_modified_at + 1), sm.identity, sm.snapshot
		 FROM sync_manga_state sm
		 JOIN (
			SELECT mu.manga_id, mu.user_id FROM manga_user mu
			JOIN manga m ON m.id = mu.manga_id WHERE m.source_id = ?1
			UNION
			SELECT c.manga_id, cu.user_id FROM chapter_user cu
			JOIN chapter c ON c.id = cu.chapter_id
			JOIN manga m ON m.id = c.manga_id WHERE m.source_id = ?1
		 ) fans ON fans.manga_id = sm.manga_id
		 JOIN user u ON u.id = fans.user_id
		 WHERE sm.source_id = ?1 ORDER BY fans.user_id, sm.manga_id",
		id,
		now,
	)?;
	journal::append!(
		conn,
		"SELECT fans.user_id, 'chapter', CAST(scs.chapter_id AS TEXT), 1,
		 MAX(?2, CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER) + 1), scs.identity, scs.snapshot
		 FROM sync_chapter_state scs
		 JOIN (
			SELECT c.id AS chapter_id, mu.user_id FROM chapter c
			JOIN manga m ON m.id = c.manga_id
			JOIN manga_user mu ON mu.manga_id = c.manga_id WHERE m.source_id = ?1
			UNION
			SELECT cu.chapter_id, cu.user_id FROM chapter_user cu
			JOIN chapter c ON c.id = cu.chapter_id
			JOIN manga m ON m.id = c.manga_id WHERE m.source_id = ?1
		 ) fans ON fans.chapter_id = scs.chapter_id
		 JOIN user u ON u.id = fans.user_id
		 WHERE scs.source_id = ?1 ORDER BY fans.user_id, scs.manga_id, scs.chapter_id",
		id,
		now,
	)?;
	journal::append!(
		conn,
		"SELECT cu.user_id, 'chapter_user', cu.chapter_id || ':' || cu.user_id, 1,
		 MAX(?2, cu.last_modified_at + 1), json_set(scs.identity, '$.userId', cu.user_id),
		 json_object(
			'chapterId', cu.chapter_id,
			'userId', cu.user_id,
			'lastPageRead', cu.last_page_read,
			'isRead', cu.is_read,
			'isBookmarked', cu.is_bookmarked,
			'lastReadAt', cu.last_read_at,
			'lastModifiedAt', cu.last_modified_at
		 )
		 FROM chapter_user cu
		 JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
		 JOIN user u ON u.id = cu.user_id
		 WHERE scs.source_id = ?1 ORDER BY cu.user_id, scs.manga_id, cu.chapter_id",
		id,
		now,
	)?;
	sqlx::query("DELETE FROM sync_chapter_state WHERE source_id = ?").bind(id).execute(&mut *conn).await?;
	for manga_id in manga_ids {
		journal::cascade_library_rows(conn, manga_id).await?;
	}
	sqlx::query("DELETE FROM source WHERE id = ?").bind(id).execute(conn).await?;
	Ok(())
}
