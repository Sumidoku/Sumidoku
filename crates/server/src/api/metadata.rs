//! Metadata locks REST surface: restore (unlock) the fields an entry's own
//! source should own again, so the next refresh fills them.
//!
//! A metadata write reads the lock map before changing it, so the endpoint
//! opens one write transaction and lets the store journal the new version
//! instead of reading deferred and upgrading later.

use crate::error::{ApiError, ApiResult};
use crate::metadata::{self, Field};
use crate::model::{MangaRow, manga_from_row};
use crate::source::{manga_row, upsert_manga};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataRestoreRequest {
	/// Fields to unlock; empty means every locked field.
	#[serde(default)]
	pub fields: Option<Vec<String>>,
	/// Re-read the owning source's details so unlocked fields take source values now.
	#[serde(default)]
	pub refresh: bool,
}

/// Editing metadata is a library feature: only a durable entry has a row worth
/// locking, while a stray search result is refreshed by its source anyway. Local
/// sources are exempt: their entries are the user's own files and are editable
/// without a library membership row.
pub(crate) async fn require_in_library(
	pool: &sqlx::SqlitePool,
	manga_id: i64,
	source_id: i64,
	user_id: i64,
) -> ApiResult<()> {
	if crate::source::source_is_local(pool, source_id).await? {
		return Ok(());
	}
	let in_library: Option<(bool,)> = sqlx::query_as("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = ?")
		.bind(manga_id)
		.bind(user_id)
		.fetch_optional(pool)
		.await?;
	if !in_library.map(|(flag,)| flag).unwrap_or(false) {
		return Err(ApiError::not_in_library("manga not in library"));
	}
	Ok(())
}

fn field_names(fields: &[Field]) -> Vec<&'static str> {
	fields.iter().map(|field| field.as_str()).collect()
}

/// Drops field locks; with `refresh` the owning source is re-read immediately so
/// the unlocked fields take its values instead of staying stale.
pub async fn restore_metadata(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(manga_id): Path<i64>,
	body: Option<Json<MetadataRestoreRequest>>,
) -> ApiResult<Json<Value>> {
	let body = body.map(|Json(body)| body).unwrap_or_default();
	let row = manga_row(&state.pool, manga_id).await?;
	require_in_library(&state.pool, manga_id, row.source_id, user.id).await?;
	let fields: Vec<Field> = match body.fields {
		Some(names) if !names.is_empty() => names
			.iter()
			.map(|name| Field::parse(name).ok_or_else(|| ApiError::bad_request(format!("'{name}' is not a metadata field"))))
			.collect::<ApiResult<_>>()?,
		_ => Field::ALL.to_vec(),
	};
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let mut locks = metadata::load(&mut tx, manga_id).await?;
	let mut unlocked: Vec<Field> = Vec::new();
	for field in &fields {
		if metadata::unlock_field(&mut locks, *field) {
			unlocked.push(*field);
		}
	}
	crate::store::manga::set_locks(&mut tx, manga_id, &locks).await?;
	tx.commit().await?;
	let row = if body.refresh {
		refresh_from_source(&state, &row).await?
	} else {
		row
	};
	if fields.contains(&Field::Cover) {
		crate::source::cover::invalidate_cache(&state.config.covers_dir, manga_id).await;
	}
	Ok(Json(json!({
		"manga": crate::api::manga_json(&state, user.id, &row).await?,
		"unlockedFields": field_names(&unlocked),
	})))
}

/// Re-reads one entry's details from its own source, through the same
/// lock-aware upsert a refresh uses, and returns the fresh row.
async fn refresh_from_source(state: &Arc<AppState>, row: &MangaRow) -> ApiResult<MangaRow> {
	let handle = state.get_handle(row.source_id).await?;
	let updated = handle.get_manga_update(manga_from_row(row), true, false).await?;
	// The upsert writes the row and journals the version that describes it; both
	// belong to one write transaction, as in every other refresh path
	let mut write = crate::db::begin_write(&state.pool).await?;
	upsert_manga(&mut write, row.source_id, &updated, true, true, false).await?;
	write.commit().await?;
	manga_row(&state.pool, row.id).await
}

/// Lock-map view for a single entry, for callers that only need the map.
pub async fn metadata_locks(state: &Arc<AppState>, manga_id: i64) -> ApiResult<metadata::Locks> {
	let mut conn = state.pool.acquire().await?;
	Ok(metadata::load(&mut conn, manga_id).await?)
}
