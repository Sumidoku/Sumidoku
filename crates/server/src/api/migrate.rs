use super::repo::source_json;
use super::{attach_internal_ids, pins, source_row};
use crate::error::{ApiError, ApiResult};
use crate::model::{SourceRow, unix_now};
use crate::source::{manga_row, reconcile_chapters, repo, upsert_manga};
use crate::state::AppState;
use aidoku::Manga;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// 迁移源清单：已装源全量下发（目标组合选择器也要用），mangaCount 是在库条数
pub async fn migrate_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at FROM source ORDER BY id",
	)
	.fetch_all(&state.pool)
	.await?;
	let counts: HashMap<i64, i64> = sqlx::query_as(
		"SELECT m.source_id, COUNT(*) FROM manga m
		 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1 AND mu.in_library = 1
		 GROUP BY m.source_id",
	)
	.fetch_all(&state.pool)
	.await?
	.into_iter()
	.collect();
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = pins::pinned_map(&state.pool).await?;
	let out: Vec<Value> = rows
		.iter()
		.map(|row| {
			let mut value = source_json(row, &latest, &pinned);
			if let Some(object) = value.as_object_mut() {
				object.insert(
					"mangaCount".into(),
					json!(counts.get(&row.id).copied().unwrap_or(0)),
				);
			}
			value
		})
		.collect();
	Ok(Json(json!(out)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupQuery {
	pub manga_id: i64,
	pub target_source_id: i64,
	pub query: Option<String>,
}

/// 跨源标题搜索：走源的正常搜索路径（含源的限速），候选附已入库的内部 id；
/// query 缺省用源条目标题，批量行的「手动重搜」靠显式 query
pub async fn migrate_lookup(
	State(state): State<Arc<AppState>>,
	Query(query): Query<LookupQuery>,
) -> ApiResult<Json<Value>> {
	let origin = manga_row(&state.pool, query.manga_id).await?;
	source_row(&state, query.target_source_id).await?;
	let title = query
		.query
		.filter(|q| !q.trim().is_empty())
		.unwrap_or(origin.title);
	if title.trim().is_empty() {
		return Err(ApiError::bad_request("lookup query is empty"));
	}
	let handle = state.get_handle(query.target_source_id).await?;
	let result = handle
		.get_search_manga_list(Some(title.clone()), 1, Vec::new())
		.await?;
	let candidates = attach_internal_ids(&state, query.target_source_id, result.entries).await?;
	Ok(Json(json!({
		"query": title,
		"candidates": candidates,
	})))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MigrateMode {
	Copy,
	Migrate,
	MigrateWithHistory,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateEntry {
	pub from_manga_id: i64,
	pub to_source_id: i64,
	pub to_manga_key: String,
	/// 客户端手里的候选条目；有它就以它为 get_manga_update 入参（同 register_manga 的语义）
	pub to_manga: Option<Manga>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateRequest {
	pub mode: MigrateMode,
	pub entries: Vec<MigrateEntry>,
}

struct EntryOutcome {
	to_manga_id: i64,
	warnings: Vec<String>,
}

struct EntryFailure {
	reason: &'static str,
	message: Option<String>,
}

/// 条目级失败与 500 分开：reason 进逐条结果，数据库类错误照常掀掉整个请求
enum EntryRun {
	Ok(EntryOutcome),
	Failed(EntryFailure),
}

fn failed(reason: &'static str) -> EntryRun {
	EntryRun::Failed(EntryFailure {
		reason,
		message: None,
	})
}

fn failed_with(reason: &'static str, message: String) -> EntryRun {
	EntryRun::Failed(EntryFailure {
		reason,
		message: Some(message),
	})
}

/// 手动解析 body：axum 的 Json 拒绝是 422，端点约定非法形状/非法 mode 400
pub async fn migrate_execute(
	State(state): State<Arc<AppState>>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let body = body.ok_or_else(|| ApiError::bad_request("missing body"))?;
	let request: MigrateRequest = serde_json::from_value(body.0)
		.map_err(|e| ApiError::bad_request(format!("invalid migrate request: {e}")))?;
	if request.entries.is_empty() {
		return Err(ApiError::bad_request("entries must not be empty"));
	}
	for entry in &request.entries {
		if entry.to_manga_key.is_empty() {
			return Err(ApiError::bad_request("toMangaKey must not be empty"));
		}
	}
	// 批次串行：单条目一个事务，部分失败不回滚已完成条目，逐条给 ok/reason
	let mut results = Vec::with_capacity(request.entries.len());
	for entry in &request.entries {
		let run = run_entry(&state, request.mode, entry).await?;
		let value = match run {
			EntryRun::Ok(outcome) => json!({
				"fromMangaId": entry.from_manga_id,
				"ok": true,
				"toMangaId": outcome.to_manga_id,
				"warnings": outcome.warnings,
			}),
			EntryRun::Failed(failure) => json!({
				"fromMangaId": entry.from_manga_id,
				"ok": false,
				"reason": failure.reason,
				"message": failure.message,
			}),
		};
		results.push(value);
	}
	Ok(Json(json!({ "results": results })))
}

async fn run_entry(
	state: &Arc<AppState>,
	mode: MigrateMode,
	entry: &MigrateEntry,
) -> ApiResult<EntryRun> {
	let Ok(origin) = manga_row(&state.pool, entry.from_manga_id).await else {
		return Ok(failed("not_found"));
	};
	if source_row(state, entry.to_source_id).await.is_err() {
		return Ok(failed("source_not_found"));
	}
	// 同源不同 key 是 breaking-change 的 forceMigrate 回退路径（新 key 另起一行，
	// 进度按章节号搬）；只有原地搬自己（同 key）会清掉自身进度，必须拒
	if origin.source_id == entry.to_source_id && origin.key == entry.to_manga_key {
		return Ok(failed("same_source"));
	}
	let origin_user: Option<(bool, Option<i64>)> =
		sqlx::query_as("SELECT in_library, in_library_at FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(origin.id)
			.fetch_optional(&state.pool)
			.await?;
	if !origin_user.map(|(flag, _)| flag).unwrap_or(false) {
		return Ok(failed("not_in_library"));
	}

	// 目标已存在且有章节就直接复用；否则回源拉详情顺带初始化章节（事务外，拿不到算条目失败）
	let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(entry.to_source_id)
		.bind(&entry.to_manga_key)
		.fetch_optional(&state.pool)
		.await?;
	let mut to_manga_id = None;
	let mut fetched = None;
	if let Some((id,)) = existing {
		let (chapters,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(id)
			.fetch_one(&state.pool)
			.await?;
		if chapters > 0 {
			to_manga_id = Some(id);
		}
	}
	if to_manga_id.is_none() {
		let mut initial = entry.to_manga.clone().unwrap_or_default();
		initial.key = entry.to_manga_key.clone();
		let handle = state.get_handle(entry.to_source_id).await?;
		match handle.get_manga_update(initial, true, true).await {
			Ok(updated) => fetched = Some(updated),
			Err(e) => return Ok(failed_with("fetch_failed", e.to_string())),
		}
	}

	let mut warnings = Vec::new();
	let mut tx = state.pool.begin().await?;
	let to_manga_id = match (to_manga_id, &fetched) {
		(Some(id), _) => id,
		(None, Some(updated)) => {
			let id = upsert_manga(&mut tx, entry.to_source_id, updated, true, true, true).await?;
			reconcile_chapters(&mut tx, id, &updated.chapters.clone().unwrap_or_default()).await?;
			id
		}
		(None, None) => unreachable!("fetched is Some when to_manga_id is None"),
	};

	// 入库态搬运：目标已在库保留它自己的首次入库时间，否则继承源条目的
	let origin_at: Option<i64> =
		sqlx::query_scalar("SELECT in_library_at FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(origin.id)
			.fetch_optional(&mut *tx)
			.await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, 1, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(to_manga_id)
	.bind(origin_at.unwrap_or_else(unix_now))
	.execute(&mut *tx)
	.await?;

	// 分类是并集：目标原有分类保留，源条目的补进来
	let category_ids: Vec<i64> =
		sqlx::query_scalar("SELECT category_id FROM category_manga WHERE manga_id = ? AND user_id = 1")
			.bind(origin.id)
			.fetch_all(&mut *tx)
			.await?;
	for category_id in &category_ids {
		sqlx::query("INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
			.bind(category_id)
			.bind(to_manga_id)
			.execute(&mut *tx)
			.await?;
	}

	let origin_progress: Vec<OriginProgress> = sqlx::query_as(
		"SELECT c.key, c.number, c.volume, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		 WHERE cu.user_id = 1 AND c.manga_id = ?
		 AND (cu.last_page_read > 0 OR cu.is_read = 1 OR cu.is_bookmarked = 1 OR cu.last_read_at IS NOT NULL)",
	)
	.bind(origin.id)
	.fetch_all(&mut *tx)
	.await?;
	let targets: Vec<TargetChapter> = sqlx::query_as(
		"SELECT id, number, volume, source_order FROM chapter WHERE manga_id = ? ORDER BY source_order ASC",
	)
	.bind(to_manga_id)
	.fetch_all(&mut *tx)
	.await?;
	for row in &origin_progress {
		let Some(target) = match_target(&targets, row) else {
			warnings.push(format!(
				"progress not carried: no target chapter matches source chapter {}",
				chapter_label(row)
			));
			continue;
		};
		// 目标已有进度保留较新值：页码取大、已读/书签取或、阅读时间取新
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
			 VALUES (?, 1, ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 last_page_read = MAX(last_page_read, excluded.last_page_read),
			 is_read = MAX(is_read, excluded.is_read),
			 is_bookmarked = MAX(is_bookmarked, excluded.is_bookmarked),
			 last_read_at = CASE
			 WHEN excluded.last_read_at IS NULL THEN last_read_at
			 WHEN last_read_at IS NULL THEN excluded.last_read_at
			 ELSE MAX(last_read_at, excluded.last_read_at) END",
		)
		.bind(target.id)
		.bind(row.last_page_read)
		.bind(row.is_read)
		.bind(row.is_bookmarked)
		.bind(row.last_read_at)
		.execute(&mut *tx)
		.await?;
	}

	match mode {
		MigrateMode::Copy => {}
		MigrateMode::Migrate | MigrateMode::MigrateWithHistory => {
			sqlx::query(
				"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, 1, 0)
				 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
			)
			.bind(origin.id)
			.execute(&mut *tx)
			.await?;
			sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = 1")
				.bind(origin.id)
				.execute(&mut *tx)
				.await?;
			// chapter_user 还背着下载标记，清进度只重置进度字段不删行
			if mode == MigrateMode::Migrate {
				sqlx::query(
					"UPDATE chapter_user SET last_page_read = 0, is_read = 0, is_bookmarked = 0,
					 last_read_at = NULL
					 WHERE user_id = 1 AND chapter_id IN (SELECT id FROM chapter WHERE manga_id = ?)",
				)
				.bind(origin.id)
				.execute(&mut *tx)
				.await?;
			}
		}
	}
	tx.commit().await?;
	Ok(EntryRun::Ok(EntryOutcome {
		to_manga_id,
		warnings,
	}))
}

#[derive(Debug, sqlx::FromRow)]
struct OriginProgress {
	key: String,
	number: Option<f64>,
	volume: Option<f64>,
	last_page_read: i64,
	is_read: bool,
	is_bookmarked: bool,
	last_read_at: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct TargetChapter {
	id: i64,
	number: Option<f64>,
	volume: Option<f64>,
}

/// 章节号精确匹配（重号取源序最小者）；源章节没有章节号才按卷号兜底，
/// 且只落在目标同样没有章节号的章节上，避免卷号误吞正常章节
fn match_target<'t>(targets: &'t [TargetChapter], row: &OriginProgress) -> Option<&'t TargetChapter> {
	if let Some(number) = row.number {
		return targets.iter().find(|t| t.number == Some(number));
	}
	if let Some(volume) = row.volume {
		return targets.iter().find(|t| t.number.is_none() && t.volume == Some(volume));
	}
	None
}

fn chapter_label(row: &OriginProgress) -> String {
	if let Some(number) = row.number {
		return format!("number {number}");
	}
	if let Some(volume) = row.volume {
		return format!("volume {volume}");
	}
	format!("key {}", row.key)
}
