//! Aidoku .aib 导入：binary plist 编码的 Codable Backup（Aidoku/Aidoku Core/Backup/Models），
//! additive 合并语义与 BackupDoc 恢复的 upsert 不同，合并原语见 import_merge

use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::import_merge::{
	ChapterMeta, ImportCounts, MangaMeta, ensure_category, ensure_chapter_row, ensure_manga_row,
	mark_in_library, merge_chapter_progress, source_id_by_key,
};

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

fn aib_manga_meta(m: &AibManga) -> ApiResult<MangaMeta<'_>> {
	Ok(MangaMeta {
		title: m.title.as_str(),
		authors: serde_json::to_string(m.author.as_slice())?,
		artists: serde_json::to_string(m.artist.as_slice())?,
		description: m.desc.as_deref(),
		tags: serde_json::to_string(&m.tags)?,
		status: m.status,
		content_rating: content_rating_from_aib(m.nsfw),
		cover_url: m.cover.as_deref(),
		url: m.url.as_deref(),
	})
}

fn aib_chapter_meta(c: &AibChapter) -> ApiResult<ChapterMeta<'_>> {
	Ok(ChapterMeta {
		title: c.title.as_deref(),
		number: c.chapter,
		volume: c.volume,
		scanlators: serde_json::to_string(c.scanlator.as_slice())?,
		language: c.lang.as_deref(),
		url: c.url.as_deref(),
		date_uploaded: c.date_uploaded.map(date_to_unix),
		source_order: c.source_order,
	})
}

pub async fn import_aidoku_backup(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<AidokuImportQuery>,
	body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
	let result = import_aidoku_doc(&state, user.id, &body, query.dry_run).await?;
	Ok(Json(result))
}

/// 校验 + 单事务导入；dry-run 走同一执行路径，末尾回滚代替提交，报告与真跑一致
pub async fn import_aidoku_doc(state: &AppState, user_id: i64, body: &[u8], dry_run: bool) -> ApiResult<Value> {
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

	let mut imported = ImportCounts::default();
	let mut skipped: Vec<Skipped> = Vec::new();
	let mut seen_categories: HashSet<String> = HashSet::new();
	{
		let mut tx = crate::db::begin_write(&state.pool).await?;
		import_categories(&mut tx, user_id, &doc, &mut seen_categories).await?;
		import_library(&mut tx, user_id, &doc, &manga_meta, &mut imported, &mut skipped, &mut seen_categories)
			.await?;
		import_progress(&mut tx, user_id, &doc, &manga_meta, &chapter_meta, &mut imported, &mut skipped)
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
	user_id: i64,
	doc: &AibBackup,
	seen: &mut HashSet<String>,
) -> ApiResult<()> {
	for value in &doc.categories {
		let title = category_title(value)
			.filter(|t| !t.trim().is_empty())
			.ok_or_else(|| ApiError::bad_request("aidoku category must have a non-empty title"))?;
		ensure_category(tx, user_id, title.trim()).await?;
		seen.insert(title.trim().to_string());
	}
	Ok(())
}

async fn import_library(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &AibBackup,
	manga_meta: &HashMap<(&str, &str), &AibManga>,
	imported: &mut ImportCounts,
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
		let meta = manga_meta
			.get(&(entry.source_id.as_str(), entry.manga_id.as_str()))
			.map(|m| aib_manga_meta(m))
			.transpose()?;
		let manga_id = ensure_manga_row(tx, source_id, &entry.manga_id, meta.as_ref()).await?;
		let in_library_at = entry.date_added.map(date_to_unix).unwrap_or_else(unix_now);
		mark_in_library(tx, user_id, manga_id, in_library_at).await?;
		for name in &entry.categories {
			let name = name.trim();
			if name.is_empty() {
				continue;
			}
			let category_id = ensure_category(tx, user_id, name).await?;
			seen_categories.insert(name.to_string());
			sqlx::query(
				"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
				 VALUES (?, ?, ?)",
			)
			.bind(category_id)
			.bind(manga_id)
			.bind(user_id)
			.execute(&mut **tx)
			.await?;
		}
		imported.library += 1;
	}
	Ok(())
}

async fn import_progress(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &AibBackup,
	manga_meta: &HashMap<(&str, &str), &AibManga>,
	chapter_meta: &HashMap<(&str, &str, &str), &AibChapter>,
	imported: &mut ImportCounts,
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
		let meta = manga_meta
			.get(&(entry.source_id.as_str(), entry.manga_id.as_str()))
			.map(|m| aib_manga_meta(m))
			.transpose()?;
		let manga_id = ensure_manga_row(tx, source_id, &entry.manga_id, meta.as_ref()).await?;
		let meta = chapter_meta
			.get(&(
				entry.source_id.as_str(),
				entry.manga_id.as_str(),
				entry.chapter_id.as_str(),
			))
			.map(|c| aib_chapter_meta(c))
			.transpose()?;
		let chapter_id = ensure_chapter_row(tx, manga_id, &entry.chapter_id, meta.as_ref()).await?;
		// Aidoku 里 -1 是「读完」标记不是页码，负数一律按 0 处理
		let last_page_read = entry.progress.unwrap_or(0).max(0);
		let last_read_at = entry.date_read.map(date_to_unix);
		merge_chapter_progress(tx, user_id, chapter_id, last_page_read, entry.completed, last_read_at).await?;
		imported.progress += 1;
	}
	Ok(())
}
