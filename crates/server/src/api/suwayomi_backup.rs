//! Suwayomi/Mihon 备份导入（.tachibk / .proto.gz）：protobuf wire 兼容 Mihon
//! backup schema，容器按魔数嗅探——两家实际都写 gzip(proto)（Suwayomi
//! ProtoBackupExport、Mihon BackupCreator 均过 okio gzip），Mihon 还接受裸
//! proto；zip 容器是对 fork 的防御性兼容。additive 合并原语见 import_merge。
//!
//! prost 消息为手写 wire 契约（字段号出处见 PR）：未声明的字段（tracking、
//! memo、preferences、suwayomi meta 9000+ 等）prost 解码时跳过

use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::Json;
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::backup::BODY_MAX;
use super::import_merge::{
	ChapterMeta, ImportCounts, MangaMeta, ensure_category, ensure_chapter_row, ensure_manga_row,
	mark_in_library, merge_chapter_progress,
};

/// 解压/解包后的 proto 上限：与上传 body 上限同档，防 zip/gzip bomb
const PROTO_MAX: u64 = BODY_MAX as u64;

#[derive(Clone, PartialEq, Message)]
struct TachiBackup {
	#[prost(message, repeated, tag = "1")]
	manga: Vec<TachiManga>,
	#[prost(message, repeated, tag = "2")]
	categories: Vec<TachiCategory>,
	#[prost(message, repeated, tag = "101")]
	sources: Vec<TachiSource>,
}

#[derive(Clone, PartialEq, Message)]
struct TachiManga {
	#[prost(int64, tag = "1")]
	source: i64,
	/// 源内键（1.x 里叫 key），不是可浏览的 URL
	#[prost(string, tag = "2")]
	url: String,
	#[prost(string, tag = "3")]
	title: String,
	#[prost(string, optional, tag = "4")]
	artist: Option<String>,
	#[prost(string, optional, tag = "5")]
	author: Option<String>,
	#[prost(string, optional, tag = "6")]
	description: Option<String>,
	#[prost(string, repeated, tag = "7")]
	genre: Vec<String>,
	#[prost(int32, tag = "8")]
	status: i32,
	#[prost(string, optional, tag = "9")]
	thumbnail_url: Option<String>,
	#[prost(int64, tag = "13")]
	date_added: i64,
	#[prost(message, repeated, tag = "16")]
	chapters: Vec<TachiChapter>,
	/// 分类 order 值（Mihon MangaRestorer 按 order 回查），不是数组位置
	#[prost(int64, repeated, tag = "17")]
	categories: Vec<i64>,
	/// kotlinx ProtoBuf 省略默认值字段，favorite=true 不上线，缺席即 true
	#[prost(bool, optional, tag = "100")]
	favorite: Option<bool>,
	#[prost(message, repeated, tag = "104")]
	history: Vec<TachiHistory>,
}

#[derive(Clone, PartialEq, Message)]
struct TachiChapter {
	#[prost(string, tag = "1")]
	url: String,
	#[prost(string, tag = "2")]
	name: String,
	#[prost(string, optional, tag = "3")]
	scanlator: Option<String>,
	#[prost(bool, tag = "4")]
	read: bool,
	#[prost(int64, tag = "6")]
	last_page_read: i64,
	#[prost(int64, tag = "8")]
	date_upload: i64,
	#[prost(float, tag = "9")]
	chapter_number: f32,
	#[prost(int64, tag = "10")]
	source_order: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiCategory {
	#[prost(string, tag = "1")]
	name: String,
	#[prost(int64, tag = "2")]
	order: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiSource {
	#[prost(string, tag = "1")]
	name: String,
	#[prost(int64, tag = "2")]
	source_id: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiHistory {
	#[prost(string, tag = "1")]
	url: String,
	#[prost(int64, tag = "2")]
	last_read: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuwayomiImportQuery {
	#[serde(default)]
	pub dry_run: bool,
	/// urlencoded JSON：{ "<mihonSourceId>": "<aidokuSourceKey>" }
	#[serde(default)]
	pub source_mapping: Option<String>,
}

/// 备份时间戳是 epoch 毫秒，0 表示无值（Mihon toInstantOrNull 约定）
fn ms_to_unix(ms: i64) -> Option<i64> {
	(ms > 0).then_some(ms / 1000)
}

/// 备份 status 枚举（SManga：1 连载 2 完结 3 授权 4 出版完 5 砍 6 休刊）映射到本体的 1/2/3/4
fn status_from_tachi(status: i32) -> i64 {
	match status {
		1 => 1,
		2 => 2,
		5 => 3,
		6 => 4,
		_ => 0,
	}
}

/// 归一化只留字母数字并小写（Unicode 感知，CJK 源名不会归一成空串）
fn normalize_source_name(name: &str) -> String {
	name.chars()
		.filter(|c| c.is_alphanumeric())
		.flat_map(char::to_lowercase)
		.collect()
}

fn gunzip(bytes: &[u8]) -> ApiResult<Vec<u8>> {
	use std::io::Read;
	let mut out = Vec::new();
	flate2::read::GzDecoder::new(bytes)
		.take(PROTO_MAX + 1)
		.read_to_end(&mut out)
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	if out.len() as u64 > PROTO_MAX {
		return Err(ApiError::bad_request("suwayomi backup too large"));
	}
	Ok(out)
}

/// zip 容器：取 backup.proto，否则唯一的 .proto 条目；条目本身可能还是 gzip
fn unzip_proto(bytes: &[u8], depth: u8) -> ApiResult<Vec<u8>> {
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	let names: Vec<String> = archive
		.file_names()
		.map(|n| n.map(|cow| cow.into_owned()))
		.collect::<Result<Vec<_>, _>>()
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	let protos: Vec<&str> = names
		.iter()
		.map(String::as_str)
		.filter(|n| n.ends_with(".proto"))
		.collect();
	let entry = if names.iter().any(|n| n == "backup.proto") {
		"backup.proto"
	} else if let [only] = protos.as_slice() {
		only
	} else {
		return Err(ApiError::bad_request("suwayomi backup zip has no proto entry"));
	};
	let inner = aidoku_wasm::package::read_zip_entry(&mut archive, entry, PROTO_MAX)
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	unpack(&inner, depth + 1)
}

fn unpack(bytes: &[u8], depth: u8) -> ApiResult<Vec<u8>> {
	if bytes.starts_with(b"\x1f\x8b") {
		return gunzip(bytes);
	}
	if bytes.starts_with(b"PK\x03\x04") {
		if depth >= 1 {
			return Err(ApiError::bad_request("invalid suwayomi backup: nested zip"));
		}
		return unzip_proto(bytes, depth);
	}
	Ok(bytes.to_vec())
}

/// 按 Mihon 源 id 分组（保持备份内首次出现顺序，报告稳定）
fn group_by_source(doc: &TachiBackup) -> Vec<(i64, Vec<&TachiManga>)> {
	let mut groups: Vec<(i64, Vec<&TachiManga>)> = Vec::new();
	let mut index: HashMap<i64, usize> = HashMap::new();
	for manga in &doc.manga {
		let pos = match index.get(&manga.source) {
			Some(&pos) => pos,
			None => {
				let pos = groups.len();
				index.insert(manga.source, pos);
				groups.push((manga.source, Vec::new()));
				pos
			}
		};
		groups[pos].1.push(manga);
	}
	groups
}

#[derive(Debug, sqlx::FromRow)]
struct InstalledSource {
	id: i64,
	key: String,
	name: String,
}

/// 一个 Mihon 源组的归处：显式映射 > 唯一命中自动匹配 > unmatched（多命中绝不猜）
enum Resolution {
	Mapped { key: String },
	Auto { key: String },
	Unmatched { candidates: Vec<(String, String)> },
}

impl Resolution {
	fn key(&self) -> Option<&str> {
		match self {
			Resolution::Mapped { key } | Resolution::Auto { key } => Some(key),
			Resolution::Unmatched { .. } => None,
		}
	}
}

fn resolve_group(
	mihon_id: i64,
	name: &str,
	mapping: &HashMap<i64, String>,
	installed: &[InstalledSource],
	by_normalized_name: &HashMap<String, Vec<usize>>,
) -> Resolution {
	if let Some(key) = mapping.get(&mihon_id) {
		return Resolution::Mapped { key: key.clone() };
	}
	let normalized = normalize_source_name(name);
	let candidates: Vec<(String, String)> = by_normalized_name
		.get(&normalized)
		.map(|ids| {
			ids.iter()
				.map(|i| (installed[*i].key.clone(), installed[*i].name.clone()))
				.collect()
		})
		.unwrap_or_default();
	// 名字归一化为空（如纯标点）没有匹配资格，唯一命中才自动匹配
	if !normalized.is_empty() && candidates.len() == 1 {
		return Resolution::Auto {
			key: candidates[0].0.clone(),
		};
	}
	Resolution::Unmatched { candidates }
}

fn parse_mapping(raw: Option<&str>) -> ApiResult<HashMap<i64, String>> {
	let Some(raw) = raw else {
		return Ok(HashMap::new());
	};
	let parsed: HashMap<String, String> = serde_json::from_str(raw)
		.map_err(|e| ApiError::bad_request(format!("invalid sourceMapping: {e}")))?;
	let mut out = HashMap::new();
	for (k, v) in parsed {
		let id: i64 = k
			.parse()
			.map_err(|_| ApiError::bad_request(format!("invalid sourceMapping key: {k}")))?;
		out.insert(id, v);
	}
	Ok(out)
}

pub async fn import_suwayomi_backup(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<SuwayomiImportQuery>,
	body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
	let mapping = parse_mapping(query.source_mapping.as_deref())?;
	let result = import_suwayomi_doc(&state, user.id, &body, query.dry_run, &mapping).await?;
	Ok(Json(result))
}

/// 校验 + 单事务导入；dry-run 走同一执行路径，末尾回滚代替提交，报告与真跑一致
pub async fn import_suwayomi_doc(
	state: &AppState,
	user_id: i64,
	body: &[u8],
	dry_run: bool,
	mapping: &HashMap<i64, String>,
) -> ApiResult<Value> {
	let proto = unpack(body, 0)?;
	let doc = TachiBackup::decode(proto.as_slice())
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;

	// 键是后续一切对账的锚点，空键没法安全落库
	for manga in &doc.manga {
		if manga.url.is_empty() {
			return Err(ApiError::bad_request("suwayomi manga entry must have a non-empty url"));
		}
		if manga.chapters.iter().any(|c| c.url.is_empty())
			|| manga.history.iter().any(|h| h.url.is_empty())
		{
			return Err(ApiError::bad_request(
				"suwayomi chapter/history entry must have a non-empty url",
			));
		}
	}

	let source_names: HashMap<i64, &str> = doc
		.sources
		.iter()
		.map(|s| (s.source_id, s.name.as_str()))
		.collect();
	let installed: Vec<InstalledSource> =
		sqlx::query_as("SELECT id, key, name FROM source ORDER BY id")
			.fetch_all(&state.pool)
			.await?;
	let installed_id_by_key: HashMap<&str, i64> = installed
		.iter()
		.map(|s| (s.key.as_str(), s.id))
		.collect();
	for key in mapping.values() {
		if !installed_id_by_key.contains_key(key.as_str()) {
			return Err(ApiError::bad_request(format!(
				"sourceMapping target not installed: {key}"
			)));
		}
	}
	let mut by_normalized_name: HashMap<String, Vec<usize>> = HashMap::new();
	for (i, s) in installed.iter().enumerate() {
		let normalized = normalize_source_name(&s.name);
		if !normalized.is_empty() {
			by_normalized_name.entry(normalized).or_default().push(i);
		}
	}

	let groups = group_by_source(&doc);
	let resolutions: Vec<Resolution> = groups
		.iter()
		.map(|(id, _)| {
			resolve_group(
				*id,
				source_names.get(id).copied().unwrap_or(""),
				mapping,
				&installed,
				&by_normalized_name,
			)
		})
		.collect();

	let mut imported = ImportCounts::default();
	let mut skipped: Vec<Value> = Vec::new();
	let mut seen_categories: HashSet<String> = HashSet::new();
	{
		let mut tx = state.pool.begin().await?;
		import_categories(&mut tx, user_id, &doc, &mut seen_categories).await?;
		for ((mihon_id, mangas), resolution) in groups.iter().zip(&resolutions) {
			let name = source_names.get(mihon_id).copied().unwrap_or("");
			let Some(source_key) = resolution.key() else {
				for manga in mangas {
					skipped.push(json!({
						"section": if manga.favorite.unwrap_or(true) { "library" } else { "progress" },
						"mihonSourceId": mihon_id.to_string(),
						"mihonSourceName": name,
						"mangaKey": manga.url,
						"reason": "no source mapping",
					}));
				}
				continue;
			};
			let source_id = installed_id_by_key[source_key];
			import_group(&mut tx, user_id, &doc, source_id, mangas, &mut imported, &mut seen_categories)
				.await?;
		}
		imported.categories = seen_categories.len() as i64;
		if dry_run {
			tx.rollback().await?;
		} else {
			tx.commit().await?;
		}
	}

	let sources_json: Vec<Value> = groups
		.iter()
		.zip(&resolutions)
		.map(|((mihon_id, mangas), resolution)| {
			let name = source_names.get(mihon_id).copied().unwrap_or("");
			let (state_label, key) = match resolution {
				Resolution::Mapped { key } => ("mapped", Some(key.as_str())),
				Resolution::Auto { key } => ("auto", Some(key.as_str())),
				Resolution::Unmatched { .. } => ("unmatched", None),
			};
			let candidates: Vec<Value> = match resolution {
				Resolution::Unmatched { candidates } => candidates
					.iter()
					.map(|(k, n)| json!({"key": k, "name": n}))
					.collect(),
				_ => Vec::new(),
			};
			let mut value = json!({
				"mihonSourceId": mihon_id.to_string(),
				"mihonSourceName": name,
				"mangaCount": mangas.len(),
				"match": state_label,
				"candidates": candidates,
			});
			if let Some(key) = key {
				value["aidokuSourceKey"] = json!(key);
				if let Some(s) = installed.iter().find(|s| s.key == key) {
					value["aidokuSourceName"] = json!(s.name);
				}
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
		"skipped": skipped,
		"sources": sources_json,
	}))
}

/// 顶层 categories 数组先建行；成员引用按 order 回查这张表
async fn import_categories(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &TachiBackup,
	seen: &mut HashSet<String>,
) -> ApiResult<()> {
	for category in &doc.categories {
		let name = category.name.trim();
		if name.is_empty() {
			return Err(ApiError::bad_request("suwayomi category must have a non-empty name"));
		}
		ensure_category(tx, user_id, name).await?;
		seen.insert(name.to_string());
	}
	Ok(())
}

async fn import_group(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &TachiBackup,
	source_id: i64,
	mangas: &[&TachiManga],
	imported: &mut ImportCounts,
	seen_categories: &mut HashSet<String>,
) -> ApiResult<()> {
	// 分类成员是 order 值回查（Mihon MangaRestorer 的语义），不是数组位置
	let category_by_order: HashMap<i64, &str> = doc
		.categories
		.iter()
		.map(|c| (c.order, c.name.as_str()))
		.collect();
	for manga in mangas {
		let meta = MangaMeta {
			title: manga.title.as_str(),
			authors: serde_json::to_string(manga.author.as_slice())?,
			artists: serde_json::to_string(manga.artist.as_slice())?,
			description: manga.description.as_deref(),
			tags: serde_json::to_string(&manga.genre)?,
			status: status_from_tachi(manga.status),
			content_rating: 0,
			cover_url: manga.thumbnail_url.as_deref(),
			// url 列是可浏览的源站页面，manga.url 是源内键，不放过去
			url: None,
		};
		let manga_id = ensure_manga_row(tx, source_id, &manga.url, Some(&meta)).await?;
		if manga.favorite.unwrap_or(true) {
			let in_library_at = ms_to_unix(manga.date_added).unwrap_or_else(unix_now);
			mark_in_library(tx, user_id, manga_id, in_library_at).await?;
			for order in &manga.categories {
				let Some(name) = category_by_order.get(order) else {
					continue;
				};
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

		let history: HashMap<&str, i64> = manga
			.history
			.iter()
			.map(|h| (h.url.as_str(), h.last_read))
			.collect();
		let mut seen_chapters: HashSet<&str> = HashSet::new();
		for chapter in &manga.chapters {
			seen_chapters.insert(chapter.url.as_str());
			let meta = ChapterMeta {
				title: (!chapter.name.is_empty()).then_some(chapter.name.as_str()),
				number: (chapter.chapter_number != 0.0).then_some(chapter.chapter_number as f64),
				volume: None,
				scanlators: serde_json::to_string(chapter.scanlator.as_slice())?,
				language: None,
				url: None,
				date_uploaded: ms_to_unix(chapter.date_upload),
				source_order: chapter.source_order,
			};
			let chapter_id = ensure_chapter_row(tx, manga_id, &chapter.url, Some(&meta)).await?;
			let last_read_at = history
				.get(chapter.url.as_str())
				.and_then(|ts| ms_to_unix(*ts));
			merge_chapter_progress(
				tx,
				user_id,
				chapter_id,
				chapter.last_page_read.max(0),
				chapter.read,
				last_read_at,
			)
			.await?;
			imported.progress += 1;
		}
		// 只在 history 出现的章节（备份勾了 history 没勾 chapters）：裸键补行，只带时间
		for entry in &manga.history {
			if seen_chapters.contains(entry.url.as_str()) {
				continue;
			}
			let chapter_id = ensure_chapter_row(tx, manga_id, &entry.url, None).await?;
			merge_chapter_progress(tx, user_id, chapter_id, 0, false, ms_to_unix(entry.last_read)).await?;
			imported.progress += 1;
		}
	}
	Ok(())
}
