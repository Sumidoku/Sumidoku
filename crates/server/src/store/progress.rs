//! Writes to `chapter_user`: reading progress and the downloaded flag for each chapter.

use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::model::ChapterProgress;
use crate::sync::journal;

#[derive(sqlx::FromRow)]
struct Row {
	last_page_read: i64,
	is_read: bool,
	is_bookmarked: bool,
	last_read_at: Option<i64>,
	is_downloaded: bool,
	last_modified_at: i64,
}

impl Row {
	fn same_progress(&self, other: &Row) -> bool {
		self.last_page_read == other.last_page_read
			&& self.is_read == other.is_read
			&& self.is_bookmarked == other.is_bookmarked
			&& self.last_read_at == other.last_read_at
	}

	/// A row that only marks a download holds nothing peers track
	fn is_local_only(&self) -> bool {
		self.is_downloaded && self.last_page_read == 0 && !self.is_read && !self.is_bookmarked && self.last_read_at.is_none()
	}
}

async fn row(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64) -> ApiResult<Option<Row>> {
	Ok(sqlx::query_as(
		"SELECT last_page_read, is_read, is_bookmarked, last_read_at, is_downloaded, last_modified_at
		 FROM chapter_user WHERE chapter_id = ? AND user_id = ?",
	)
	.bind(chapter_id)
	.bind(user_id)
	.fetch_optional(conn)
	.await?)
}

async fn emit(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, floor: i64) -> ApiResult<()> {
	let key = format!("{chapter_id}:{user_id}");
	let version = journal::next_version(conn, Some(user_id), "chapter_user", &key, floor).await?;
	set_modified_at(&mut *conn, chapter_id, user_id, version).await?;
	journal::parents_of_progress(conn, user_id, chapter_id).await?;
	journal::progress(conn, chapter_id, user_id, None).await
}

/// Publishes a new version if the write changed anything peers see
async fn publish_change(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, before: Option<Row>) -> ApiResult<()> {
	let Some(after) = row(conn, chapter_id, user_id).await? else { return Ok(()) };
	match before {
		None if after.is_local_only() => Ok(()),
		None => emit(conn, chapter_id, user_id, 0).await,
		Some(before) if !before.same_progress(&after) => emit(conn, chapter_id, user_id, before.last_modified_at + 1).await,
		Some(_) => Ok(()),
	}
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64) -> ApiResult<()> {
	let Some(row) = row(conn, chapter_id, user_id).await? else { return Ok(()) };
	let key = format!("{chapter_id}:{user_id}");
	if row.last_modified_at == 0 || row.last_modified_at == journal::latest_version(conn, Some(user_id), "chapter_user", &key).await? {
		return emit(conn, chapter_id, user_id, row.last_modified_at + 1).await;
	}
	journal::parents_of_progress(conn, user_id, chapter_id).await?;
	journal::progress(conn, chapter_id, user_id, None).await
}

/// Replaces progress outright. `is_downloaded` is left alone: it points at local files, which
/// neither a backup nor another device knows about.
pub async fn set(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, progress: &ChapterProgress) -> ApiResult<()> {
	let before = row(conn, chapter_id, user_id).await?;
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
	.execute(&mut *conn)
	.await?;
	publish_change(conn, chapter_id, user_id, before).await
}

/// Combines with existing progress so nothing goes backwards: page and read time take the newer
/// value, and the read and bookmarked flags stay set once set.
pub(crate) async fn merge(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, progress: &ChapterProgress) -> ApiResult<()> {
	let before = row(conn, chapter_id, user_id).await?;
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
	.execute(&mut *conn)
	.await?;
	publish_change(conn, chapter_id, user_id, before).await
}

/// Resets progress in place: the rows also carry the downloaded flag, so they are not deleted
pub(crate) async fn reset_manga(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<()> {
	let chapter_ids: Vec<i64> = sqlx::query_scalar(
		"SELECT cu.chapter_id FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		 WHERE cu.user_id = ? AND c.manga_id = ? ORDER BY cu.chapter_id",
	)
	.bind(user_id)
	.bind(manga_id)
	.fetch_all(&mut *conn)
	.await?;
	for chapter_id in chapter_ids {
		let before = row(conn, chapter_id, user_id).await?;
		sqlx::query(
			"UPDATE chapter_user SET last_page_read = 0, is_read = 0, is_bookmarked = 0, last_read_at = NULL
			 WHERE chapter_id = ? AND user_id = ?",
		)
		.bind(chapter_id)
		.bind(user_id)
		.execute(&mut *conn)
		.await?;
		publish_change(conn, chapter_id, user_id, before).await?;
	}
	Ok(())
}

pub(crate) async fn set_downloaded(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64, downloaded: bool) -> ApiResult<()> {
	let before = row(conn, chapter_id, user_id).await?;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = excluded.is_downloaded",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(downloaded)
	.execute(&mut *conn)
	.await?;
	publish_change(conn, chapter_id, user_id, before).await
}

pub async fn delete(conn: &mut SqliteConnection, chapter_id: i64, user_id: i64) -> ApiResult<()> {
	if let Some(before) = row(conn, chapter_id, user_id).await? {
		journal::parents_of_progress(conn, user_id, chapter_id).await?;
		journal::progress(conn, chapter_id, user_id, Some(journal::delete_version(before.last_modified_at))).await?;
	}
	sqlx::query("DELETE FROM chapter_user WHERE chapter_id = ? AND user_id = ?")
		.bind(chapter_id)
		.bind(user_id)
		.execute(conn)
		.await?;
	Ok(())
}

pub async fn set_modified_at(
	conn: &mut SqliteConnection,
	chapter_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<sqlx::sqlite::SqliteQueryResult> {
	Ok(sqlx::query("UPDATE chapter_user SET last_modified_at = ? WHERE chapter_id = ? AND user_id = ?")
		.bind(modified_at)
		.bind(chapter_id)
		.bind(user_id)
		.execute(conn)
		.await?)
}
