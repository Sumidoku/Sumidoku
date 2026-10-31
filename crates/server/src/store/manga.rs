//! Writes to `manga`.

use sqlx::sqlite::SqliteQueryResult;
use sqlx::{Executor, Sqlite};

use crate::error::ApiResult;

/// The columns a source, a backup or a sync event describes. The JSON columns hold array text.
pub(crate) struct MangaFields<'a> {
	pub title: &'a str,
	pub authors: &'a str,
	pub artists: &'a str,
	pub description: Option<&'a str>,
	pub tags: &'a str,
	pub status: i64,
	pub content_rating: i64,
	pub viewer: i64,
	pub cover_url: Option<&'a str>,
	pub url: Option<&'a str>,
	pub initialized: bool,
	pub update_strategy: i64,
}

/// Timestamps the caller stamps with a source upsert
pub(crate) struct FetchTimes {
	pub next_update_time: Option<i64>,
	pub last_fetched_at: Option<i64>,
	pub chapters_last_fetched_at: Option<i64>,
	pub now: i64,
}

/// A refresh revives a trashed row. `next_update_is_fallback` survives only an unchanged value, so a
/// derived deadline the source merely echoes back stays derived; the fetch times are stamped apart,
/// because only the caller knows which fetches happened.
pub(crate) async fn upsert_from_source<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
	times: &FetchTimes,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga (source_id, key, title, authors, artists, description, tags, status,
		 content_rating, viewer, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (source_id, key) DO UPDATE SET
		 title = excluded.title, authors = excluded.authors, artists = excluded.artists,
		 description = excluded.description, tags = excluded.tags, status = excluded.status,
		 content_rating = excluded.content_rating, viewer = excluded.viewer,
		 cover_url = excluded.cover_url,
		 url = excluded.url, initialized = excluded.initialized,
		 trashed_at = NULL,
		 update_strategy = excluded.update_strategy,
		 next_update_time = excluded.next_update_time,
		 next_update_is_fallback = CASE
		 WHEN manga.next_update_time IS excluded.next_update_time THEN manga.next_update_is_fallback
		 ELSE 0 END,
		 created_at = manga.created_at",
	)
	.bind(source_id)
	.bind(key)
	.bind(manga.title)
	.bind(manga.authors)
	.bind(manga.artists)
	.bind(manga.description)
	.bind(manga.tags)
	.bind(manga.status)
	.bind(manga.content_rating)
	.bind(manga.viewer)
	.bind(manga.cover_url)
	.bind(manga.url)
	.bind(manga.initialized)
	.bind(manga.update_strategy)
	.bind(times.next_update_time)
	.bind(times.last_fetched_at)
	.bind(times.chapters_last_fetched_at)
	.bind(times.now)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn upsert_from_backup<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga (source_id, key, title, authors, artists, description, tags, status,
		 content_rating, viewer, cover_url, url, initialized, update_strategy)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (source_id, key) DO UPDATE SET
		 title = excluded.title, authors = excluded.authors, artists = excluded.artists,
		 description = excluded.description, tags = excluded.tags, status = excluded.status,
		 content_rating = excluded.content_rating, viewer = excluded.viewer,
		 cover_url = excluded.cover_url,
		 url = excluded.url, initialized = excluded.initialized,
		 update_strategy = excluded.update_strategy",
	)
	.bind(source_id)
	.bind(key)
	.bind(manga.title)
	.bind(manga.authors)
	.bind(manga.artists)
	.bind(manga.description)
	.bind(manga.tags)
	.bind(manga.status)
	.bind(manga.content_rating)
	.bind(manga.viewer)
	.bind(manga.cover_url)
	.bind(manga.url)
	.bind(manga.initialized)
	.bind(manga.update_strategy)
	.execute(db)
	.await?;
	Ok(())
}

/// Existing rows are left alone. Imports have no viewer, and the row stays uninitialized until a
/// source refresh fills it in, so `viewer`, `initialized` and `update_strategy` are not taken from `manga`.
pub(crate) async fn insert_from_import<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
	now: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT OR IGNORE INTO manga (source_id, key, title, authors, artists, description, tags,
		 status, content_rating, cover_url, url, initialized, update_strategy, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
	)
	.bind(source_id)
	.bind(key)
	.bind(manga.title)
	.bind(manga.authors)
	.bind(manga.artists)
	.bind(manga.description)
	.bind(manga.tags)
	.bind(manga.status)
	.bind(manga.content_rating)
	.bind(manga.cover_url)
	.bind(manga.url)
	.bind(now)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn set_fields<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
) -> ApiResult<()> {
	sqlx::query(
		"UPDATE manga SET source_id = ?, key = ?, title = ?, authors = ?, artists = ?, description = ?, tags = ?,
		 status = ?, content_rating = ?, viewer = ?, cover_url = ?, url = ?, initialized = ?, update_strategy = ?
		 WHERE id = ?",
	)
	.bind(source_id)
	.bind(key)
	.bind(manga.title)
	.bind(manga.authors)
	.bind(manga.artists)
	.bind(manga.description)
	.bind(manga.tags)
	.bind(manga.status)
	.bind(manga.content_rating)
	.bind(manga.viewer)
	.bind(manga.cover_url)
	.bind(manga.url)
	.bind(manga.initialized)
	.bind(manga.update_strategy)
	.bind(id)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn insert_stub<'e, E: Executor<'e, Database = Sqlite>>(db: E, source_id: i64, key: &str) -> ApiResult<i64> {
	Ok(
		sqlx::query_scalar("INSERT INTO manga (source_id, key, title, last_modified_at) VALUES (?, ?, '', 0) RETURNING id")
			.bind(source_id)
			.bind(key)
			.fetch_one(db)
			.await?,
	)
}

pub(crate) async fn insert_stub_with_id<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, source_id: i64, key: &str) -> ApiResult<()> {
	sqlx::query("INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, ?, '', 0)")
		.bind(id)
		.bind(source_id)
		.bind(key)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_modified_at<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, modified_at: i64) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query("UPDATE manga SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(db)
		.await?)
}

pub(crate) async fn delete<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM manga WHERE id = ?").bind(id).execute(db).await?;
	Ok(())
}

/// Returns whether a row went: a manga restored in the meantime is not deleted
pub(crate) async fn delete_trashed<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64) -> ApiResult<bool> {
	let deleted = sqlx::query("DELETE FROM manga WHERE id = ? AND trashed_at IS NOT NULL")
		.bind(id)
		.execute(db)
		.await?
		.rows_affected();
	Ok(deleted > 0)
}

pub(crate) async fn trash<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, now: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET trashed_at = ? WHERE id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn insert_local<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	source_id: i64,
	key: &str,
	title: String,
	mtime: i64,
	kind: &str,
	now: i64,
) -> ApiResult<i64> {
	Ok(sqlx::query("INSERT INTO manga (source_id, key, title, scan_mtime, local_entry_kind, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL)")
		.bind(source_id)
		.bind(key)
		.bind(title)
		.bind(mtime)
		.bind(kind)
		.bind(now)
		.execute(db)
		.await?
		.last_insert_rowid())
}

pub(crate) async fn backfill_entry_kind<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, kind: &str) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET local_entry_kind = ? WHERE id = ? AND local_entry_kind IS NULL")
		.bind(kind)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

/// Records a changed local entry and revives it if trashed; `reinitialize` sends it back through metadata analysis
pub(crate) async fn mark_scanned<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	mtime: i64,
	kind: &str,
	reinitialize: bool,
) -> ApiResult<()> {
	let sql = if reinitialize {
		"UPDATE manga SET scan_mtime = ?, local_entry_kind = ?, trashed_at = NULL, initialized = 0 WHERE id = ?"
	} else {
		"UPDATE manga SET scan_mtime = ?, local_entry_kind = ?, trashed_at = NULL WHERE id = ?"
	};
	sqlx::query(sql).bind(mtime).bind(kind).bind(id).execute(db).await?;
	Ok(())
}

pub(crate) async fn set_updated_at<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET updated_at = ? WHERE id = ?").bind(at).bind(id).execute(db).await?;
	Ok(())
}

pub(crate) async fn set_update_strategy<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, strategy: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET update_strategy = ? WHERE id = ?")
		.bind(strategy)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_last_fetched_at<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET last_fetched_at = ? WHERE id = ?").bind(at).bind(id).execute(db).await?;
	Ok(())
}

pub(crate) async fn set_chapters_fetched_at<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
		.bind(at)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_next_update<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	id: i64,
	next_update_time: Option<i64>,
	is_fallback: bool,
	chapters_fetched_at: i64,
) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET next_update_time = ?, next_update_is_fallback = ?, chapters_last_fetched_at = ? WHERE id = ?")
		.bind(next_update_time)
		.bind(is_fallback)
		.bind(chapters_fetched_at)
		.bind(id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_cover_url<'e, E: Executor<'e, Database = Sqlite>>(db: E, id: i64, url: &str) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?").bind(url).bind(id).execute(db).await?;
	Ok(())
}
