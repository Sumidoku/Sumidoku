//! The admin endpoint for OPDS instance registration: name + catalog URL + optional basic credentials; multiple instances coexist

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
	// A new instance is in no repo cache: latest/pinned are empty maps; the DTO matches the list endpoint's shape
	Ok(Json(source_json(&row, &Default::default(), &Default::default())))
}
