use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::sync::{WireChange, changes_since_for_peer, instance_id};
use axum::extract::{Query, State};
use axum::response::Json;
use axum::Extension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullQuery {
	origin: Option<String>,
	since: Option<i64>,
	limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResponse {
	origin: String,
	user_id: i64,
	next_cursor: i64,
	has_more: bool,
	changes: Vec<WireChange>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushBody {
	origin: String,
	changes: Vec<WireChange>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
	results: Vec<crate::sync::apply::ApplyResult>,
}

pub async fn get_changes(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<PullQuery>,
) -> ApiResult<Json<PullResponse>> {
	let peer = query
		.origin
		.filter(|origin| !origin.is_empty())
		.ok_or_else(|| ApiError::bad_request("origin is required"))?;
	let since = query.since.unwrap_or(0);
	let requested_limit = query.limit.unwrap_or(100);
	if requested_limit < 1 {
		return Err(ApiError::bad_request("limit must be greater than zero"));
	}
	let page = changes_since_for_peer(&state.pool, user.id, &peer, since, requested_limit.min(500)).await?;
	let origin = instance_id(&state.pool).await?;
	let changes = page.changes.into_iter().map(WireChange::from).collect();
	Ok(Json(PullResponse {
		origin,
		user_id: user.id,
		next_cursor: page.next_cursor,
		has_more: page.has_more,
		changes,
	}))
}

pub async fn post_changes(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<PushBody>,
) -> ApiResult<Json<PushResponse>> {
	let results = crate::sync::apply::push(&state.pool, &body.origin, user.id, body.changes).await?;
	Ok(Json(PushResponse { results }))
}

pub async fn compact(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let result = crate::sync::compact::compact(&state.pool).await?;
	Ok(Json(serde_json::json!({
		"baseCursor": result.base_cursor,
		"removed": result.removed,
	})))
}
