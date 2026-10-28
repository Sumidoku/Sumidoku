use crate::api::{PageQuery, chapter_json, manga_json_by_ids, progress_of};
use crate::error::{ApiError, ApiResult};
use crate::model::ChapterUserRow;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::Extension;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
	pub category_id: Option<i64>,
}

pub async fn fetch_updates(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	body: Option<Json<FetchRequest>>,
) -> Response {
	let scheduler = state.updater();
	let category_id = body.and_then(|Json(body)| body.category_id);
	if let Some(id) = category_id {
		let exists: Option<(i64,)> =
			match sqlx::query_as("SELECT 1 FROM category WHERE id = ? AND user_id = ?")
				.bind(id)
				.bind(user.id)
				.fetch_optional(&state.pool)
				.await
			{
				Ok(row) => row,
				Err(e) => return ApiError::from(e).into_response(),
			};
		if exists.is_none() {
			return ApiError::not_found("category not found").into_response();
		}
	}
	let Some(guard) = scheduler.try_acquire() else {
		return (
			StatusCode::CONFLICT,
			Json(json!({"error": {"code": "busy", "message": "update already running"}})),
		)
			.into_response();
	};
	let user_id = user.id;
	tokio::spawn(async move {
		scheduler.fetch(user_id, category_id, guard).await;
	});
	Json(json!({ "started": true })).into_response()
}

pub async fn get_summary(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	Ok(Json(serde_json::to_value(
		state.updater().summary().await.visible_to(user.id),
	)?))
}

pub async fn get_recent(
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
		 FROM chapter c
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = ? AND mu.in_library = 1
			AND (mu.in_library_at IS NULL OR COALESCE(c.created_at, c.fetched_at) >= mu.in_library_at)
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 ORDER BY c.fetched_at DESC, c.id DESC LIMIT ? OFFSET ?",
	)
	.bind(user.id)
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
		entries.push(json!({
			"chapter": chapter_json(&row.chapter, progress_of(row).as_ref())?,
			"manga": manga_value,
			"fetchedAt": row.chapter.fetched_at,
		}));
	}
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": has_next_page,
	})))
}
