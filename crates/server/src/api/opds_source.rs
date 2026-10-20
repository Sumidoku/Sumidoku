//! OPDS 实例注册的 admin 端点：name + 目录 URL + 可选 basic 凭据，多实例并存

use super::repo::source_json;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterOpdsRequest {
	pub name: String,
	pub url: String,
	pub username: Option<String>,
	pub password: Option<String>,
}

pub async fn register_opds_source(
	State(state): State<Arc<AppState>>,
	Json(body): Json<RegisterOpdsRequest>,
) -> ApiResult<Json<Value>> {
	let row = crate::opds_source::register_instance(
		&state,
		&body.name,
		&body.url,
		body.username.as_deref(),
		body.password.as_deref(),
	)
	.await?;
	// 新实例不进任何仓库缓存：latest/pinned 空表，DTO 与列表端点同形
	Ok(Json(source_json(&row, &Default::default(), &Default::default())))
}
