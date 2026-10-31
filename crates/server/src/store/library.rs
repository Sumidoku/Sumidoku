//! Writes to `manga_user`, the owner's library state for each manga.

use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::sync::journal;

/// The full row, for backup restore and sync
pub struct LibraryEntry<'a> {
	pub manga_id: i64,
	pub user_id: i64,
	pub in_library: bool,
	pub in_library_at: Option<i64>,
	pub viewer_flags: i64,
	pub incognito: bool,
	pub sort_key: &'a str,
}

#[derive(sqlx::FromRow, PartialEq)]
struct Published {
	in_library: bool,
	in_library_at: Option<i64>,
	viewer_flags: i64,
	sort_key: String,
	incognito: bool,
	last_modified_at: i64,
}

async fn published(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<Option<Published>> {
	Ok(sqlx::query_as(
		"SELECT in_library, in_library_at, viewer_flags, sort_key, incognito, last_modified_at
		 FROM manga_user WHERE manga_id = ? AND user_id = ?",
	)
	.bind(manga_id)
	.bind(user_id)
	.fetch_optional(conn)
	.await?)
}

async fn emit(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, floor: i64) -> ApiResult<()> {
	let key = format!("{manga_id}:{user_id}");
	let version = journal::next_version(conn, Some(user_id), "manga_user", &key, floor).await?;
	set_modified_at(&mut *conn, manga_id, user_id, version).await?;
	journal::manga_for_user(conn, user_id, manga_id).await?;
	journal::library_entry(conn, manga_id, user_id, None).await
}

/// Publishes a new version if the write changed anything peers see
async fn publish_change(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, before: Option<Published>) -> ApiResult<()> {
	let Some(after) = published(conn, manga_id, user_id).await? else { return Ok(()) };
	match before {
		None => emit(conn, manga_id, user_id, 0).await,
		Some(before) if before != after => emit(conn, manga_id, user_id, before.last_modified_at + 1).await,
		Some(_) => Ok(()),
	}
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<()> {
	let Some(row) = published(conn, manga_id, user_id).await? else { return Ok(()) };
	let key = format!("{manga_id}:{user_id}");
	if row.last_modified_at == 0 || row.last_modified_at == journal::latest_version(conn, Some(user_id), "manga_user", &key).await? {
		return emit(conn, manga_id, user_id, row.last_modified_at + 1).await;
	}
	journal::manga_for_user(conn, user_id, manga_id).await?;
	journal::library_entry(conn, manga_id, user_id, None).await
}

/// An entry already on the shelf or shelved before keeps its first-shelved time
pub async fn shelve(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, at: i64) -> ApiResult<()> {
	let before = published(conn, manga_id, user_id).await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, ?, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(at)
	.execute(&mut *conn)
	.await?;
	publish_change(conn, manga_id, user_id, before).await
}

/// For imports: an entry coming back to the shelf takes the imported time, one already there keeps its own
pub(crate) async fn shelve_imported(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, at: i64) -> ApiResult<()> {
	let before = published(conn, manga_id, user_id).await?;
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
	.execute(&mut *conn)
	.await?;
	publish_change(conn, manga_id, user_id, before).await
}

pub async fn unshelve(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<()> {
	let before = published(conn, manga_id, user_id).await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, ?, 0)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
	)
	.bind(manga_id)
	.bind(user_id)
	.execute(&mut *conn)
	.await?;
	publish_change(conn, manga_id, user_id, before).await
}

/// Per-manga incognito does not depend on library state, so the row may not exist yet
pub async fn set_incognito(conn: &mut SqliteConnection, manga_id: i64, user_id: i64, incognito: bool) -> ApiResult<()> {
	let before = published(conn, manga_id, user_id).await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, incognito) VALUES (?, ?, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET incognito = excluded.incognito",
	)
	.bind(manga_id)
	.bind(user_id)
	.bind(incognito)
	.execute(&mut *conn)
	.await?;
	publish_change(conn, manga_id, user_id, before).await
}

pub async fn upsert(conn: &mut SqliteConnection, entry: &LibraryEntry<'_>) -> ApiResult<()> {
	let before = published(conn, entry.manga_id, entry.user_id).await?;
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
	.execute(&mut *conn)
	.await?;
	publish_change(conn, entry.manga_id, entry.user_id, before).await
}

pub async fn delete(conn: &mut SqliteConnection, manga_id: i64, user_id: i64) -> ApiResult<()> {
	if let Some(before) = published(conn, manga_id, user_id).await? {
		journal::manga_for_user(conn, user_id, manga_id).await?;
		journal::library_entry(conn, manga_id, user_id, Some(journal::delete_version(before.last_modified_at))).await?;
	}
	sqlx::query("DELETE FROM manga_user WHERE manga_id = ? AND user_id = ?")
		.bind(manga_id)
		.bind(user_id)
		.execute(conn)
		.await?;
	Ok(())
}

pub async fn set_modified_at(
	conn: &mut SqliteConnection,
	manga_id: i64,
	user_id: i64,
	modified_at: i64,
) -> ApiResult<sqlx::sqlite::SqliteQueryResult> {
	Ok(sqlx::query("UPDATE manga_user SET last_modified_at = ? WHERE manga_id = ? AND user_id = ?")
		.bind(modified_at)
		.bind(manga_id)
		.bind(user_id)
		.execute(conn)
		.await?)
}
