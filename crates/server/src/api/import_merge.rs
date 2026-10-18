//! 外部 App 备份导入（.aib / .tachibk）共用的 additive 合并原语：
//! 已存在库条目优先（元数据不覆盖、入库时间保留），进度取 max(last_page_read)、
//! is_read 取 OR、last_read_at NULL 安全取较新；is_bookmarked 不携带不覆盖

use crate::error::ApiResult;
use sqlx::{Sqlite, Transaction};

#[derive(Debug, Default)]
pub struct ImportCounts {
	pub library: i64,
	pub categories: i64,
	pub progress: i64,
}

/// 同名分类已存在就原样使用（sort_order/三态以现有为准），新分类排到末尾
pub async fn ensure_category(tx: &mut Transaction<'_, Sqlite>, name: &str) -> ApiResult<i64> {
	let existing: Option<(i64,)> =
		sqlx::query_as("SELECT id FROM category WHERE user_id = 1 AND name = ?")
			.bind(name)
			.fetch_optional(&mut **tx)
			.await?;
	if let Some((id,)) = existing {
		return Ok(id);
	}
	let (next,): (i64,) =
		sqlx::query_as("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM category WHERE user_id = 1")
			.fetch_one(&mut **tx)
			.await?;
	let result = sqlx::query(
		"INSERT INTO category (user_id, name, sort_order, is_default, include_in_update, include_in_download)
		 VALUES (1, ?, ?, 0, 0, 0)",
	)
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

/// manga 行元数据的中间形态：各导入器把自家备份模型归一到它
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

/// manga 行缺失时按备份元数据补行；initialized=0，等下次源刷新补全
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
	sqlx::query(
		"INSERT OR IGNORE INTO manga (source_id, key, title, authors, artists, description, tags,
		 status, content_rating, cover_url, url, initialized, update_strategy)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)",
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
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

/// 章节行元数据的中间形态
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

/// 章节行缺失时按备份元数据补行（fetched_at=0 标记是备份补的，等源刷新覆盖）
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
	sqlx::query(
		"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators, language,
		 url, date_uploaded, source_order, page_count, fetched_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)",
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
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

/// 补入库旗标：已入库的保留现有 in_library_at（现有条目优先），未入库的用备份的入库时间
pub async fn mark_in_library(
	tx: &mut Transaction<'_, Sqlite>,
	manga_id: i64,
	in_library_at: i64,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at)
		 VALUES (?, 1, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET
		 in_library = 1,
		 in_library_at = CASE WHEN manga_user.in_library != 0 THEN manga_user.in_library_at
		 ELSE excluded.in_library_at END",
	)
	.bind(manga_id)
	.bind(in_library_at)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

/// 进度合并：页码取大、已读取 OR、时间 NULL 安全取新；is_bookmarked 是本机状态，备份不携带也不覆盖
pub async fn merge_chapter_progress(
	tx: &mut Transaction<'_, Sqlite>,
	chapter_id: i64,
	last_page_read: i64,
	is_read: bool,
	last_read_at: Option<i64>,
) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked,
		 last_read_at) VALUES (?, 1, ?, ?, 0, ?)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
		 last_page_read = MAX(chapter_user.last_page_read, excluded.last_page_read),
		 is_read = MAX(chapter_user.is_read, excluded.is_read),
		 last_read_at = CASE
		 WHEN chapter_user.last_read_at IS NULL THEN excluded.last_read_at
		 WHEN excluded.last_read_at IS NULL THEN chapter_user.last_read_at
		 ELSE MAX(chapter_user.last_read_at, excluded.last_read_at) END",
	)
	.bind(chapter_id)
	.bind(last_page_read)
	.bind(is_read)
	.bind(last_read_at)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
