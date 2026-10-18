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

/// setting 里的有序 pin 列表（source key 数组），顺序即后续跨源迁移的默认优先级，
/// 对齐 Aidoku Browse.pinnedList；源重装卸载会换内部 id，key 才是稳定身份
const PINNED_KEY: &str = "browse.pinned_sources";

/// key → pinOrder，source_json 用；形状不合按空列表（setting 通用 PUT 允许任意 JSON）
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

/// 卸载源时剔除 stale key，余者相对顺序不变
pub(crate) async fn prune_pinned(pool: &SqlitePool, source_key: &str) -> ApiResult<()> {
	let keys = pinned_keys(pool).await?;
	let kept: Vec<String> = keys.iter().filter(|key| *key != source_key).cloned().collect();
	if kept.len() != keys.len() {
		write_pinned(pool, &kept).await?;
	}
	Ok(())
}

/// pin 追加到列表尾、unpin 移除；pinned 缺省为无操作（非法值按端点约定 400 而非 422）
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

/// 整体替换顺序：ids 必须与当前 pinned 是同一集合（长度相等且元素相同），
/// 缺一个或多一个都按 400 挡下——reorder 不许顺带丢 pin；
/// 手动解析 body 与 manga PATCH 同约定：非法形状 400 而非 axum 的 422
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
