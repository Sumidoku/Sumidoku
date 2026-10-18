//! Aidoku .aib 导入：binary plist 编码的 Codable Backup（Aidoku/Aidoku Core/Backup/Models），
//! 语义与 BackupDoc 恢复的 upsert 不同——additive：已存在库条目不覆盖，
//! 进度取 max(last_page_read)、is_read 取 OR、last_read_at 取较新

use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AidokuImportQuery {
	#[serde(default)]
	pub dry_run: bool,
}

// 以下结构只声明消费的字段，未知字段容忍忽略；字段名对齐 Aidoku 的 Codable 键
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AibBackup {
	#[serde(default)]
	library: Vec<AibLibraryManga>,
	#[serde(default)]
	history: Vec<AibHistory>,
	#[serde(default)]
	manga: Vec<AibManga>,
	#[serde(default)]
	chapters: Vec<AibChapter>,
	// Aidoku 会写出裸字符串或 {title,sort,group,data} 对象两种形态，按 Value 手工取 title
	#[serde(default)]
	categories: Vec<plist::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AibLibraryManga {
	manga_id: String,
	source_id: String,
	#[serde(default)]
	date_added: Option<plist::Date>,
	#[serde(default)]
	categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AibHistory {
	source_id: String,
	manga_id: String,
	chapter_id: String,
	#[serde(default)]
	progress: Option<i64>,
	#[serde(default)]
	completed: bool,
	#[serde(default)]
	date_read: Option<plist::Date>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AibManga {
	id: String,
	source_id: String,
	#[serde(default)]
	title: String,
	#[serde(default)]
	author: Option<String>,
	#[serde(default)]
	artist: Option<String>,
	#[serde(default)]
	desc: Option<String>,
	#[serde(default)]
	tags: Vec<String>,
	#[serde(default)]
	cover: Option<String>,
	#[serde(default)]
	url: Option<String>,
	#[serde(default)]
	status: i64,
	#[serde(default)]
	nsfw: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AibChapter {
	id: String,
	source_id: String,
	manga_id: String,
	#[serde(default)]
	title: Option<String>,
	#[serde(default)]
	scanlator: Option<String>,
	#[serde(default)]
	url: Option<String>,
	#[serde(default)]
	lang: Option<String>,
	#[serde(default)]
	chapter: Option<f64>,
	#[serde(default)]
	volume: Option<f64>,
	#[serde(default)]
	date_uploaded: Option<plist::Date>,
	#[serde(default)]
	source_order: i64,
}

#[derive(Debug, Default)]
struct Imported {
	library: i64,
	categories: i64,
	progress: i64,
}

#[derive(Debug)]
struct Skipped {
	section: &'static str,
	source_key: String,
	manga_key: String,
	chapter_key: Option<String>,
	reason: &'static str,
}

/// Swift Date 在 plist 里是 CFAbsoluteTime，plist crate 已归一到 SystemTime
fn date_to_unix(date: plist::Date) -> i64 {
	let time: std::time::SystemTime = date.into();
	time.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs() as i64)
		.unwrap_or(0)
}

/// Aidoku 存的是 contentRating.rawValue - 1（>0 时），0 混淆了 unknown/safe，按 unknown 处理
fn content_rating_from_aib(nsfw: i64) -> i64 {
	match nsfw {
		1 => 2,
		2 => 3,
		_ => 0,
	}
}

fn category_title(value: &plist::Value) -> Option<String> {
	match value {
		plist::Value::String(title) => Some(title.clone()),
		plist::Value::Dictionary(dict) => dict
			.get("title")
			.and_then(plist::Value::as_string)
			.map(str::to_string),
		_ => None,
	}
}

pub async fn import_aidoku_backup(
	State(state): State<Arc<AppState>>,
	Query(query): Query<AidokuImportQuery>,
	body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
	let result = import_aidoku_doc(&state, &body, query.dry_run).await?;
	Ok(Json(result))
}

/// 校验 + 单事务导入；dry-run 走同一执行路径，末尾回滚代替提交，报告与真跑一致
pub async fn import_aidoku_doc(state: &AppState, body: &[u8], dry_run: bool) -> ApiResult<Value> {
	let doc: AibBackup = plist::from_bytes(body)
		.map_err(|e| ApiError::bad_request(format!("invalid aidoku backup: {e}")))?;

	let manga_meta: HashMap<(&str, &str), &AibManga> = doc
		.manga
		.iter()
		.map(|m| ((m.source_id.as_str(), m.id.as_str()), m))
		.collect();
	let chapter_meta: HashMap<(&str, &str, &str), &AibChapter> = doc
		.chapters
		.iter()
		.map(|c| ((c.source_id.as_str(), c.manga_id.as_str(), c.id.as_str()), c))
		.collect();

	let mut imported = Imported::default();
	let mut skipped: Vec<Skipped> = Vec::new();
	let mut seen_categories: HashSet<String> = HashSet::new();
	{
		let mut tx = state.pool.begin().await?;
		import_categories(&mut tx, &doc, &mut seen_categories).await?;
		import_library(&mut tx, &doc, &manga_meta, &mut imported, &mut skipped, &mut seen_categories)
			.await?;
		import_progress(&mut tx, &doc, &manga_meta, &chapter_meta, &mut imported, &mut skipped)
			.await?;
		imported.categories = seen_categories.len() as i64;
		if dry_run {
			tx.rollback().await?;
		} else {
			tx.commit().await?;
		}
	}
	let skipped_json: Vec<Value> = skipped
		.iter()
		.map(|s| {
			let mut value = json!({
				"section": s.section,
				"sourceKey": s.source_key,
				"mangaKey": s.manga_key,
				"reason": s.reason,
			});
			if let Some(chapter_key) = &s.chapter_key {
				value["chapterKey"] = json!(chapter_key);
			}
			value
		})
		.collect();
	Ok(json!({
		"dryRun": dry_run,
		"imported": {
			"library": imported.library,
			"categories": imported.categories,
			"progress": imported.progress,
		},
		"skipped": skipped_json,
	}))
}

/// 顶层 categories 数组先建行；书库条目里只出现引用形态的分类在 import_library 里懒建
async fn import_categories(
	tx: &mut Transaction<'_, Sqlite>,
	doc: &AibBackup,
	seen: &mut HashSet<String>,
) -> ApiResult<()> {
	for value in &doc.categories {
		let title = category_title(value)
			.filter(|t| !t.trim().is_empty())
			.ok_or_else(|| ApiError::bad_request("aidoku category must have a non-empty title"))?;
		ensure_category(tx, title.trim()).await?;
		seen.insert(title.trim().to_string());
	}
	Ok(())
}

/// 同名分类已存在就原样使用（sort_order/三态以现有为准），新分类排到末尾
async fn ensure_category(tx: &mut Transaction<'_, Sqlite>, name: &str) -> ApiResult<i64> {
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

async fn source_id_by_key(tx: &mut Transaction<'_, Sqlite>, key: &str) -> ApiResult<Option<i64>> {
	let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
	Ok(row.map(|(id,)| id))
}

/// manga 行缺失时按备份 manga 元数据补行；initialized=0，等下次源刷新补全
async fn ensure_manga_row(
	tx: &mut Transaction<'_, Sqlite>,
	source_id: i64,
	key: &str,
	meta: Option<&AibManga>,
) -> ApiResult<i64> {
	let (title, authors, artists, description, tags, status, content_rating, cover_url, url) =
		match meta {
			Some(m) => (
				m.title.as_str(),
				serde_json::to_string(&m.author.as_slice())?,
				serde_json::to_string(&m.artist.as_slice())?,
				m.desc.as_deref(),
				serde_json::to_string(&m.tags)?,
				m.status,
				content_rating_from_aib(m.nsfw),
				m.cover.as_deref(),
				m.url.as_deref(),
			),
			None => (
				"",
				"[]".to_string(),
				"[]".to_string(),
				None,
				"[]".to_string(),
				0,
				0,
				None,
				None,
			),
		};
	sqlx::query(
		"INSERT OR IGNORE INTO manga (source_id, key, title, authors, artists, description, tags,
		 status, content_rating, cover_url, url, initialized, update_strategy)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)",
	)
	.bind(source_id)
	.bind(key)
	.bind(title)
	.bind(authors)
	.bind(artists)
	.bind(description)
	.bind(tags)
	.bind(status)
	.bind(content_rating)
	.bind(cover_url)
	.bind(url)
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

/// 章节行缺失时按备份 chapters 元数据补行（fetched_at=0 标记是备份补的，等源刷新覆盖）
async fn ensure_chapter_row(
	tx: &mut Transaction<'_, Sqlite>,
	manga_id: i64,
	key: &str,
	meta: Option<&AibChapter>,
) -> ApiResult<i64> {
	let (title, number, volume, scanlators, language, url, date_uploaded, source_order) = match meta {
		Some(c) => (
			c.title.as_deref(),
			c.chapter,
			c.volume,
			serde_json::to_string(&c.scanlator.as_slice())?,
			c.lang.as_deref(),
			c.url.as_deref(),
			c.date_uploaded.map(date_to_unix),
			c.source_order,
		),
		None => (None, None, None, "[]".to_string(), None, None, None, 0),
	};
	sqlx::query(
		"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators, language,
		 url, date_uploaded, source_order, page_count, fetched_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)",
	)
	.bind(manga_id)
	.bind(key)
	.bind(title)
	.bind(number)
	.bind(volume)
	.bind(scanlators)
	.bind(language)
	.bind(url)
	.bind(date_uploaded)
	.bind(source_order)
	.execute(&mut **tx)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
		.bind(manga_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
	Ok(id)
}

async fn import_library(
	tx: &mut Transaction<'_, Sqlite>,
	doc: &AibBackup,
	manga_meta: &HashMap<(&str, &str), &AibManga>,
	imported: &mut Imported,
	skipped: &mut Vec<Skipped>,
	seen_categories: &mut HashSet<String>,
) -> ApiResult<()> {
	for entry in &doc.library {
		if entry.source_id.is_empty() || entry.manga_id.is_empty() {
			return Err(ApiError::bad_request(
				"aidoku library entry must have non-empty sourceId and mangaId",
			));
		}
		let Some(source_id) = source_id_by_key(tx, &entry.source_id).await? else {
			skipped.push(Skipped {
				section: "library",
				source_key: entry.source_id.clone(),
				manga_key: entry.manga_id.clone(),
				chapter_key: None,
				reason: "source not installed",
			});
			continue;
		};
		let manga_id = ensure_manga_row(
			tx,
			source_id,
			&entry.manga_id,
			manga_meta.get(&(entry.source_id.as_str(), entry.manga_id.as_str())).copied(),
		)
		.await?;
		let in_library_at = entry.date_added.map(date_to_unix).unwrap_or_else(unix_now);
		// 已入库的保留现有 in_library_at（现有条目优先），未入库的补旗标用备份的入库时间
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
		for name in &entry.categories {
			let name = name.trim();
			if name.is_empty() {
				continue;
			}
			let category_id = ensure_category(tx, name).await?;
			seen_categories.insert(name.to_string());
			sqlx::query(
				"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
				 VALUES (?, ?, 1)",
			)
			.bind(category_id)
			.bind(manga_id)
			.execute(&mut **tx)
			.await?;
		}
		imported.library += 1;
	}
	Ok(())
}

async fn import_progress(
	tx: &mut Transaction<'_, Sqlite>,
	doc: &AibBackup,
	manga_meta: &HashMap<(&str, &str), &AibManga>,
	chapter_meta: &HashMap<(&str, &str, &str), &AibChapter>,
	imported: &mut Imported,
	skipped: &mut Vec<Skipped>,
) -> ApiResult<()> {
	for entry in &doc.history {
		if entry.source_id.is_empty() || entry.manga_id.is_empty() || entry.chapter_id.is_empty() {
			return Err(ApiError::bad_request(
				"aidoku history entry must have non-empty sourceId, mangaId and chapterId",
			));
		}
		let Some(source_id) = source_id_by_key(tx, &entry.source_id).await? else {
			skipped.push(Skipped {
				section: "progress",
				source_key: entry.source_id.clone(),
				manga_key: entry.manga_id.clone(),
				chapter_key: Some(entry.chapter_id.clone()),
				reason: "source not installed",
			});
			continue;
		};
		let manga_id = ensure_manga_row(
			tx,
			source_id,
			&entry.manga_id,
			manga_meta.get(&(entry.source_id.as_str(), entry.manga_id.as_str())).copied(),
		)
		.await?;
		let chapter_id = ensure_chapter_row(
			tx,
			manga_id,
			&entry.chapter_id,
			chapter_meta
				.get(&(
					entry.source_id.as_str(),
					entry.manga_id.as_str(),
					entry.chapter_id.as_str(),
				))
				.copied(),
		)
		.await?;
		// Aidoku 里 -1 是「读完」标记不是页码，负数一律按 0 处理
		let last_page_read = entry.progress.unwrap_or(0).max(0);
		let last_read_at = entry.date_read.map(date_to_unix);
		// 合并：页码取大、已读取 OR、时间取新；is_bookmarked 是本机状态，备份不携带也不覆盖
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
		.bind(entry.completed)
		.bind(last_read_at)
		.execute(&mut **tx)
		.await?;
		imported.progress += 1;
	}
	Ok(())
}
