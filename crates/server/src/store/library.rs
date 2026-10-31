//! Writes to `manga_user`, the owner's library state for each manga.

use sqlx::{Executor, Sqlite};

use crate::error::ApiResult;

/// The full row, for backup restore and sync
pub(crate) struct LibraryEntry<'a> {
	pub manga_id: i64,
	pub user_id: i64,
	pub in_library: bool,
	pub in_library_at: Option<i64>,
	pub viewer_flags: i64,
	pub incognito: bool,
	pub sort_key: &'a str,
}

/// An entry already on the shelf or shelved before keeps its first-shelved time
pub(crate) async fn shelve<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, user_id: i64, at: i64) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, ?, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(at)
	.execute(db)
	.await?;
	Ok(())
}

/// For imports: an entry coming back to the shelf takes the imported time, one already there keeps its own
pub(crate) async fn shelve_imported<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	user_id: i64,
	at: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at)
		 VALUES (?, ?, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET
		 in_library = 1,
		 in_library_at = CASE WHEN manga_user.in_library != 0 THEN manga_user.in_library_at
		 ELSE excluded.in_library_at END",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(at)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn unshelve<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, ?, 0)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
	)
	.bind(manga_id)
	.bind(user_id)
	.execute(db)
	.await?;
	Ok(())
}

/// Per-manga incognito does not depend on library state, so the row may not exist yet
pub(crate) async fn set_incognito<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	user_id: i64,
	incognito: bool,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, incognito) VALUES (?, ?, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET incognito = excluded.incognito",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(incognito)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn upsert<'e, E: Executor<'e, Database = Sqlite>>(db: E, entry: &LibraryEntry<'_>) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at, viewer_flags, incognito, sort_key)
		 VALUES (?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET
		 in_library = excluded.in_library, in_library_at = excluded.in_library_at,
		 viewer_flags = excluded.viewer_flags, incognito = excluded.incognito,
		 sort_key = excluded.sort_key",
	)
	.bind(entry.manga_id)
	.bind(entry.user_id)
	.bind(entry.in_library)
	.bind(entry.in_library_at)
	.bind(entry.viewer_flags)
	.bind(entry.incognito)
	.bind(entry.sort_key)
	.execute(db)
	.await?;
	Ok(())
}

pub(crate) async fn delete<'e, E: Executor<'e, Database = Sqlite>>(db: E, manga_id: i64, user_id: i64) -> ApiResult<()> {
	sqlx::query("DELETE FROM manga_user WHERE manga_id = ? AND user_id = ?")
		.bind(manga_id)
		.bind(user_id)
		.execute(db)
		.await?;
	Ok(())
}

pub(crate) async fn set_modified_at<'e, E: Executor<'e, Database = Sqlite>>(
	db: E,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<sqlx::sqlite::SqliteQueryResult> {
	Ok(sqlx::query("UPDATE manga_user SET last_modified_at = ? WHERE manga_id = ? AND user_id = ?")
		.bind(modified_at)
		.bind(manga_id)
		.bind(user_id)
		.execute(db)
		.await?)
}
