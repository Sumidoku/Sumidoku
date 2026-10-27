//! Additive merge primitives shared by external-app backup imports (.aib / .tachibk):
//! existing library entries win (metadata not overwritten, in-library time kept), progress takes max(last_page_read),
//! is_read takes OR, last_read_at takes the newer NULL-safely; is_bookmarked is neither carried nor overwritten

use crate::error::ApiResult;
use sqlx::{Sqlite, Transaction};

#[derive(Debug, Default)]
pub struct ImportCounts {
	pub library: i64,
	pub categories: i64,
	pub progress: i64,
}

/// A same-named existing category keeps its sort order; new categories append at the end
pub async fn ensure_category(tx: &mut Transaction<'_, Sqlite>, user_id: i64, name: &str) -> ApiResult<i64> {
	let existing: Option<(i64,)> =
		sqlx::query_as("SELECT id FROM category WHERE user_id = ? AND name = ?")
			.bind(user_id)
			.bind(name)
			.fetch_optional(&mut **tx)
			.await?;
	if let Some((id,)) = existing {
		return Ok(id);
	}
	let (next,): (i64,) =
		sqlx::query_as("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM category WHERE user_id = ?")
			.bind(user_id)
			.fetch_one(&mut **tx)
			.await?;
	let result = sqlx::query(
		"INSERT INTO category (user_id, name, sort_order, is_default)
		 VALUES (?, ?, ?, 0)",
	)
	.bind(user_id)
	.bind(name)
	.bind(next)
	.execute(&mut **tx)
	.await?;
	Ok(result.last_insert_rowid())
}

pub async fn source_id_by_key(tx: &mut Transaction<'_, Sqlite>, key: &str) -> ApiResult<Option<i64>> {
	let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
	Ok(row.map(|(id,)| id))
}

/// Intermediate shape of a manga row's metadata: each importer normalizes its own backup model onto it
pub struct MangaMeta<'a> {
	pub title: &'a str,
	pub authors: String,
	pub artists: String,
	pub description: Option<&'a str>,
	pub tags: String,
	pub status: i64,
	pub content_rating: i64,
	pub cover_url: Option<&'a str>,
	pub url: Option<&'a str>,
}

/// Creates the manga row from backup metadata when missing; initialized=0, to be completed by the next source refresh
pub async fn ensure_manga_row(
	tx: &mut Transaction<'_, Sqlite>,
	source_id: i64,
	key: &str,
	meta: Option<&MangaMeta<'_>>,
) -> ApiResult<i64> {
	let empty = MangaMeta {
		title: "",
		authors: "[]".to_string(),
		artists: "[]".to_string(),
		description: None,
		tags: "[]".to_string(),
		status: 0,
		content_rating: 0,
		cover_url: None,
		url: None,
	};
	let meta = meta.unwrap_or(&empty);
	let now = crate::model::unix_now();
	sqlx::query(
		"INSERT OR IGNORE INTO manga (source_id, key, title, authors, artists, description, tags,
		 status, content_rating, cover_url, url, initialized, update_strategy, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
	)
	.bind(source_id)
	.bind(key)
	.bind(meta.title)
	.bind(&meta.authors)
	.bind(&meta.artists)
	.bind(meta.description)
	.bind(&meta.tags)
	.bind(meta.status)
	.bind(meta.content_rating)
	.bind(meta.cover_url)
	.bind(meta.url)
	.bind(now)
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

/// Intermediate shape of a chapter row's metadata
pub struct ChapterMeta<'a> {
	pub title: Option<&'a str>,
	pub number: Option<f64>,
	pub volume: Option<f64>,
	pub scanlators: String,
	pub language: Option<&'a str>,
	pub url: Option<&'a str>,
	pub date_uploaded: Option<i64>,
	pub source_order: i64,
}

/// Creates the chapter row from backup metadata when missing (fetched_at=0 marks a backup-created row, to be overwritten by the source refresh)
pub async fn ensure_chapter_row(
	tx: &mut Transaction<'_, Sqlite>,
	manga_id: i64,
	key: &str,
	meta: Option<&ChapterMeta<'_>>,
) -> ApiResult<i64> {
	let empty = ChapterMeta {
		title: None,
		number: None,
		volume: None,
		scanlators: "[]".to_string(),
		language: None,
		url: None,
		date_uploaded: None,
		source_order: 0,
	};
	let meta = meta.unwrap_or(&empty);
	let now = crate::model::unix_now();
	sqlx::query(
		"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators, language,
		 url, date_uploaded, source_order, page_count, fetched_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
	)
	.bind(manga_id)
	.bind(key)
	.bind(meta.title)
	.bind(meta.number)
	.bind(meta.volume)
	.bind(&meta.scanlators)
	.bind(meta.language)
	.bind(meta.url)
	.bind(meta.date_uploaded)
	.bind(meta.source_order)
	.bind(now)
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

/// Sets the in-library flag: already-shelved entries keep their existing in_library_at (existing entries win), unshelved ones take the backup's shelving time
pub async fn mark_in_library(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	manga_id: i64,
	in_library_at: i64,
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
	.bind(in_library_at)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

/// Progress merge: page number takes the max, is_read takes OR, timestamp takes the newer NULL-safely; is_bookmarked is local state, neither carried nor overwritten
pub async fn merge_chapter_progress(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	chapter_id: i64,
	last_page_read: i64,
	is_read: bool,
	last_read_at: Option<i64>,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked,
		 last_read_at) VALUES (?, ?, ?, ?, 0, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = MAX(chapter_user.last_page_read, excluded.last_page_read),
		 is_read = MAX(chapter_user.is_read, excluded.is_read),
		 last_read_at = CASE
		 WHEN chapter_user.last_read_at IS NULL THEN excluded.last_read_at
		 WHEN excluded.last_read_at IS NULL THEN chapter_user.last_read_at
		 ELSE MAX(chapter_user.last_read_at, excluded.last_read_at) END",
	)
	.bind(chapter_id)
	.bind(user_id)
	.bind(last_page_read)
	.bind(is_read)
	.bind(last_read_at)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
