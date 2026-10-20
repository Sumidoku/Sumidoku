use crate::api::{PageQuery, chapter_json, manga_json_by_ids, progress_of};
use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterProgress, ChapterUserRow, unix_now};
use crate::source::chapter_row;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPatch {
	pub last_page_read: Option<i64>,
	pub is_read: Option<bool>,
	pub is_bookmarked: Option<bool>,
}

impl ProgressPatch {
	fn is_empty(&self) -> bool {
		self.last_page_read.is_none() && self.is_read.is_none() && self.is_bookmarked.is_none()
	}

	/// lastPageRead 落笔即视为读过：只有它出现时才刷新 last_read_at；
	/// 隐身（全局或单本任一开启）时不打新戳，旧戳保持不动
	async fn apply(
		&self,
		tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
		user_id: i64,
		chapter_id: i64,
	) -> ApiResult<ChapterProgress> {
		let current = read_progress(&mut **tx, user_id, chapter_id).await?;
		let stamp = self.last_page_read.is_some() && !incognito_on(tx, user_id, chapter_id).await?;
		let merged = ChapterProgress {
			last_page_read: self.last_page_read.unwrap_or(current.last_page_read),
			is_read: self.is_read.unwrap_or(current.is_read),
			is_bookmarked: self.is_bookmarked.unwrap_or(current.is_bookmarked),
			last_read_at: if stamp { Some(unix_now()) } else { current.last_read_at },
		};
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked,
			 last_read_at)
			 VALUES (?, ?, ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
			 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at",
		)
		.bind(chapter_id)
		.bind(user_id)
		.bind(merged.last_page_read)
		.bind(merged.is_read)
		.bind(merged.is_bookmarked)
		.bind(merged.last_read_at)
		.execute(&mut **tx)
		.await?;
		Ok(merged)
	}
}

/// 隐身判定：该用户命名空间键 user.<id>.incognito.enabled 或该章节所属漫画的
/// manga_user.incognito，任一开启即暂停打戳（对齐 Aidoku 的 pause history：不记新历史，旧历史不动）
async fn incognito_on(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, user_id: i64, chapter_id: i64) -> ApiResult<bool> {
	let global: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = ?")
		.bind(crate::api::user_setting_key(user_id, "incognito.enabled"))
		.fetch_optional(&mut **tx)
		.await?;
	if global
		.and_then(|(raw,)| serde_json::from_str::<bool>(&raw).ok())
		.unwrap_or(false)
	{
		return Ok(true);
	}
	let per_manga: Option<(bool,)> = sqlx::query_as(
		"SELECT mu.incognito FROM chapter c
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = ?
		 WHERE c.id = ?",
	)
	.bind(user_id)
	.bind(chapter_id)
	.fetch_optional(&mut **tx)
	.await?;
	Ok(per_manga.map(|(on,)| on).unwrap_or(false))
}

async fn read_progress<'e, E>(executor: E, user_id: i64, chapter_id: i64) -> ApiResult<ChapterProgress>
where
	E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
	let row: Option<(i64, bool, bool, Option<i64>)> = sqlx::query_as(
		"SELECT last_page_read, is_read, is_bookmarked, last_read_at FROM chapter_user
		 WHERE chapter_id = ? AND user_id = ?",
	)
	.bind(chapter_id)
	.bind(user_id)
	.fetch_optional(executor)
	.await?;
	Ok(match row {
		Some((last_page_read, is_read, is_bookmarked, last_read_at)) => ChapterProgress {
			last_page_read,
			is_read,
			is_bookmarked,
			last_read_at,
		},
		None => ChapterProgress::default(),
	})
}

fn progress_json(chapter_id: i64, progress: &ChapterProgress) -> Value {
	json!({
		"chapterId": chapter_id,
		"lastPageRead": progress.last_page_read,
		"isRead": progress.is_read,
		"isBookmarked": progress.is_bookmarked,
		"lastReadAt": progress.last_read_at,
	})
}

pub async fn update_chapter(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Json(patch): Json<ProgressPatch>,
) -> ApiResult<Json<Value>> {
	chapter_row(&state.pool, id).await?;
	let progress = if patch.is_empty() {
		read_progress(&state.pool, user.id, id).await?
	} else {
		let mut tx = crate::db::begin_write(&state.pool).await?;
		let merged = patch.apply(&mut tx, user.id, id).await?;
		tx.commit().await?;
		merged
	};
	Ok(Json(progress_json(id, &progress)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchUpdateRequest {
	pub chapter_ids: Vec<i64>,
	#[serde(flatten)]
	pub patch: ProgressPatch,
}

pub async fn batch_update_chapters(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<BatchUpdateRequest>,
) -> ApiResult<Json<Value>> {
	if body.chapter_ids.is_empty() || body.patch.is_empty() {
		return Ok(Json(json!({ "updated": 0 })));
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let mut updated = 0;
	for chapter_id in &body.chapter_ids {
		let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM chapter WHERE id = ?")
			.bind(chapter_id)
			.fetch_optional(&mut *tx)
			.await?;
		if exists.is_none() {
			continue;
		}
		body.patch.apply(&mut tx, user.id, *chapter_id).await?;
		updated += 1;
	}
	tx.commit().await?;
	Ok(Json(json!({ "updated": updated })))
}

pub async fn get_history(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let rows: Vec<ChapterUserRow> = sqlx::query_as(
		"SELECT c.*, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		 WHERE cu.user_id = ? AND cu.last_read_at IS NOT NULL
		 ORDER BY cu.last_read_at DESC, cu.chapter_id DESC
		 LIMIT ? OFFSET ?",
	)
	.bind(user.id)
	.bind(PAGE_SIZE + 1)
	.bind((page as i64 - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next_page = rows.len() as i64 > PAGE_SIZE;
	let page_rows: Vec<&ChapterUserRow> = rows.iter().take(PAGE_SIZE as usize).collect();
	let manga_ids: Vec<i64> = page_rows.iter().map(|row| row.chapter.manga_id).collect();
	let manga_values = manga_json_by_ids(&state, user.id, &manga_ids).await?;
	let mut entries = Vec::with_capacity(page_rows.len());
	for (row, manga_value) in page_rows.into_iter().zip(manga_values) {
		let progress = progress_of(row).unwrap_or_default();
		entries.push(json!({
			"chapter": chapter_json(&row.chapter, Some(&progress))?,
			"manga": manga_value,
			"lastReadAt": row.last_read_at,
			"lastPageRead": progress.last_page_read,
			"isRead": progress.is_read,
		}));
	}
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": has_next_page,
	})))
}
