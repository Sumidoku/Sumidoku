use crate::error::ApiResult;
use crate::model::SourceRow;
use crate::source::repo::{self, BrowseFilter, SourceRepoRow};
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

fn repo_json(row: &SourceRepoRow) -> Value {
	json!({
		"id": row.id,
		"url": row.url,
		"name": row.name,
		"sourceCount": repo::cached_entries(row).len(),
		"addedAt": row.added_at,
		"fetchedAt": row.fetched_at,
		"fetchError": row.fetch_error,
	})
}

/// Single outlet for the installed-source DTO: list/install/update/patch share one shape; updateVersion takes the
/// newest version across the repo caches (a value only when strictly greater than installed), null when there is no
/// repo or no update; pinOrder is the index inside browse.pinned_sources, null when not pinned
pub(crate) fn source_json(
	row: &SourceRow,
	latest: &HashMap<String, (i64, i64)>,
	pinned: &HashMap<String, i64>,
) -> Value {
	let update = latest
		.get(&row.key)
		.and_then(|&(version, _)| (version > row.version).then_some(version));
	// Aidoku's deprecation semantics: installed externally (repo_id non-null) and the key is no longer in any repo index
	let missing_from_repos = row.repo_id.is_some() && !latest.contains_key(&row.key);
	// Home capability bit: stamped at registration; old unstamped rows are null, and consumers treat that as unknown (the entry point is not hidden)
	let has_home = row
		.features
		.as_deref()
		.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
		.and_then(|v| v.get("home").and_then(|b| b.as_bool()));
	let config = row
		.config
		.as_deref()
		.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
		.unwrap_or(Value::Null);
	json!({
		"id": row.id,
		"key": row.key,
		"name": row.name,
		"kind": row.kind,
		"version": row.version,
		"languages": row.languages.0,
		"contentRating": row.content_rating,
		"baseUrl": row.base_url,
		"breakingChangeVersion": row.breaking_change_version,
		"config": config,
		"enabled": row.enabled,
		"installedAt": row.installed_at,
		"updatedAt": row.updated_at,
		"updateVersion": update,
		"missingFromRepos": missing_from_repos,
		"hasHome": has_home,
		"pinned": pinned.contains_key(&row.key),
		"pinOrder": pinned.get(&row.key),
	})
}

pub async fn list_repos(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let repos = repo::list_repos(&state.pool).await?;
	let out: Vec<Value> = repos.iter().map(repo_json).collect();
	Ok(Json(json!(out)))
}

#[derive(Debug, Deserialize)]
pub struct AddRepoRequest {
	pub url: String,
}

pub async fn add_repo(
	State(state): State<Arc<AppState>>,
	Json(body): Json<AddRepoRequest>,
) -> ApiResult<Json<Value>> {
	let url = repo::parse_repo_url(&body.url)?;
	let row = repo::add_repo(&state, url).await?;
	Ok(Json(repo_json(&row)))
}

pub async fn delete_repo(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	repo::delete_repo(&state, id).await?;
	Ok(StatusCode::NO_CONTENT)
}

pub async fn refresh_repo(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let row = repo::refresh_repo(&state, id).await?;
	Ok(Json(repo_json(&row)))
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
	pub lang: Option<String>,
	pub query: Option<String>,
	/// Comma-separated content-rating whitelist (Aidoku SourceContentRating numbers); absent means no filtering
	#[serde(rename = "contentRating")]
	pub content_rating: Option<String>,
}

pub async fn browse_sources(
	State(state): State<Arc<AppState>>,
	Query(query): Query<BrowseQuery>,
) -> ApiResult<Json<Value>> {
	let filter = BrowseFilter {
		lang: query
			.lang
			.unwrap_or_default()
			.split(',')
			.map(|l| l.trim().to_lowercase())
			.filter(|l| !l.is_empty())
			.collect(),
		query: query.query.unwrap_or_default().trim().to_string(),
		// Some(empty string) means no rating selected at all: parses to an empty set, matching no source
		content_ratings: query
			.content_rating
			.map(|s| s.split(',').filter_map(|v| v.trim().parse::<i64>().ok()).collect()),
	};
	let items = repo::browse_sources(&state.pool, &filter).await?;
	let out: Vec<Value> = items
		.iter()
		.map(|item| {
			let update = item.installed_version.and_then(|installed| {
				(item.entry.version > installed).then_some(item.entry.version)
			});
			json!({
				"key": item.entry.key,
				"name": item.entry.name,
				"version": item.entry.version,
				"languages": item.entry.languages,
				"contentRating": item.entry.content_rating,
				"iconUrl": item.entry.icon_url,
				"baseUrl": item.entry.base_url,
				"repoId": item.repo_id,
				"repoName": item.repo_name,
				"installedId": item.installed_id,
				"installedVersion": item.installed_version,
				"updateVersion": update,
			})
		})
		.collect();
	Ok(Json(json!(out)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
	pub repo_id: i64,
	pub source_key: String,
}

pub async fn install_source(
	State(state): State<Arc<AppState>>,
	Json(body): Json<InstallRequest>,
) -> ApiResult<Json<Value>> {
	let row = repo::install_from_repo(&state, body.repo_id, &body.source_key).await?;
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = super::pins::pinned_map(&state.pool).await?;
	Ok(Json(source_json(&row, &latest, &pinned)))
}

pub async fn update_source(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let update = repo::update_source(&state, id, user.id).await?;
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = super::pins::pinned_map(&state.pool).await?;
	let mut value = source_json(&update.source, &latest, &pinned);
	if let Some(object) = value.as_object_mut() {
		object.insert("migrationRequired".into(), json!(update.migration.required));
		object.insert("migrationWarnings".into(), json!(update.migration.warnings));
	}
	Ok(Json(value))
}

pub async fn update_all(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let outcome = repo::update_all(&state, user.id).await?;
	let failed: Vec<Value> = outcome
		.failed
		.iter()
		.map(|(key, reason)| json!({ "key": key, "reason": reason }))
		.collect();
	let migration_warnings: Vec<Value> = outcome
		.migration_warnings
		.iter()
		.map(|(key, message)| json!({ "key": key, "message": message }))
		.collect();
	Ok(Json(json!({
		"updated": outcome.updated,
		"failed": failed,
		"migrationRequired": outcome.migration_required,
		"migrationWarnings": migration_warnings,
	})))
}
