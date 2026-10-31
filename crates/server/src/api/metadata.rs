//! Metadata matching REST surface (#...): preview a tracker or source entry's
//! metadata against a library entry, apply the fields the user confirmed, and
//! restore (unlock) source-owned metadata afterwards.
//!
//! The tracker side is a plain lookup; matching never writes on its own. The
//! apply request re-reads the remote entry and revalidates every offered value,
//! so a client cannot smuggle in metadata the preview never showed.

use crate::error::{ApiError, ApiResult};
use crate::metadata::{self, Field, FieldValue};
use crate::model::{MangaRow, manga_from_row, unix_now};
use crate::source::{manga_row, upsert_manga};
use crate::state::AppState;
use crate::tracker::core::TrackMode;
use crate::tracker::metadata::{TrackerMetadata, TitleVariants};
use crate::tracker::{Provider, TrackerError, dao};
use aidoku::{ContentRating, Manga, MangaStatus};
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Where the incoming metadata comes from: a tracker binding candidate or
/// another source's search result.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Citation {
	Tracker(Provider, String),
	/// A source reference: `source_id` addresses the installed instance (the wire
	/// reference and the fetch handle) while `source_key` is the natural key the
	/// lock records, because an instance id means nothing outside one database.
	Source { source_id: i64, source_key: String, remote_id: String },
}

impl Citation {
	/// The tag stored in `metadata_locks` (the provenance shown to the user).
	fn lock_provider(&self) -> String {
		match self {
			Self::Tracker(provider, _) => metadata::tracker_reference(provider.as_str()),
			Self::Source { source_key, .. } => metadata::source_reference(source_key),
		}
	}

	fn remote_id(&self) -> &str {
		match self {
			Self::Tracker(_, remote_id) | Self::Source { remote_id, .. } => remote_id,
		}
	}

	/// The wire reference the client sends back (`tracker:anilist`, `source:5`).
	fn wire_provider(&self) -> String {
		match self {
			Self::Tracker(provider, _) => format!("tracker:{}", provider.as_str()),
			Self::Source { source_id, .. } => format!("source:{source_id}"),
		}
	}
}

async fn parse_citation(state: &AppState, provider: &str, remote_id: &str) -> ApiResult<Citation> {
	let remote_id = remote_id.trim();
	if remote_id.is_empty() {
		return Err(ApiError::bad_request("remoteId must not be blank"));
	}
	if let Some(name) = provider.strip_prefix("tracker:") {
		let parsed = Provider::parse(name).ok_or_else(|| ApiError::not_found(format!("tracker provider '{name}' is not supported")))?;
		return Ok(Citation::Tracker(parsed, remote_id.to_string()));
	}
	if let Some(source_id) = provider.strip_prefix("source:") {
		let source_id: i64 = source_id
			.parse()
			.map_err(|_| ApiError::bad_request("source reference must be 'source:<id>'"))?;
		// Resolve the natural key now: the lock stores it, and a citation naming a
		// source that is not installed cannot be fetched anyway
		let source_key = crate::source::source_key(&state.pool, source_id)
			.await?
			.ok_or_else(|| ApiError::not_found(format!("source {source_id} is not installed")))?;
		return Ok(Citation::Source { source_id, source_key, remote_id: remote_id.to_string() });
	}
	Err(ApiError::bad_request("provider must be 'tracker:<provider>' or 'source:<id>'"))
}

async fn fetch_metadata(state: &Arc<AppState>, citation: &Citation, user_id: i64) -> ApiResult<TrackerMetadata> {
	match citation {
		Citation::Tracker(provider, remote_id) => {
			let services = state
				.tracker
				.get()
				.ok_or_else(|| ApiError::internal("tracker services are not initialized"))?;
			let service = services
				.get(*provider)
				.ok_or_else(|| ApiError::not_found(format!("tracker provider '{provider}' is not supported")))?;
			// MangaBaka detail is a public endpoint, so a signed-out user can still preview a match; the other
			// providers read their metadata with an OAuth token and answer NotLoggedIn from the call itself
			if !service.metadata_is_public()
				&& !service.is_logged_in(user_id).await.map_err(crate::api::tracker::tracker_error)?
			{
				return Err(crate::api::tracker::tracker_error(TrackerError::NotLoggedIn { provider: *provider }));
			}
			service
				.metadata(user_id, remote_id)
				.await
				.map_err(crate::api::tracker::tracker_error)
		}
		Citation::Source { source_id, remote_id, .. } => source_metadata(state, *source_id, remote_id).await,
	}
}

/// Another source's entry as metadata: fetch its details the same way a library
/// refresh would, without touching the library entry.
async fn source_metadata(state: &Arc<AppState>, source_id: i64, key: &str) -> ApiResult<TrackerMetadata> {
	let handle = state.get_handle(source_id).await?;
	let initial = Manga { key: key.to_string(), ..Default::default() };
	let manga = handle.get_manga_update(initial, true, false).await?;
	Ok(TrackerMetadata {
		id: key.to_string(),
		title: TitleVariants { default: manga.title.clone(), alternatives: Vec::new() },
		authors: manga.authors.unwrap_or_default(),
		artists: manga.artists.unwrap_or_default(),
		description: manga.description.filter(|text| !text.trim().is_empty()),
		tags: manga.tags.unwrap_or_default(),
		status: (manga.status != MangaStatus::Unknown).then_some(manga.status),
		content_rating: (manga.content_rating != ContentRating::Unknown).then_some(manga.content_rating),
		cover_url: manga.cover,
		url: manga.url,
		info: None,
	})
}

/// The value a field receives when the user does not override it; `None` means
/// the provider cannot supply that field.
fn incoming_for(metadata: &TrackerMetadata, field: Field) -> Option<Value> {
	match field {
		Field::Title => metadata.title_default().map(|title| json!(title)),
		Field::Authors => (!metadata.authors.is_empty()).then(|| json!(metadata.authors)),
		Field::Artists => (!metadata.artists.is_empty()).then(|| json!(metadata.artists)),
		Field::Description => metadata
			.description
			.as_deref()
			.map(str::trim)
			.filter(|text| !text.is_empty())
			.map(|text| json!(text)),
		Field::Tags => (!metadata.tags.is_empty()).then(|| json!(metadata.tags)),
		Field::Status => metadata.status.map(|status| json!(metadata::status_name(status))),
		Field::ContentRating => metadata.content_rating.map(|rating| json!(metadata::content_rating_name(rating))),
		Field::Cover => metadata
			.cover_url
			.as_deref()
			.filter(|url| url.starts_with("http://") || url.starts_with("https://"))
			.map(|url| json!(url)),
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPreviewQuery {
	pub provider: String,
	pub remote_id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataApplyRequest {
	pub provider: String,
	pub remote_id: String,
	pub fields: Vec<String>,
	#[serde(default)]
	pub values: Option<HashMap<String, Value>>,
	/// Also remember a tracker binding for this entry in the same transaction.
	#[serde(default)]
	pub bind: bool,
}

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

/// Matching and editing metadata is a library feature: only a durable entry has
/// a row worth locking, while a stray search result is refreshed by its source
/// anyway. Local sources are exempt: their entries are the user's own files and
/// are editable without a library membership row.
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

/// A preview never writes: it reports the provider's values next to the current
/// ones and marks fields the provider cannot fill as unselectable.
pub async fn preview_metadata(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(manga_id): Path<i64>,
	Query(query): Query<MetadataPreviewQuery>,
) -> ApiResult<Json<Value>> {
	let row = manga_row(&state.pool, manga_id).await?;
	require_in_library(&state.pool, manga_id, row.source_id, user.id).await?;
	let citation = parse_citation(&state, &query.provider, &query.remote_id).await?;
	let metadata = fetch_metadata(&state, &citation, user.id).await?;
	let mut conn = state.pool.acquire().await?;
	let locks = metadata::load(&mut conn, manga_id).await?;
	let linked = is_linked(&state, manga_id, user.id, &citation).await?;
	let fields: Vec<Value> = Field::ALL
		.into_iter()
		.map(|field| {
			let incoming = incoming_for(&metadata, field);
			let reason = incoming.is_none().then(|| "the provider has no value for this field".to_string());
			let alternatives: Vec<Value> = if field == Field::Title {
				metadata
					.title
					.alternatives
					.iter()
					.filter(|alternative| !alternative.value.trim().is_empty())
					.map(|alternative| json!({"label": alternative.label, "value": alternative.value}))
					.collect()
			} else {
				Vec::new()
			};
			json!({
				"field": field.as_str(),
				"label": field.label(),
				"current": metadata::current_value(&row, field),
				"incoming": incoming.unwrap_or(Value::Null),
				"selectable": reason.is_none(),
				"reason": reason,
				"alternatives": alternatives,
				"lock": locks.get(field.as_str()).map(|lock| serde_json::to_value(lock).unwrap_or(Value::Null)),
			})
		})
		.collect();
	Ok(Json(json!({
		"candidate": {
			"provider": citation.wire_provider(),
			"remoteId": citation.remote_id(),
			"title": metadata.title_default().unwrap_or_default(),
			"url": metadata.url,
			"coverUrl": metadata.cover_url,
			"info": metadata.info,
		},
		"linked": linked,
		"fields": fields,
	})))
}

async fn is_linked(state: &Arc<AppState>, manga_id: i64, user_id: i64, citation: &Citation) -> ApiResult<bool> {
	let Citation::Tracker(provider, remote_id) = citation else {
		return Ok(false);
	};
	let links = dao::list_links_by_manga_and_user(&state.pool, manga_id, user_id).await?;
	Ok(links.iter().any(|link| link.provider == provider.as_str() && link.track_id == *remote_id))
}

fn field_names(fields: &[Field]) -> Vec<&'static str> {
	fields.iter().map(|field| field.as_str()).collect()
}

/// Writes the confirmed fields, records ownership, and optionally remembers the
/// tracker binding — all in one transaction.
pub async fn apply_metadata(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(manga_id): Path<i64>,
	Json(body): Json<MetadataApplyRequest>,
) -> ApiResult<Json<Value>> {
	let row = manga_row(&state.pool, manga_id).await?;
	require_in_library(&state.pool, manga_id, row.source_id, user.id).await?;
	let citation = parse_citation(&state, &body.provider, &body.remote_id).await?;
	if body.bind && !matches!(citation, Citation::Tracker(..)) {
		return Err(ApiError::bad_request("bind is only available for tracker matches"));
	}
	let metadata = fetch_metadata(&state, &citation, user.id).await?;
	let mut planned: Vec<(Field, FieldValue)> = Vec::new();
	let mut seen: Vec<Field> = Vec::new();
	for name in &body.fields {
		let field = Field::parse(name).ok_or_else(|| ApiError::bad_request(format!("'{name}' is not a metadata field")))?;
		if seen.contains(&field) {
			continue;
		}
		let incoming = incoming_for(&metadata, field)
			.ok_or_else(|| ApiError::bad_request(format!("{} has no value in this match", field.as_str())))?;
		let chosen = body.values.as_ref().and_then(|values| values.get(field.as_str())).cloned().unwrap_or(incoming.clone());
		if field == Field::Title {
			let title = chosen
				.as_str()
				.ok_or_else(|| ApiError::bad_request("title must be a string"))?;
			if !metadata.title.values().iter().any(|value| value == title) {
				return Err(ApiError::bad_request("title must be one of the offered title variants"));
			}
		} else if chosen != incoming {
			return Err(ApiError::bad_request(format!("{} can only carry the matched value", field.as_str())));
		}
		let value = metadata::parse_value(field, &chosen).map_err(ApiError::bad_request)?;
		planned.push((field, value));
		seen.push(field);
	}
	if planned.is_empty() {
		return Err(ApiError::bad_request("select at least one field to apply"));
	}
	let now = unix_now();
	let lock_provider = citation.lock_provider();
	// The lock map is read before it is written, so the transaction starts as a write:
	// a deferred BEGIN would hit SQLITE_BUSY_SNAPSHOT when it upgrades against a
	// concurrent commit
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let mut locks = metadata::load(&mut tx, manga_id).await?;
	for (field, _) in &planned {
		metadata::lock_field(&mut locks, *field, &lock_provider, Some(citation.remote_id()), now);
	}
	crate::store::manga::set_metadata(&mut tx, manga_id, &planned, &locks).await?;
	if body.bind && let Citation::Tracker(provider, track_id) = &citation {
		dao::upsert_link_tx(
			&mut tx,
			&dao::NewTrackerLink {
				manga_id,
				user_id: user.id,
				provider: provider.as_str(),
				track_id,
				title: metadata.title_default(),
				track_mode: TrackMode::Auto,
				chapter_offset: 0,
			},
		)
		.await?;
	}
	tx.commit().await?;
	if planned.iter().any(|(field, _)| *field == Field::Cover) {
		crate::source::cover::invalidate_cache(&state.config.covers_dir, manga_id).await;
	}
	let row = manga_row(&state.pool, manga_id).await?;
	Ok(Json(json!({
		"manga": crate::api::manga_json(&state, user.id, &row).await?,
		"appliedFields": field_names(&planned.iter().map(|(field, _)| *field).collect::<Vec<_>>()),
	})))
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

/// Lock-map view for a single entry, used by the edit dialog to label fields.
pub async fn metadata_locks(state: &Arc<AppState>, manga_id: i64) -> ApiResult<metadata::Locks> {
	let mut conn = state.pool.acquire().await?;
	Ok(metadata::load(&mut conn, manga_id).await?)
}
