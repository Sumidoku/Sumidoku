//! UI-managed API keys: generated server-side, returned once at creation, stored as
//! sha256 fingerprints in the api_key table, and revoked by deleting the row. They act
//! with the built-in admin's identity.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, sqlx::FromRow)]
struct KeyRow {
	key_hash: String,
	name: String,
	created_at: i64,
	last_used_at: Option<i64>,
}

fn key_json(row: &KeyRow) -> Value {
	json!({
		"id": row.key_hash,
		"name": row.name,
		"createdAt": row.created_at,
		"lastUsedAt": row.last_used_at,
		"source": "ui",
	})
}

pub async fn list_keys(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<KeyRow> = sqlx::query_as("SELECT key_hash, name, created_at, last_used_at FROM api_key ORDER BY created_at ASC")
		.fetch_all(&state.pool)
		.await?;
	let out: Vec<Value> = rows.iter().map(key_json).collect();
	Ok(Json(json!(out)))
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
	pub name: String,
}

pub async fn create_key(
	State(state): State<Arc<AppState>>,
	Json(body): Json<CreateKeyRequest>,
) -> ApiResult<Json<Value>> {
	let name = body.name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	let mut raw = [0u8; 32];
	OsRng.fill_bytes(&mut raw);
	let key: String = format!("smk_{}", raw.iter().map(|b| format!("{b:02x}")).collect::<String>());
	let hash = crate::auth::sha256_hex(&key);
	let now = crate::model::unix_now();
	sqlx::query("INSERT INTO api_key (key_hash, name, created_at) VALUES (?, ?, ?)")
		.bind(&hash)
		.bind(name)
		.bind(now)
		.execute(&state.pool)
		.await?;
	// The plaintext key appears only here; afterwards only the fingerprint exists
	Ok(Json(json!({
		"id": hash,
		"key": key,
		"name": name,
		"createdAt": now,
		"lastUsedAt": Value::Null,
		"source": "ui",
	})))
}

pub async fn revoke_key(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
	let deleted = sqlx::query("DELETE FROM api_key WHERE key_hash = ?")
		.bind(&id)
		.execute(&state.pool)
		.await?;
	if deleted.rows_affected() == 0 {
		return Err(ApiError::not_found("api key not found"));
	}
	Ok(StatusCode::NO_CONTENT)
}
