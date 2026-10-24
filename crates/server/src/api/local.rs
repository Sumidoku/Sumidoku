//! Local-source scan and trash endpoints (kmrs's library scan/trash API shape): manual scan enqueues
//! per-instance tasks (dedup makes a repeat trigger idempotent), the trash lists soft-deleted local manga,
//! and restore/purge act on single rows — restore synchronously, empty-trash as a task.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::task::PRIORITY_MANUAL;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
	/// One instance; absent = every enabled local instance
	pub source_id: Option<i64>,
}

pub async fn scan_now(
	State(state): State<Arc<AppState>>,
	body: Option<Json<ScanRequest>>,
) -> ApiResult<Json<Value>> {
	let source_id = body.and_then(|Json(body)| body.source_id);
	if let Some(id) = source_id {
		let row = super::source_row(&state, id).await?;
		if !row.is_local() {
			return Err(ApiError::bad_request("not a local source"));
		}
	}
	let tasks = crate::local::scan::enqueue_scan(&state, source_id, PRIORITY_MANUAL).await?;
	let tasks: Vec<Value> = tasks.iter().map(|task| task.to_json()).collect();
	Ok(Json(json!(tasks)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashQuery {
	pub source_id: Option<i64>,
}

pub async fn list_trash(
	State(state): State<Arc<AppState>>,
	Query(query): Query<TrashQuery>,
) -> ApiResult<Json<Value>> {
	Ok(Json(json!(crate::local::scan::list_trash(&state, query.source_id).await?)))
}

pub async fn restore_trash(
	State(state): State<Arc<AppState>>,
	Path(manga_id): Path<i64>,
) -> ApiResult<Json<Value>> {
	crate::local::scan::restore_manga(&state, manga_id).await?;
	Ok(Json(json!({ "ok": true })))
}

pub async fn purge_trash(
	State(state): State<Arc<AppState>>,
	Path(manga_id): Path<i64>,
) -> ApiResult<StatusCode> {
	crate::local::scan::purge_manga(&state, manga_id).await?;
	Ok(StatusCode::NO_CONTENT)
}

/// Empty the trash bin of one instance (or every local instance): enqueues an empty_trash task per instance
pub async fn empty_trash(
	State(state): State<Arc<AppState>>,
	Query(query): Query<TrashQuery>,
) -> ApiResult<Json<Value>> {
	let ids: Vec<i64> = match query.source_id {
		Some(id) => {
			let row = super::source_row(&state, id).await?;
			if !row.is_local() {
				return Err(ApiError::bad_request("not a local source"));
			}
			vec![id]
		}
		None => {
			sqlx::query_scalar("SELECT id FROM source WHERE kind = 'local' AND enabled = 1 ORDER BY id")
				.fetch_all(&state.pool)
				.await?
		}
	};
	let mut tasks = Vec::with_capacity(ids.len());
	for id in ids {
		tasks.push(
			state
				.tasks()
				.enqueue(
					&format!("{}:{id}", crate::local::scan::KIND_EMPTY_TRASH),
					crate::local::scan::KIND_EMPTY_TRASH,
					Some(id),
					json!({}),
					PRIORITY_MANUAL,
				)
				.await?,
		);
	}
	let tasks: Vec<Value> = tasks.iter().map(|task| task.to_json()).collect();
	Ok(Json(json!(tasks)))
}
