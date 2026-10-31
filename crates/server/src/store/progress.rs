//! Writes to `chapter_user`: reading progress and the downloaded flag for each chapter.

use sqlx::{Executor, Sqlite};

use crate::error::ApiResult;
use crate::model::ChapterProgress;

/// Replaces progress outright. `is_downloaded` is left alone: it points at local files, which
/// neither a backup nor another device knows about.
pub(crate) async fn set<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	chapter_id: i64,
	user_id: i64,
	progress: &ChapterProgress,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, ?, ?, ?, ?, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
		 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(progress.last_page_read)
	.bind(progress.is_read)
	.bind(progress.is_bookmarked)
	.bind(progress.last_read_at)
	.execute(db)
	.await?;
	Ok(())
}

/// Combines with existing progress so nothing goes backwards: page and read time take the newer
/// value, and the read and bookmarked flags stay set once set.
pub(crate) async fn merge<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	chapter_id: i64,
	user_id: i64,
	progress: &ChapterProgress,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
		 VALUES (?, ?, ?, ?, ?, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = MAX(last_page_read, excluded.last_page_read),
		 is_read = MAX(is_read, excluded.is_read),
		 is_bookmarked = MAX(is_bookmarked, excluded.is_bookmarked),
		 last_read_at = CASE
		 WHEN excluded.last_read_at IS NULL THEN last_read_at
		 WHEN last_read_at IS NULL THEN excluded.last_read_at
		 ELSE MAX(last_read_at, excluded.last_read_at) END",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(progress.last_page_read)
	.bind(progress.is_read)
	.bind(progress.is_bookmarked)
	.bind(progress.last_read_at)
	.execute(db)
	.await?;
	Ok(())
}

/// Resets progress in place: the rows also carry the downloaded flag, so they are not deleted
pub(crate) async fn reset_manga<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query(
		"UPDATE chapter_user SET last_page_read = 0, is_read = 0, is_bookmarked = 0, last_read_at = NULL
		 WHERE user_id = ? AND chapter_id IN (SELECT id FROM chapter WHERE manga_id = ?)",
	)
	.bind(user_id)
	.bind(manga_id)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn set_downloaded<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	chapter_id: i64,
	user_id: i64,
	downloaded: bool,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = excluded.is_downloaded",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(downloaded)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn delete<'e, E: Executor<'e, Database = Sqlite>>(db: E, chapter_id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM chapter_user WHERE chapter_id = ? AND user_id = ?")
		.bind(chapter_id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_modified_at<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	chapter_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<sqlx::sqlite::SqliteQueryResult> {
	Ok(sqlx::query("UPDATE chapter_user SET last_modified_at = ? WHERE chapter_id = ? AND user_id = ?")
		.bind(modified_at)
		.bind(chapter_id)
		.bind(user_id)
		.execute(db)
		.await?)
}
