//! Writes to `manga`.

use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteQueryResult;

use crate::error::ApiResult;
use crate::sync::journal;

/// The columns a source, a backup or a sync event describes. The JSON columns hold array text.
pub struct MangaFields<'a> {
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
pub struct FetchTimes {
	pub next_update_time: Option<i64>,
	pub last_fetched_at: Option<i64>,
	pub chapters_last_fetched_at: Option<i64>,
	pub now: i64,
}

#[derive(sqlx::FromRow, PartialEq)]
struct Published {
	source_id: i64,
	key: String,
	title: String,
	authors: String,
	artists: String,
	description: Option<String>,
	tags: String,
	status: i64,
	content_rating: i64,
	viewer: i64,
	cover_url: Option<String>,
	url: Option<String>,
	initialized: bool,
	update_strategy: i64,
}

const PUBLISHED: &str = "SELECT source_id, key, title, authors, artists, description, tags, status, content_rating,
	viewer, cover_url, url, initialized, update_strategy FROM manga";

async fn published(conn: &mut SqliteConnection, id: i64) -> ApiResult<Option<Published>> {
	Ok(sqlx::query_as(&format!("{PUBLISHED} WHERE id = ?")).bind(id).fetch_optional(conn).await?)
}

async fn id_of(conn: &mut SqliteConnection, source_id: i64, key: &str) -> ApiResult<Option<i64>> {
	Ok(sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_optional(conn)
		.await?)
}

async fn version_of(conn: &mut SqliteConnection, id: i64) -> ApiResult<i64> {
	Ok(sqlx::query_scalar("SELECT last_modified_at FROM manga WHERE id = ?").bind(id).fetch_one(conn).await?)
}

async fn publish_new(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let version = journal::next_version(conn, None, "manga", &id.to_string(), 0).await?;
	set_modified_at(&mut *conn, id, version).await?;
	journal::manga_for_library(conn, id).await
}

/// Publishes a new version if the write changed anything peers see. Chapters keep the source and
/// manga keys in their shadow copy, which tombstones read after this row is gone.
async fn publish_change(conn: &mut SqliteConnection, id: i64, before: Published, version: i64) -> ApiResult<()> {
	let Some(after) = published(conn, id).await? else { return Ok(()) };
	if after == before {
		return Ok(());
	}
	if after.source_id != before.source_id || after.key != before.key {
		sqlx::query(
			"UPDATE sync_chapter_state
			 SET source_id = ?1, source_key = (SELECT key FROM source WHERE id = ?1), manga_key = ?2,
			     identity = json_set(identity, '$.sourceId', ?1, '$.sourceKey', (SELECT key FROM source WHERE id = ?1), '$.mangaKey', ?2)
			 WHERE manga_id = ?3",
		)
		.bind(after.source_id)
		.bind(&after.key)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	}
	set_modified_at(&mut *conn, id, journal::now_ms().max(version + 1)).await?;
	journal::manga_for_library(conn, id).await
}

/// Journals the row as stored, keeping a version it was given. For
/// writes that bypass this store, which only tests do.
pub async fn publish(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let version = version_of(conn, id).await?;
	if version == 0 || version == journal::latest_version(conn, None, "manga", &id.to_string()).await? {
		let next = journal::next_version(conn, None, "manga", &id.to_string(), version + 1).await?;
		set_modified_at(&mut *conn, id, next).await?;
	}
	journal::manga_for_library(conn, id).await
}

/// A refresh revives a trashed row. `next_update_is_fallback` survives only an unchanged value, so a
/// derived deadline the source merely echoes back stays derived; the fetch times are stamped apart,
/// because only the caller knows which fetches happened.
pub(crate) async fn upsert_from_source(conn: &mut SqliteConnection, source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
	times: &FetchTimes,
) -> ApiResult<()> {
	let existing = id_of(conn, source_id, key).await?;
	let before = match existing {
		Some(id) => Some((id, published(conn, id).await?, version_of(conn, id).await?)),
		None => None,
	};
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
	.execute(&mut *conn)
	.await?;
	publish_upsert(conn, source_id, key, before).await
}

async fn publish_upsert(
	conn: &mut SqliteConnection,
	source_id: i64,
	key: &str,
	before: Option<(i64, Option<Published>, i64)>,
) -> ApiResult<()> {
	match before {
		Some((id, Some(before), version)) => publish_change(conn, id, before, version).await,
		_ => match id_of(conn, source_id, key).await? {
			Some(id) => publish_new(conn, id).await,
			None => Ok(()),
		},
	}
}

pub(crate) async fn upsert_from_backup(conn: &mut SqliteConnection, source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
) -> ApiResult<()> {
	let existing = id_of(conn, source_id, key).await?;
	let before = match existing {
		Some(id) => Some((id, published(conn, id).await?, version_of(conn, id).await?)),
		None => None,
	};
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
	.execute(&mut *conn)
	.await?;
	publish_upsert(conn, source_id, key, before).await
}

/// Existing rows are left alone. Imports have no viewer, and the row stays uninitialized until a
/// source refresh fills it in, so `viewer`, `initialized` and `update_strategy` are not taken from `manga`.
pub(crate) async fn insert_from_import(conn: &mut SqliteConnection, source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
	now: i64,
) -> ApiResult<()> {
	let existed = id_of(conn, source_id, key).await?.is_some();
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
	.execute(&mut *conn)
	.await?;
	if !existed && let Some(id) = id_of(conn, source_id, key).await? {
		publish_new(conn, id).await?;
	}
	Ok(())
}

pub(crate) async fn set_fields(conn: &mut SqliteConnection, id: i64,
	source_id: i64,
	key: &str,
	manga: &MangaFields<'_>,
) -> ApiResult<()> {
	let before = published(conn, id).await?;
	let version = version_of(conn, id).await?;
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
	.execute(&mut *conn)
	.await?;
	match before {
		Some(before) => publish_change(conn, id, before, version).await,
		None => Ok(()),
	}
}

/// A sync stub is journaled like any new row; `finish` drops its events and stamps version 0
pub(crate) async fn insert_stub(conn: &mut SqliteConnection, source_id: i64, key: &str) -> ApiResult<i64> {
	let id = sqlx::query_scalar("INSERT INTO manga (source_id, key, title, last_modified_at) VALUES (?, ?, '', 0) RETURNING id")
		.bind(source_id)
		.bind(key)
		.fetch_one(&mut *conn)
		.await?;
	publish_new(conn, id).await?;
	Ok(id)
}

pub(crate) async fn insert_stub_with_id(conn: &mut SqliteConnection, id: i64, source_id: i64, key: &str) -> ApiResult<()> {
	sqlx::query("INSERT INTO manga (id, source_id, key, title, last_modified_at) VALUES (?, ?, ?, '', 0)")
		.bind(id)
		.bind(source_id)
		.bind(key)
		.execute(&mut *conn)
		.await?;
	publish_new(conn, id).await
}

pub async fn set_modified_at(conn: &mut SqliteConnection, id: i64, modified_at: i64) -> ApiResult<SqliteQueryResult> {
	Ok(sqlx::query("UPDATE manga SET last_modified_at = ? WHERE id = ?")
		.bind(modified_at)
		.bind(id)
		.execute(conn)
		.await?)
}

/// Tombstones for the manga, its chapters and their progress, then for the library rows the delete
/// cascades to. The chapters' shadow copies go first so nothing is tombstoned twice.
async fn publish_delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	let now = journal::now_ms();
	journal::append!(
		conn,
		"SELECT fans.user_id, 'manga', CAST(sm.manga_id AS TEXT), 1, MAX(?2, sm.last_modified_at + 1), sm.identity, sm.snapshot
		 FROM sync_manga_state sm
		 JOIN (
			SELECT mu.user_id FROM manga_user mu WHERE mu.manga_id = ?1
			UNION
			SELECT cu.user_id FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id WHERE c.manga_id = ?1
		 ) fans ON 1 = 1
		 JOIN user u ON u.id = fans.user_id
		 WHERE sm.manga_id = ?1 ORDER BY fans.user_id",
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
			JOIN manga_user mu ON mu.manga_id = c.manga_id WHERE c.manga_id = ?1
			UNION
			SELECT cu.chapter_id, cu.user_id FROM chapter_user cu
			JOIN chapter c ON c.id = cu.chapter_id WHERE c.manga_id = ?1
		 ) fans ON fans.chapter_id = scs.chapter_id
		 JOIN user u ON u.id = fans.user_id
		 WHERE scs.manga_id = ?1 ORDER BY fans.user_id, scs.chapter_id",
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
		 JOIN chapter c ON c.id = cu.chapter_id
		 JOIN sync_chapter_state scs ON scs.chapter_id = cu.chapter_id
		 JOIN user u ON u.id = cu.user_id
		 WHERE c.manga_id = ?1 ORDER BY cu.user_id, cu.chapter_id",
		id,
		now,
	)?;
	sqlx::query("DELETE FROM sync_chapter_state WHERE manga_id = ?").bind(id).execute(&mut *conn).await?;
	journal::cascade_library_rows(conn, id).await
}

pub async fn delete(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
	publish_delete(conn, id).await?;
	sqlx::query("DELETE FROM manga WHERE id = ?").bind(id).execute(conn).await?;
	Ok(())
}

/// Returns whether a row went: a manga restored in the meantime is not deleted
pub(crate) async fn delete_trashed(conn: &mut SqliteConnection, id: i64) -> ApiResult<bool> {
	let trashed: Option<i64> = sqlx::query_scalar("SELECT id FROM manga WHERE id = ? AND trashed_at IS NOT NULL")
		.bind(id)
		.fetch_optional(&mut *conn)
		.await?;
	if trashed.is_none() {
		return Ok(false);
	}
	delete(conn, id).await?;
	Ok(true)
}

/// Returns false when another row holds the key, leaving this one unchanged
pub(crate) async fn try_rekey(conn: &mut SqliteConnection, id: i64, key: &str) -> ApiResult<bool> {
	let before = published(conn, id).await?;
	let version = version_of(conn, id).await?;
	match sqlx::query("UPDATE manga SET key = ? WHERE id = ?").bind(key).bind(id).execute(&mut *conn).await {
		Ok(_) => {}
		Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Ok(false),
		Err(e) => return Err(e.into()),
	}
	if let Some(before) = before {
		publish_change(conn, id, before, version).await?;
	}
	Ok(true)
}

pub(crate) async fn trash(conn: &mut SqliteConnection, id: i64, now: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET trashed_at = ? WHERE id = ? AND trashed_at IS NULL")
		.bind(now)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	Ok(())
}

pub(crate) async fn insert_local(conn: &mut SqliteConnection, source_id: i64,
	key: &str,
	title: String,
	mtime: i64,
	kind: &str,
	now: i64,
) -> ApiResult<i64> {
	let id = sqlx::query("INSERT INTO manga (source_id, key, title, scan_mtime, local_entry_kind, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL)")
		.bind(source_id)
		.bind(key)
		.bind(title)
		.bind(mtime)
		.bind(kind)
		.bind(now)
		.execute(&mut *conn)
		.await?
		.last_insert_rowid();
	publish_new(conn, id).await?;
	Ok(id)
}

pub(crate) async fn backfill_entry_kind(conn: &mut SqliteConnection, id: i64, kind: &str) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET local_entry_kind = ? WHERE id = ? AND local_entry_kind IS NULL")
		.bind(kind)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	Ok(())
}

/// Records a changed local entry and revives it if trashed; `reinitialize` sends it back through metadata analysis
pub(crate) async fn mark_scanned(conn: &mut SqliteConnection, id: i64,
	mtime: i64,
	kind: &str,
	reinitialize: bool,
) -> ApiResult<()> {
	let sql = if reinitialize {
		"UPDATE manga SET scan_mtime = ?, local_entry_kind = ?, trashed_at = NULL, initialized = 0 WHERE id = ?"
	} else {
		"UPDATE manga SET scan_mtime = ?, local_entry_kind = ?, trashed_at = NULL WHERE id = ?"
	};
	let before = published(conn, id).await?;
	let version = version_of(conn, id).await?;
	sqlx::query(sql).bind(mtime).bind(kind).bind(id).execute(&mut *conn).await?;
	match before {
		Some(before) => publish_change(conn, id, before, version).await,
		None => Ok(()),
	}
}

pub(crate) async fn set_updated_at(conn: &mut SqliteConnection, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET updated_at = ? WHERE id = ?").bind(at).bind(id).execute(&mut *conn).await?;
	Ok(())
}

pub(crate) async fn set_update_strategy(conn: &mut SqliteConnection, id: i64, strategy: i64) -> ApiResult<()> {
	let before = published(conn, id).await?;
	let version = version_of(conn, id).await?;
	sqlx::query("UPDATE manga SET update_strategy = ? WHERE id = ?")
		.bind(strategy)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	match before {
		Some(before) => publish_change(conn, id, before, version).await,
		None => Ok(()),
	}
}

pub(crate) async fn set_last_fetched_at(conn: &mut SqliteConnection, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET last_fetched_at = ? WHERE id = ?").bind(at).bind(id).execute(&mut *conn).await?;
	Ok(())
}

pub(crate) async fn set_chapters_fetched_at(conn: &mut SqliteConnection, id: i64, at: i64) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
		.bind(at)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	Ok(())
}

pub(crate) async fn set_next_update(conn: &mut SqliteConnection, id: i64,
	next_update_time: Option<i64>,
	is_fallback: bool,
	chapters_fetched_at: i64,
) -> ApiResult<()> {
	sqlx::query("UPDATE manga SET next_update_time = ?, next_update_is_fallback = ?, chapters_last_fetched_at = ? WHERE id = ?")
		.bind(next_update_time)
		.bind(is_fallback)
		.bind(chapters_fetched_at)
		.bind(id)
		.execute(&mut *conn)
		.await?;
	Ok(())
}

pub(crate) async fn set_cover_url(conn: &mut SqliteConnection, id: i64, url: &str) -> ApiResult<()> {
	let before = published(conn, id).await?;
	let version = version_of(conn, id).await?;
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?").bind(url).bind(id).execute(&mut *conn).await?;
	match before {
		Some(before) => publish_change(conn, id, before, version).await,
		None => Ok(()),
	}
}
