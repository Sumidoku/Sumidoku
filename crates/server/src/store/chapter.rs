//! Writes to `chapter`.

use sqlx::sqlite::SqliteQueryResult;
use sqlx::{Executor, Sqlite};

use crate::error::ApiResult;

/// The columns a source, a backup or a sync event describes
pub(crate) struct ChapterFields<'a> {
	pub key: &'a str,
	pub title: Option<&'a str>,
	pub number: Option<f64>,
	pub volume: Option<f64>,
	/// JSON array text
	pub scanlators: &'a str,
	pub language: Option<&'a str>,
	pub thumbnail: Option<&'a str>,
	pub url: Option<&'a str>,
	pub date_uploaded: Option<i64>,
	pub source_order: i64,
	pub page_count: i64,
	pub locked: bool,
}

/// A refresh rewrites everything the source owns and revives a trashed row; `page_count` and the
/// fetch times stay with the local row.
pub(crate) async fn upsert_from_source<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	chapter: &ChapterFields<'_>,
	now: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO chapter (manga_id, key, title, number, volume, scanlators, language, thumbnail, url,
		 date_uploaded, source_order, locked, fetched_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, key) DO UPDATE SET
		 title = excluded.title, number = excluded.number, volume = excluded.volume,
		 scanlators = excluded.scanlators, language = excluded.language,
		 thumbnail = excluded.thumbnail, url = excluded.url,
		 date_uploaded = excluded.date_uploaded, source_order = excluded.source_order,
		 locked = excluded.locked, fetched_at = chapter.fetched_at, created_at = chapter.created_at,
		 trashed_at = NULL",
	)
	.bind(manga_id)
	.bind(chapter.key)
	.bind(chapter.title)
	.bind(chapter.number)
	.bind(chapter.volume)
	.bind(chapter.scanlators)
	.bind(chapter.language)
	.bind(chapter.thumbnail)
	.bind(chapter.url)
	.bind(chapter.date_uploaded)
	.bind(chapter.source_order)
	.bind(chapter.locked)
	.bind(now)
	.bind(now)
	.execute(db)
	.await?;
	Ok(())
}

/// `fetched_at = 0` marks a row the next source refresh overwrites; existing chapters are left alone
pub(crate) async fn insert_from_backup<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	chapter: &ChapterFields<'_>,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators,
		 language, thumbnail, url, date_uploaded, source_order, page_count, fetched_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
	)
	.bind(manga_id)
	.bind(chapter.key)
	.bind(chapter.title)
	.bind(chapter.number)
	.bind(chapter.volume)
	.bind(chapter.scanlators)
	.bind(chapter.language)
	.bind(chapter.thumbnail)
	.bind(chapter.url)
	.bind(chapter.date_uploaded)
	.bind(chapter.source_order)
	.bind(chapter.page_count)
	.execute(db)
	.await?;
	Ok(())
}

/// Like a backup row, but imports have no thumbnail or page count and record when they arrived
pub(crate) async fn insert_from_import<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	chapter: &ChapterFields<'_>,
	now: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators, language,
		 url, date_uploaded, source_order, page_count, fetched_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
	)
	.bind(manga_id)
	.bind(chapter.key)
	.bind(chapter.title)
	.bind(chapter.number)
	.bind(chapter.volume)
	.bind(chapter.scanlators)
	.bind(chapter.language)
	.bind(chapter.url)
	.bind(chapter.date_uploaded)
	.bind(chapter.source_order)
	.bind(now)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn set_fields<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, chapter: &ChapterFields<'_>) -> ApiResult<()> {
	sqlx::query(
		"UPDATE chapter SET key = ?, title = ?, number = ?, volume = ?, scanlators = ?, language = ?,
		 thumbnail = ?, url = ?, date_uploaded = ?, source_order = ?, page_count = ?, locked = ? WHERE id = ?",
	)
	.bind(chapter.key)
	.bind(chapter.title)
	.bind(chapter.number)
	.bind(chapter.volume)
	.bind(chapter.scanlators)
	.bind(chapter.language)
	.bind(chapter.thumbnail)
	.bind(chapter.url)
	.bind(chapter.date_uploaded)
	.bind(chapter.source_order)
	.bind(chapter.page_count)
	.bind(chapter.locked)
	.bind(id)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn insert_stub<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, key: &str) -> ApiResult<i64> {
	Ok(
		sqlx::query_scalar("INSERT INTO chapter (manga_id, key, last_modified_at) VALUES (?, ?, 0) RETURNING id")
			.bind(manga_id)
			.bind(key)
			.fetch_one(db)
			.await?,
	)
}

pub(crate) async fn insert_stub_with_id<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, manga_id: i64, key: &str) -> ApiResult<()> {
	sqlx::query("INSERT INTO chapter (id, manga_id, key, last_modified_at) VALUES (?, ?, ?, 0)")
		.bind(id)
		.bind(manga_id)
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_modified_at<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, modified_at: i64) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query("UPDATE chapter SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(db)
		.await?)
}

pub(crate) async fn delete<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM chapter WHERE id = ?").bind(id).execute(db).await?;
	Ok(())
}

/// A new key means new pages, so the count resets and a trashed row comes back
pub(crate) async fn rekey<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, key: &str) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET key = ?, page_count = 0, trashed_at = NULL WHERE id = ?")
		.bind(key)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn lock<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?").bind(id).execute(db).await?;
	Ok(())
}

/// Returns whether this call trashed it: the first stamp survives later scans as the "gone since" time
pub(crate) async fn trash<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, now: i64) -> ApiResult<bool> {
	let changed = sqlx::query("UPDATE chapter SET trashed_at = ? WHERE id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(id)
		.execute(db)
		.await?
		.rows_affected();
	Ok(changed > 0)
}

pub(crate) async fn trash_manga<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, now: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET trashed_at = ? WHERE manga_id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(manga_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_page_info<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	page_count: i64,
	epub_divina_compatible: Option<bool>,
) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET page_count = ?, epub_divina_compatible = ? WHERE id = ?")
		.bind(page_count)
		.bind(epub_divina_compatible)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_title_by_key<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, key: &str, title: &str) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET title = ? WHERE manga_id = ? AND key = ?")
		.bind(title)
		.bind(manga_id)
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_scanlators_by_key<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	key: &str,
	scanlators: &str,
) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET scanlators = ? WHERE manga_id = ? AND key = ?")
		.bind(scanlators)
		.bind(manga_id)
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_epub_compatible_by_key<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	key: &str,
	compatible: Option<bool>,
) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET epub_divina_compatible = ? WHERE manga_id = ? AND key = ?")
		.bind(compatible)
		.bind(manga_id)
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn clear_analyzed<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE id = ?").bind(id).execute(db).await?;
	Ok(())
}

pub(crate) async fn clear_manga_analyzed<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE manga_id = ? AND pages_analyzed_at IS NOT NULL")
		.bind(manga_id)
		.execute(db)
		.await?;
	Ok(())
}

/// A chapter still at zero pages is settled until a scan sees its manga change
pub(crate) async fn stamp_empty_analyzed<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = ? WHERE id = ? AND page_count = 0")
		.bind(at)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}
