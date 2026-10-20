use crate::api::repo::source_json;
use crate::error::{ApiError, ApiResult};
use crate::source::repo;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Ordered pin list in the settings (an array of source keys); the order is also the default priority for later
/// cross-source migration, aligned with Aidoku's Browse.pinnedList; reinstalling/uninstalling a source changes its internal id, so the key is the stable identity
const PINNED_KEY: &str = "browse.pinned_sources";

/// key → pinOrder, for source_json; a wrong-shaped value reads as an empty list (the generic setting PUT accepts any JSON)
pub(crate) async fn pinned_map(pool: &SqlitePool) -> ApiResult<HashMap<String, i64>> {
	let keys = pinned_keys(pool).await?;
	Ok(keys
		.into_iter()
		.enumerate()
		.map(|(i, key)| (key, i as i64))
		.collect())
}

async fn pinned_keys(pool: &SqlitePool) -> ApiResult<Vec<String>> {
	let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = ?")
		.bind(PINNED_KEY)
		.fetch_optional(pool)
		.await?;
	Ok(row
		.and_then(|(raw,)| serde_json::from_str::<Vec<String>>(&raw).ok())
		.unwrap_or_default())
}

async fn write_pinned(pool: &SqlitePool, keys: &[String]) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(PINNED_KEY)
	.bind(serde_json::to_string(keys)?)
	.execute(pool)
	.await?;
	Ok(())
}

/// Drops stale keys when a source is uninstalled; the survivors' relative order is unchanged
pub(crate) async fn prune_pinned(pool: &SqlitePool, source_key: &str) -> ApiResult<()> {
	let keys = pinned_keys(pool).await?;
	let kept: Vec<String> = keys.iter().filter(|key| *key != source_key).cloned().collect();
	if kept.len() != keys.len() {
		write_pinned(pool, &kept).await?;
	}
	Ok(())
}

/// pin appends to the list end, unpin removes; an absent pinned is a no-op (invalid values get the endpoint-convention 400, not 422)
pub async fn patch_source(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let row = super::source_row(&state, id).await?;
	let pinned = match body {
		None => None,
		Some(Json(value)) => value
			.get("pinned")
			.map(|v| v.as_bool().ok_or_else(|| ApiError::bad_request("pinned must be a boolean")))
			.transpose()?,
	};
	if let Some(pinned) = pinned {
		let mut keys = pinned_keys(&state.pool).await?;
		let at = keys.iter().position(|key| key == &row.key);
		match (pinned, at) {
			(true, None) => {
				keys.push(row.key.clone());
				write_pinned(&state.pool, &keys).await?;
			}
			(false, Some(i)) => {
				keys.remove(i);
				write_pinned(&state.pool, &keys).await?;
			}
			_ => {}
		}
	}
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = pinned_map(&state.pool).await?;
	Ok(Json(source_json(&row, &latest, &pinned)))
}

#[derive(Debug, Deserialize)]
pub struct ReorderPinnedRequest {
	pub ids: Vec<String>,
}

/// Replaces the whole order: ids must be exactly the current pinned set (same length and same elements);
/// one missing or one extra is rejected with 400 — a reorder must not drop pins along the way;
/// the body is parsed by hand, same convention as the manga PATCH: invalid shape gets 400, not axum's 422
pub async fn reorder_pinned(
	State(state): State<Arc<AppState>>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let body = body.ok_or_else(|| ApiError::bad_request("missing body"))?;
	let request: ReorderPinnedRequest = serde_json::from_value(body.0)
		.map_err(|e| ApiError::bad_request(format!("invalid reorder request: {e}")))?;
	let current = pinned_keys(&state.pool).await?;
	let current_set: HashSet<&str> = current.iter().map(String::as_str).collect();
	let new_set: HashSet<&str> = request.ids.iter().map(String::as_str).collect();
	if request.ids.len() != current.len() || new_set != current_set {
		return Err(ApiError::bad_request("ids must be exactly the pinned sources"));
	}
	write_pinned(&state.pool, &request.ids).await?;
	Ok(Json(json!(request.ids)))
}
