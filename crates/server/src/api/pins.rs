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
	Ok(crate::settings::PINNED_SOURCES.get(pool).await?.unwrap_or_default())
}

async fn write_pinned(pool: &SqlitePool, keys: &[String]) -> ApiResult<()> {
	crate::settings::PINNED_SOURCES.set(pool, &keys.to_vec()).await
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

/// pin appends to the list end, unpin removes; an absent pinned is a no-op.
/// enabled toggles whether the source can be instantiated; disabling evicts any running instance
/// so the next capability call observes the new state. Invalid values get the endpoint-convention 400, not 422.
pub async fn patch_source(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let mut row = super::source_row(&state, id).await?;
	let value = body.map(|Json(v)| v);
	let pinned = value
		.as_ref()
		.and_then(|v| v.get("pinned"))
		.map(|v| v.as_bool().ok_or_else(|| ApiError::bad_request("pinned must be a boolean")))
		.transpose()?;
	let enabled = value
		.as_ref()
		.and_then(|v| v.get("enabled"))
		.map(|v| v.as_bool().ok_or_else(|| ApiError::bad_request("enabled must be a boolean")))
		.transpose()?;
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
	if let Some(enabled) = enabled && enabled != row.enabled {
		sqlx::query("UPDATE source SET enabled = ? WHERE id = ?")
			.bind(enabled)
			.bind(id)
			.execute(&state.pool)
			.await?;
		row.enabled = enabled;
		if !enabled {
			// Evict the running instance so the disabled state is enforced immediately;
			// a later enable respawns from the updated row on next access.
			state.evict_source(id).await;
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
