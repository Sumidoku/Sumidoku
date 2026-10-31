//! Writes to `chapter`.

use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteQueryResult;

use crate::error::ApiResult;
use crate::sync::journal;

/// The columns a source, a backup or a sync event describes
pub struct ChapterFields<'a> {
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

/// Columns peers see
#[derive(sqlx::FromRow, PartialEq)]
struct Published {
	manga_id: i64,
	key: String,
	title: Option<String>,
	number: Option<f64>,
	volume: Option<f64>,
	scanlators: String,
	language: Option<String>,
	thumbnail: Option<String>,
	url: Option<String>,
	date_uploaded: Option<i64>,
	source_order: i64,
	page_count: i64,
	locked: bool,
	last_modified_at: i64,
}

impl Published {
	fn same_fields(&self, other: &Published) -> bool {
		self.manga_id == other.manga_id
			&& self.key == other.key
			&& self.title == other.title
			&& self.number == other.number
			&& self.volume == other.volume
			&& self.scanlators == other.scanlators
			&& self.language == other.language
			&& self.thumbnail == other.thumbnail
			&& self.url == other.url
			&& self.date_uploaded == other.date_uploaded
			&& self.source_order == other.source_order
			&& self.page_count == other.page_count
			&& self.locked == other.locked
	}
}

const PUBLISHED: &str = "SELECT manga_id, key, title, number, volume, scanlators, language, thumbnail, url,
	date_uploaded, source_order, page_count, locked, last_modified_at FROM chapter";

async fn published(conn: &mut SqliteConnection, id: i64) -> ApiResult<Option<Published>> {
	Ok(sqlx::query_as(&format!("{PUBLISHED} WHERE id = ?")).bind(id).fetch_optional(conn).await?)
}

async fn published_by_key(conn: &mut SqliteConnection, manga_id: i64, key: &str) -> ApiResult<Option<(i64, Published)>> {
	let id: Option<i64> = sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(key)
		.fetch_optional(&mut *conn)
		.await?;
	let Some(id) = id else { return Ok(None) };
	Ok(published(conn, id).await?.map(|row| (id, row)))
}

/// Keeps the copy of each chapter that outlives the row, which tombstones and child events read
async fn refresh_state(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO sync_chapter_state
		 (chapter_id, manga_id, source_id, source_key, manga_key, chapter_key, identity, snapshot)
		 SELECT c.id, m.id, s.id, s.key, m.key, c.key,
		 json_object(
			'chapterId', c.id,
			'mangaId', m.id,
			'sourceId', s.id,
			'sourceKey', s.key,
			'mangaKey', m.key,
			'chapterKey', c.key
		 ),
		 json_object(
			'id', c.id,
			'mangaId', c.manga_id,
			'key', c.key,
			'title', c.title,
			'number', c.number,
			'volume', c.volume,
			'scanlators', json(c.scanlators),
			'language', c.language,
			'thumbnail', c.thumbnail,
			'url', c.url,
			'dateUploaded', c.date_uploaded,
			'sourceOrder', c.source_order,
			'pageCount', c.page_count,
			'locked', c.locked,
			'lastModifiedAt', c.last_modified_at
		 )
		 FROM chapter c
		 JOIN manga m ON m.id = c.manga_id
		 JOIN source s ON s.id = m.source_id
		 WHERE c.id = ?
		 ON CONFLICT (chapter_id) DO UPDATE SET
		 manga_id = excluded.manga_id, source_id = excluded.source_id, source_key = excluded.source_key,
		 manga_key = excluded.manga_key, chapter_key = excluded.chapter_key,
		 identity = excluded.identity, snapshot = excluded.snapshot",
	)
	.bind(id)
	.execute(conn)
	.await?;
	Ok(())
}

async fn emit(conn: &mut SqliteConnection, id: i64, manga_id: i64, floor: i64) -> ApiResult<()> {
	let version = journal::next_version(conn, None, "chapter", &id.to_string(), floor).await?;
	sqlx::query("UPDATE chapter SET last_modified_at = ? WHERE id = ?")
		.bind(version)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	refresh_state(conn, id).await?;
	journal::manga_for_chapter_fans(conn, id, manga_id).await?;
	journal::chapter_for_fans(conn, id, manga_id).await
}

/// Publishes a new version if the write changed anything peers see
async fn publish_change(conn: &mut SqliteConnection, id: i64, before: Option<Published>) -> ApiResult<()> {
	let Some(after) = published(conn, id).await? else { return Ok(()) };
	match before {
		None => emit(conn, id, after.manga_id, 0).await,
		Some(before) if !before.same_fields(&after) => emit(conn, id, after.manga_id, before.last_modified_at + 1).await,
		Some(_) => Ok(()),
	}
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let Some(row) = published(conn, id).await? else { return Ok(()) };
	if row.last_modified_at == 0 || row.last_modified_at == journal::latest_version(conn, None, "chapter", &id.to_string()).await? {
		return emit(conn, id, row.manga_id, row.last_modified_at + 1).await;
	}
	refresh_state(conn, id).await?;
	journal::manga_for_chapter_fans(conn, id, row.manga_id).await?;
	journal::chapter_for_fans(conn, id, row.manga_id).await
}

/// A refresh rewrites everything the source owns and revives a trashed row; `page_count` and the
/// fetch times stay with the local row.
pub async fn upsert_from_source(conn: &mut SqliteConnection, manga_id: i64, chapter: &ChapterFields<'_>, now: i64) -> ApiResult<()> {
	let before = published_by_key(conn, manga_id, chapter.key).await?;
	let id: i64 = sqlx::query_scalar(
		"INSERT INTO chapter (manga_id, key, title, number, volume, scanlators, language, thumbnail, url,
		 date_uploaded, source_order, locked, fetched_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (manga_id, key) DO UPDATE SET
		 title = excluded.title, number = excluded.number, volume = excluded.volume,
		 scanlators = excluded.scanlators, language = excluded.language,
		 thumbnail = excluded.thumbnail, url = excluded.url,
		 date_uploaded = excluded.date_uploaded, source_order = excluded.source_order,
		 locked = excluded.locked, fetched_at = chapter.fetched_at, created_at = chapter.created_at,
		 trashed_at = NULL
		 RETURNING id",
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
	.fetch_one(&mut *conn)
	.await?;
	publish_change(conn, id, before.map(|(_, row)| row)).await
}

async fn publish_inserted(conn: &mut SqliteConnection, manga_id: i64, key: &str, existed: bool) -> ApiResult<()> {
	if existed {
		return Ok(());
	}
	match published_by_key(conn, manga_id, key).await? {
		Some((id, row)) => emit(conn, id, row.manga_id, 0).await,
		None => Ok(()),
	}
}

/// `fetched_at = 0` marks a row the next source refresh overwrites; existing chapters are left alone
pub(crate) async fn insert_from_backup(conn: &mut SqliteConnection, manga_id: i64, chapter: &ChapterFields<'_>) -> ApiResult<()> {
	let existed = published_by_key(conn, manga_id, chapter.key).await?.is_some();
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
	.execute(&mut *conn)
	.await?;
	publish_inserted(conn, manga_id, chapter.key, existed).await
}

/// Like a backup row, but imports have no thumbnail or page count and record when they arrived
pub(crate) async fn insert_from_import(conn: &mut SqliteConnection, manga_id: i64, chapter: &ChapterFields<'_>, now: i64) -> ApiResult<()> {
	let existed = published_by_key(conn, manga_id, chapter.key).await?.is_some();
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
	.execute(&mut *conn)
	.await?;
	publish_inserted(conn, manga_id, chapter.key, existed).await
}

pub(crate) async fn set_fields(conn: &mut SqliteConnection, id: i64, chapter: &ChapterFields<'_>) -> ApiResult<()> {
	let before = published(conn, id).await?;
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
	.execute(&mut *conn)
	.await?;
	publish_change(conn, id, before).await
}

/// A sync stub is journaled like any new row; `finish` drops its events and stamps version 0
pub(crate) async fn insert_stub(conn: &mut SqliteConnection, manga_id: i64, key: &str) -> ApiResult<i64> {
	let id = sqlx::query_scalar("INSERT INTO chapter (manga_id, key, last_modified_at) VALUES (?, ?, 0) RETURNING id")
		.bind(manga_id)
		.bind(key)
		.fetch_one(&mut *conn)
		.await?;
	emit(conn, id, manga_id, 0).await?;
	Ok(id)
}

pub(crate) async fn insert_stub_with_id(conn: &mut SqliteConnection, id: i64, manga_id: i64, key: &str) -> ApiResult<()> {
	sqlx::query("INSERT INTO chapter (id, manga_id, key, last_modified_at) VALUES (?, ?, ?, 0)")
		.bind(id)
		.bind(manga_id)
		.bind(key)
		.execute(&mut *conn)
		.await?;
	emit(conn, id, manga_id, 0).await
}

pub async fn set_modified_at(conn: &mut SqliteConnection, id: i64, modified_at: i64) -> ApiResult<SqliteQueryResult> {
	let result = sqlx::query("UPDATE chapter SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	sqlx::query("UPDATE sync_chapter_state SET snapshot = json_set(snapshot, '$.lastModifiedAt', ?) WHERE chapter_id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(conn)
		.await?;
	Ok(result)
}

/// Tombstones go first because they read the row and the shadow copy. The shadow copy goes before
/// the delete so the cascade to `chapter_user` stays quiet: those rows were just tombstoned here.
pub async fn delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let shadow: Option<i64> = sqlx::query_scalar("SELECT manga_id FROM sync_chapter_state WHERE chapter_id = ?")
		.bind(id)
		.fetch_optional(&mut *conn)
		.await?;
	if let Some(manga_id) = shadow {
		journal::manga_for_chapter_fans(conn, id, manga_id).await?;
		let now = journal::now_ms();
		journal::append!(
			conn,
			"SELECT fans.user_id, 'chapter', CAST(scs.chapter_id AS TEXT), 1,
			 MAX(?3, CAST(json_extract(scs.snapshot, '$.lastModifiedAt') AS INTEGER) + 1), scs.identity, scs.snapshot
			 FROM sync_chapter_state scs
			 JOIN (
				SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = ?2
				UNION
				SELECT cu.user_id FROM chapter_user cu WHERE cu.chapter_id = ?1
			 ) fans ON 1 = 1
			 JOIN user u ON u.id = fans.user_id
			 WHERE scs.chapter_id = ?1 ORDER BY fans.user_id",
			id,
			manga_id,
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
			 WHERE cu.chapter_id = ?1 ORDER BY cu.user_id",
			id,
			now,
		)?;
		sqlx::query("DELETE FROM sync_chapter_state WHERE chapter_id = ?").bind(id).execute(&mut *conn).await?;
	}
	sqlx::query("DELETE FROM chapter WHERE id = ?").bind(id).execute(conn).await?;
	Ok(())
}

/// A new key means new pages, so the count resets and a trashed row comes back
pub(crate) async fn rekey(conn: &mut SqliteConnection, id: i64, key: &str) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE chapter SET key = ?, page_count = 0, trashed_at = NULL WHERE id = ?")
		.bind(key)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

/// Returns false when another row holds the key, leaving this one unchanged
pub(crate) async fn try_set_key(conn: &mut SqliteConnection, id: i64, key: &str) -> ApiResult<bool> {
	let before = published(conn, id).await?;
	match sqlx::query("UPDATE chapter SET key = ? WHERE id = ?").bind(key).bind(id).execute(&mut *conn).await {
		Ok(_) => {}
		Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Ok(false),
		Err(e) => return Err(e.into()),
	}
	publish_change(conn, id, before).await?;
	Ok(true)
}

pub(crate) async fn lock(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?").bind(id).execute(&mut *conn).await?;
	publish_change(conn, id, before).await
}

/// Returns whether this call trashed it: the first stamp survives later scans as the "gone since" time
pub(crate) async fn trash(conn: &mut SqliteConnection, id: i64, now: i64) -> ApiResult<bool> {
	let changed = sqlx::query("UPDATE chapter SET trashed_at = ? WHERE id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(id)
		.execute(conn)
		.await?
		.rows_affected();
	Ok(changed > 0)
}

pub(crate) async fn trash_manga(conn: &mut SqliteConnection, manga_id: i64, now: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET trashed_at = ? WHERE manga_id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(manga_id)
		.execute(conn)
		.await?;
	Ok(())
}

pub(crate) async fn set_page_info(
	conn: &mut SqliteConnection,
	id: i64,
	page_count: i64,
	epub_divina_compatible: Option<bool>,
) -> ApiResult<()> {
	let before = published(conn, id).await?;
	sqlx::query("UPDATE chapter SET page_count = ?, epub_divina_compatible = ? WHERE id = ?")
		.bind(page_count)
		.bind(epub_divina_compatible)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	publish_change(conn, id, before).await
}

pub(crate) async fn set_title_by_key(conn: &mut SqliteConnection, manga_id: i64, key: &str, title: &str) -> ApiResult<()> {
	let Some((id, before)) = published_by_key(conn, manga_id, key).await? else { return Ok(()) };
	sqlx::query("UPDATE chapter SET title = ? WHERE id = ?").bind(title).bind(id).execute(&mut *conn).await?;
	publish_change(conn, id, Some(before)).await
}

pub(crate) async fn set_scanlators_by_key(conn: &mut SqliteConnection, manga_id: i64, key: &str, scanlators: &str) -> ApiResult<()> {
	let Some((id, before)) = published_by_key(conn, manga_id, key).await? else { return Ok(()) };
	sqlx::query("UPDATE chapter SET scanlators = ? WHERE id = ?").bind(scanlators).bind(id).execute(&mut *conn).await?;
	publish_change(conn, id, Some(before)).await
}

pub(crate) async fn set_epub_compatible_by_key(conn: &mut SqliteConnection,
	manga_id: i64,
	key: &str,
	compatible: Option<bool>,
) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET epub_divina_compatible = ? WHERE manga_id = ? AND key = ?")
		.bind(compatible)
		.bind(manga_id)
		.bind(key)
		.execute(conn)
		.await?;
	Ok(())
}

pub(crate) async fn clear_analyzed(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE id = ?").bind(id).execute(conn).await?;
	Ok(())
}

pub(crate) async fn clear_manga_analyzed(conn: &mut SqliteConnection, manga_id: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE manga_id = ? AND pages_analyzed_at IS NOT NULL")
		.bind(manga_id)
		.execute(conn)
		.await?;
	Ok(())
}

/// A chapter still at zero pages is settled until a scan sees its manga change
pub(crate) async fn stamp_empty_analyzed(conn: &mut SqliteConnection, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter SET pages_analyzed_at = ? WHERE id = ? AND page_count = 0")
		.bind(at)
		.bind(id)
		.execute(conn)
		.await?;
	Ok(())
}
