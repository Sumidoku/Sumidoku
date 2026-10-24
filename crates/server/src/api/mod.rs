pub mod aidoku_backup;
pub mod api_keys;
pub mod backup;
pub mod docs;
pub mod download;
pub mod events;
pub mod filter_groups;
pub mod filters;
pub mod import_merge;
pub mod internal_source;
pub mod key_realign;
pub mod library;
pub mod listings;
pub mod local;
pub mod login;
pub mod migrate;
pub mod oauth;
pub mod page;
pub mod partial;
pub mod pins;
pub mod reader;
pub mod repo;
pub mod search;
pub mod settings;
pub mod server_settings;
pub mod source_import;
pub mod suwayomi_backup;
pub mod sync;
pub mod task;
pub mod update;

use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterProgress, ChapterRow, ChapterUserRow, MangaRow, SourceRow, chapter_from_row,
	default_value_to_json, json_to_default_value, manga_from_row, viewer_from_int,
};
use crate::source::cover;
use crate::source::{
	chapter_row, load_pages, manga_row, manga_rows_by_ids, reconcile_chapters, scan_sources_dir,
	upsert_manga,
};
use crate::state::{AppState, load_cookies, load_defaults};
use aidoku::{DeepLinkResult, FilterValue, Listing, Manga, UpdateStrategy};
use aidoku_wasm::Error as SourceError;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Extension, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn router(state: Arc<AppState>) -> Router {
	// Public surface: health check + auth itself (if login can't be reached, everything deadlocks)
	let public = Router::new()
		.route("/health", get(health))
		.route("/api/openapi.json", get(docs::openapi))
		.route("/api/docs", get(docs::index))
		.route("/api/docs/", get(docs::index))
		.route("/api/docs/{file}", get(docs::file))
		.route("/api/v1/version", get(version))
		.route("/api/v1/auth/status", get(crate::auth::status))
		.route("/api/v1/auth/login", post(crate::auth::login))
		.route("/api/v1/auth/logout", post(crate::auth::logout));
	// Global state and config; require_auth below establishes the single admin identity
	let admin = Router::new()
		.route("/api/v1/sources/scan", post(scan_sources))
		.route(
			"/api/v1/sources/import",
			post(source_import::import_source)
				.route_layer(axum::extract::DefaultBodyLimit::max(source_import::IMPORT_BODY_MAX)),
		)
		.route("/api/v1/sources/internal", post(internal_source::register_internal_source))
		.route("/api/v1/sources/reorder-pinned", post(pins::reorder_pinned))
		.route("/api/v1/sources/{id}", delete(delete_source).patch(pins::patch_source))
		.route("/api/v1/sources/{id}/update", post(repo::update_source))
		.route("/api/v1/sources/{id}/settings", get(get_source_settings).put(put_source_settings))
		.route("/api/v1/sources/{id}/settings/reset", post(reset_source_settings))
		.route("/api/v1/sources/{id}/notify", post(notify_source))
		.route("/api/v1/sources/{id}/cache/clear", post(clear_source_cache))
		.route("/api/v1/sources/{id}/login/basic", post(basic_login))
		.route("/api/v1/sources/{id}/login/web", post(login::web_login))
		.route("/api/v1/sources/{id}/login/oauth/start", post(oauth::oauth_start))
		.route("/api/v1/sources/{id}/login/oauth/complete", post(oauth::oauth_complete))
		.route("/api/v1/sources/{id}/login/logout", post(login::logout))
		.route(
			"/api/v1/source-repos",
			get(repo::list_repos).post(repo::add_repo),
		)
		.route("/api/v1/source-repos/{id}", delete(repo::delete_repo))
		.route("/api/v1/source-repos/{id}/refresh", post(repo::refresh_repo))
		.route("/api/v1/source-repos/sources", get(repo::browse_sources))
		.route("/api/v1/source-repos/install", post(repo::install_source))
		.route("/api/v1/source-repos/update-all", post(repo::update_all))
		.route("/api/v1/backups/files", get(backup::list_backups))
		.route("/api/v1/backups/files/restore", post(backup::restore_backup))
		.route("/api/v1/sync/compact", post(sync::compact));
	let protected = Router::new()
		.route("/api/v1/sources", get(list_sources))
		.route("/api/v1/search", post(search::search_all))
		.route("/api/v1/sources/{id}/icon", get(get_source_icon))
		.route("/api/v1/sources/{id}/image", get(cover::get_source_image))
		.route("/api/v1/sources/{id}/filters", get(get_filters))
		.route("/api/v1/sources/{id}/search", post(search_source))
		.route("/api/v1/sources/{id}/listings", get(get_listings))
		.route("/api/v1/sources/{id}/listings/{listing_id}", get(get_manga_list))
		.route("/api/v1/sources/{id}/home", get(get_home))
		.route("/api/v1/sources/{id}/deep-link", post(deep_link))
		.route("/api/v1/manga", post(register_manga))
		.route("/api/v1/manga/{id}", get(get_manga).patch(update_manga))
		.route("/api/v1/manga/{id}/chapters", get(get_chapters))
		.route(
			"/api/v1/manga/{id}/cover",
			get(cover::get_cover).put(cover::select_cover),
		)
		.route(
			"/api/v1/manga/{id}/covers/alternates",
			get(cover::get_alternate_covers),
		)
		.route("/api/v1/manga/{id}/categories", put(library::set_manga_categories))
		.route("/api/v1/chapter/{id}", patch(reader::update_chapter))
		.route("/api/v1/chapter/batch", post(reader::batch_update_chapters))
		.route("/api/v1/chapter/{id}/pages", get(get_pages))
		.route("/api/v1/chapter/{id}/page/{index}", get(page::get_page))
		.route("/api/v1/library", get(library::list_library).post(library::add_to_library))
		.route("/api/v1/library/source-counts", get(library::source_counts))
		.route("/api/v1/library/{id}", delete(library::remove_from_library))
		.route("/api/v1/migrate/sources", get(migrate::migrate_sources))
		.route("/api/v1/migrate/lookup", get(migrate::migrate_lookup))
		.route("/api/v1/migrate", post(migrate::migrate_execute))
		.route("/api/v1/categories", get(library::list_categories).post(library::create_category))
		.route(
			"/api/v1/categories/{id}",
			patch(library::update_category).delete(library::delete_category),
		)
		.route("/api/v1/categories/reorder", patch(library::reorder_categories))
		.route(
			"/api/v1/filter-groups",
			get(filter_groups::list_groups).post(filter_groups::create_group),
		)
		.route(
			"/api/v1/filter-groups/{id}",
			patch(filter_groups::update_group).delete(filter_groups::delete_group),
		)
		.route(
			"/api/v1/filter-groups/reorder",
			patch(filter_groups::reorder_groups),
		)
		.route("/api/v1/history", get(reader::get_history))
		.route(
			"/api/v1/downloads",
			get(crate::api::download::list_downloads).post(crate::api::download::enqueue_downloads),
		)
		.route("/api/v1/downloads/start", post(crate::api::download::start_downloads))
		.route("/api/v1/downloads/stop", post(crate::api::download::stop_downloads))
		.route("/api/v1/downloads/clear", post(crate::api::download::clear_downloads))
		.route(
			"/api/v1/downloads/{id}",
			delete(crate::api::download::delete_download),
		)
		.route("/api/v1/chapter/{id}/file", get(crate::api::download::get_chapter_file))
		.route("/api/v1/tasks", get(task::list_tasks))
		.route("/api/v1/tasks/{id}", delete(task::cancel_task))
		.route("/api/v1/local/scan", post(local::scan_now))
		.route(
			"/api/v1/local/trash",
			get(local::list_trash).delete(local::empty_trash),
		)
		.route(
			"/api/v1/local/trash/{id}",
			delete(local::purge_trash),
		)
		.route("/api/v1/local/trash/{id}/restore", post(local::restore_trash))
		.route("/api/v1/update/fetch", post(crate::api::update::fetch_updates))
		.route("/api/v1/update/summary", get(crate::api::update::get_summary))
		.route("/api/v1/update/recent", get(crate::api::update::get_recent))
		.route("/api/v1/events", get(crate::api::events::events))
		.route("/api/v1/settings", get(get_settings).put(put_settings))
		.route("/api/v1/sync/changes", get(sync::get_changes).post(sync::post_changes))
		.route(
			"/api/v1/backups/document",
			get(backup::export_backup)
				.post(backup::import_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route(
			"/api/v1/backups/import/aidoku",
			post(aidoku_backup::import_aidoku_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route(
			"/api/v1/backups/import/suwayomi",
			post(suwayomi_backup::import_suwayomi_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route("/api/v1/auth/password", put(crate::auth::change_password))
		.route(
			"/api/v1/auth/api-keys",
			get(api_keys::list_keys).post(api_keys::create_key),
		)
		.route("/api/v1/auth/api-keys/{id}", delete(api_keys::revoke_key))
		.merge(admin)
		.layer(axum::middleware::from_fn_with_state(
			state.clone(),
			crate::auth::require_auth,
		));
	public.merge(protected).with_state(state)
}

async fn health() -> Json<Value> {
	Json(json!({"status": "ok"}))
}

async fn version() -> Json<Value> {
	Json(json!({"version": env!("CARGO_PKG_VERSION")}))
}

pub(crate) async fn source_row(state: &AppState, id: i64) -> ApiResult<SourceRow> {
	let row: Option<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("source not found"))
}

async fn list_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source ORDER BY id",
	)
	.fetch_all(&state.pool)
	.await?;
	let latest = crate::source::repo::latest_versions(&state.pool).await?;
	let pinned = pins::pinned_map(&state.pool).await?;
	let sources: Vec<Value> = rows
		.iter()
		.map(|row| repo::source_json(row, &latest, &pinned))
		.collect();
	Ok(Json(json!(sources)))
}

async fn scan_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let outcome = scan_sources_dir(&state, &state.config.sources_dir()).await?;
	Ok(Json(serde_json::to_value(outcome)?))
}

async fn delete_source(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
	let row = source_row(&state, id).await?;
	// The default local instance while the deployment config explicitly names local roots would be
	// recreated empty on the next boot, so deleting it is refused until the config stops defining it
	if row.key == crate::model::LOCAL_SOURCE_KEY && state.config.local_dirs.is_some() {
		return Err(ApiError::bad_request(
			"the default local source is defined by the server config (SUMIDOKU_LOCAL_DIRS/local_dirs); remove it there first",
		));
	}
	state.evict_source(id).await;
	sqlx::query("DELETE FROM source WHERE id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	pins::prune_pinned(&state.pool, &row.key).await?;
	// Uninstall also drops the on-disk package, otherwise the next directory scan would
	// re-register the source. Internal instances (local/opds) live entirely in the database (no package).
	if !row.is_opds() && !row.is_local() {
		remove_source_package(&state.config.sources_dir(), std::path::Path::new(&row.wasm_path)).await;
	}
	Ok(StatusCode::NO_CONTENT)
}

/// Removes the package a source row was registered from: the containing directory for directory
/// packages (`wasm_path` is `<dir>/main.wasm`), the `.aix` file itself plus its written-out icon
/// sidecar otherwise. Only paths inside `sources_dir` are touched; a missing or undeletable
/// package only warns — the database row is already gone.
async fn remove_source_package(sources_dir: &std::path::Path, wasm_path: &std::path::Path) {
	if !wasm_path.starts_with(sources_dir) {
		tracing::warn!(?wasm_path, "source package outside sources_dir left in place");
		return;
	}
	let (package, is_dir) = match wasm_path.extension().map(|e| e.to_ascii_lowercase()) {
		Some(ext) if ext == "aix" => (wasm_path.to_path_buf(), false),
		_ => match wasm_path.parent() {
			Some(parent) if parent != wasm_path => (parent.to_path_buf(), true),
			_ => return,
		},
	};
	let result = if is_dir {
		tokio::fs::remove_dir_all(&package).await
	} else {
		tokio::fs::remove_file(&package).await
	};
	if let Err(e) = result {
		tracing::warn!(?package, "failed to remove source package: {e}");
		return;
	}
	if !is_dir {
		// the `.aix` icon sidecar (`<name>.png`, written at registration) dies with the package
		let _ = tokio::fs::remove_file(wasm_path.with_extension("png")).await;
	}
}

async fn get_source_icon(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Response> {
	let row = source_row(&state, id).await?;
	let Some(path) = row.icon_path else {
		return Err(ApiError::not_found("icon not found"));
	};
	let bytes = match tokio::fs::read(&path).await {
		Ok(bytes) => bytes,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			return Err(ApiError::not_found("icon not found"));
		}
		Err(e) => return Err(ApiError::from(sqlx::Error::Io(e))),
	};
	let mime = match path
		.rsplit('.')
		.next()
		.map(str::to_ascii_lowercase)
		.as_deref()
	{
		Some("png") => "image/png",
		Some("jpg" | "jpeg") => "image/jpeg",
		Some("webp") => "image/webp",
		Some("gif") => "image/gif",
		_ => "application/octet-stream",
	};
	Ok(cover::bytes_response(bytes, mime, 86400))
}

/// When a dynamic capability (filters/listings/home/deep-link) is unsupported, the endpoint convention answers 501 unsupported
fn dynamic_capability_error(e: SourceError) -> ApiError {
	match e {
		SourceError::Unimplemented => ApiError::unsupported_501("feature not supported by source"),
		SourceError::Unsupported(name) => {
			ApiError::unsupported_501(format!("feature not supported by source: {name}"))
		}
		other => ApiError::from(other),
	}
}

/// Reads a static json from the source package (source.json/settings.json/filters.json);
/// directory sources read the file next to wasm_path, .aix sources take it from the zip's `Payload/`; missing or unparsable both yield None
async fn package_json(state: &AppState, id: i64, file: &str) -> ApiResult<Option<Value>> {
	let row = source_row(state, id).await?;
	let Some(raw) = read_package_file(&row.wasm_path, file).await else {
		return Ok(None);
	};
	Ok(serde_json::from_slice(&raw).ok())
}

/// Static-file read shared by directory and .aix sources: try the file next to wasm_path first, then fall back to the zip's `Payload/<file>`
async fn read_package_file(wasm_path: &str, file: &str) -> Option<Vec<u8>> {
	let path = std::path::PathBuf::from(wasm_path);
	if let Some(dir) = path.parent()
		&& let Ok(raw) = tokio::fs::read(dir.join(file)).await
	{
		return Some(raw);
	}
	let entry = format!("Payload/{file}");
	tokio::task::spawn_blocking(move || read_aix_entry(&path, &entry))
		.await
		.ok()
		.flatten()
}

/// Reads an entry under `Payload/` from an `.aix` (zip); non-zip or missing entry both yield None
fn read_aix_entry(path: &std::path::Path, entry: &str) -> Option<Vec<u8>> {
	use aidoku_wasm::package::{JSON_ENTRY_MAX, read_zip_entry};
	let file = std::fs::File::open(path).ok()?;
	let mut archive = zip::ZipArchive::new(file).ok()?;
	read_zip_entry(&mut archive, entry, JSON_ENTRY_MAX).ok()
}

async fn get_filters(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	// Aligned with AidokuRunner: static filters first, dynamic exported results appended;
	// static items are normalized into the dynamic-export shape (dynamic items already have it, passed through as-is)
	let mut out: Vec<Value> = Vec::new();
	if let Some(doc) = package_json(&state, id, "source.json").await?
		&& let Some(filters) = doc.get("filters")
	{
		out.extend(filters::normalize_static_filters(filters));
	}
	if out.is_empty()
		&& let Some(filters) = package_json(&state, id, "filters.json").await?
	{
		out.extend(filters::normalize_static_filters(&filters));
	}
	if handle.features().dynamic_filters {
		match handle.get_filters().await {
			Ok(filters) => {
				if let Value::Array(items) = serde_json::to_value(filters)? {
					out.extend(items);
				}
			}
			Err(e @ (SourceError::Unimplemented | SourceError::Unsupported(_))) => {
				if out.is_empty() {
					return Err(dynamic_capability_error(e));
				}
			}
			Err(e) => return Err(ApiError::from(e)),
		}
	}
	if out.is_empty() {
		return Err(dynamic_capability_error(SourceError::Unsupported("get_filters")));
	}
	Ok(Json(Value::Array(out)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
	pub query: Option<String>,
	pub page: Option<i32>,
	pub filters: Option<Vec<FilterValue>>,
}

/// key → internal id for in-library manga; streaming and non-streaming search share the same attach semantics
pub(crate) async fn known_manga_ids(state: &AppState, source_id: i64) -> ApiResult<HashMap<String, i64>> {
	let known: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_all(&state.pool)
	.await?;
	Ok(known.into_iter().map(|(id, key)| (key, id)).collect())
}

pub(crate) fn manga_json_with_ids(
	manga: Manga,
	source_id: i64,
	known: &HashMap<String, i64>,
) -> ApiResult<Value> {
	let mut value = serde_json::to_value(&manga)?;
	let object = value
		.as_object_mut()
		.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
	object.insert("sourceId".into(), json!(source_id));
	if let Some(id) = known.get(&manga.key) {
		object.insert("id".into(), json!(id));
	}
	Ok(value)
}

/// Attaches sourceId to source-side Manga from search/list results; in-library ones also get the internal id
pub(crate) async fn attach_internal_ids(
	state: &AppState,
	source_id: i64,
	entries: Vec<Manga>,
) -> ApiResult<Vec<Value>> {
	let known = known_manga_ids(state, source_id).await?;
	entries
		.into_iter()
		.map(|manga| manga_json_with_ids(manga, source_id, &known))
		.collect()
}

/// Browse/search for the local source reads the scan index, never the filesystem (phase 2): rows come from
/// the manga table (trashed excluded — they are gone from disk and live in the trash view), the multi-root
/// dedup rule is unchanged (a numbered key is shadowed when the bare form exists), and a non-empty query
/// filters by title substring (case-insensitive, same as Suwayomi LocalSource). An instance with no rows at
/// all gets its first scan kicked at manual priority — the dedup id makes repeated browsing cheap — and the
/// web UI refreshes on the task SSE event; everything returns in one page, hasNextPage is always false
pub(crate) async fn local_search(state: &AppState, source_id: i64, query: Option<&str>) -> ApiResult<Vec<Value>> {
	let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_one(&state.pool)
		.await?;
	if rows == 0
		&& let Err(e) =
			crate::local::scan::enqueue_scan(state, Some(source_id), crate::task::PRIORITY_MANUAL).await
	{
		tracing::warn!(source_id, "enqueue first scan on browse failed: {}", e.message());
	}
	let mut rows: Vec<(i64, String, String)> = sqlx::query_as(
		"SELECT id, key, title FROM manga WHERE source_id = ? AND trashed_at IS NULL",
	)
	.bind(source_id)
	.fetch_all(&state.pool)
	.await?;
	// first directory wins: a numbered key (N:rel) is shadowed when the bare rel exists in the same instance
	let bare: HashSet<String> = rows
		.iter()
		.map(|(_, key, _)| key.clone())
		.filter(|key| crate::local::rel_of(key) == key)
		.collect();
	rows.retain(|(_, key, _)| crate::local::rel_of(key) == key || !bare.contains(crate::local::rel_of(key)));
	let needle = query
		.map(str::trim)
		.filter(|q| !q.is_empty())
		.map(str::to_lowercase);
	rows.retain(|(_, _, title)| needle.as_ref().is_none_or(|n| title.to_lowercase().contains(n)));
	rows.sort_by(|a, b| crate::local::natural_sort::compare(&a.1, &b.1));
	rows.into_iter()
		.map(|(id, key, title)| {
			let mut value = serde_json::to_value(Manga {
				key,
				title,
				..Manga::default()
			})?;
			let object = value
				.as_object_mut()
				.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
			object.insert("sourceId".into(), json!(source_id));
			object.insert("id".into(), json!(id));
			Ok(value)
		})
		.collect()
}

async fn search_source(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	headers: HeaderMap,
	Json(body): Json<SearchRequest>,
) -> ApiResult<Response> {
	let page = body.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	// The local source has no source call to stream: always the buffered path (ndjson clients treat it as a non-streaming terminal, same as register's local arm)
	if crate::source::source_is_local(&state.pool, id).await? {
		let entries = local_search(&state, id, body.query.as_deref()).await?;
		return Ok(Json(json!({
			"entries": entries,
			"hasNextPage": false,
		}))
		.into_response());
	}
	// opds sources likewise have no source call to stream; degraded marks a catalog without OpenSearch, where results are just traversal filtering
	if crate::source::source_is_opds(&state.pool, id).await? {
		let outcome = crate::opds_source::search(&state, id, body.query.as_deref()).await?;
		let entries = attach_internal_ids(&state, id, outcome.entries).await?;
		return Ok(Json(json!({
			"entries": entries,
			"hasNextPage": false,
			"degraded": outcome.degraded,
		}))
		.into_response());
	}
	let handle = state.get_handle(id).await?;
	if !partial::wants_ndjson(&headers) {
		let result = handle
			.get_search_manga_list(body.query, page, body.filters.unwrap_or_default())
			.await?;
		let entries = attach_internal_ids(&state, id, result.entries).await?;
		return Ok(Json(json!({
			"entries": entries,
			"hasNextPage": result.has_next_page,
		}))
		.into_response());
	}
	let known = known_manga_ids(&state, id).await?;
	let (line_tx, line_rx) = mpsc::unbounded_channel();
	let (partial_tx, partial_rx) = mpsc::unbounded_channel();
	let call = tokio::spawn(async move {
		handle
			.get_search_manga_list_streaming(body.query, page, body.filters.unwrap_or_default(), partial_tx)
			.await
	});
	tokio::spawn(partial::forward_search(partial_rx, call, line_tx, id, known));
	Ok(partial::ndjson_response(line_rx))
}

async fn get_listings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	// Aligned with AidokuRunner: static listings first, dynamic exported results appended;
	// both sources are normalized to the listing_json shape, where kind is a number for static and an enum name for dynamic
	let mut out: Vec<Value> = Vec::new();
	if let Some(doc) = package_json(&state, id, "source.json").await?
		&& let Some(items) = doc.get("listings").and_then(|v| v.as_array())
	{
		out.extend(
			items
				.iter()
				.filter_map(listings::parse_static_listing)
				.map(|l| listings::listing_json(&l)),
		);
	}
	if handle.features().dynamic_listings {
		match handle.get_listings().await {
			Ok(listings) => out.extend(listings.iter().map(listings::listing_json)),
			Err(e @ (SourceError::Unimplemented | SourceError::Unsupported(_))) => {
				if out.is_empty() {
					return Err(dynamic_capability_error(e));
				}
			}
			Err(e) => return Err(ApiError::from(e)),
		}
	}
	if out.is_empty() {
		return Err(dynamic_capability_error(SourceError::Unsupported("get_listings")));
	}
	Ok(Json(Value::Array(out)))
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
	pub page: Option<i32>,
}

/// Static (source.json listings) first, dynamic exported results appended, same order as the get_listings endpoint;
/// a failed dynamic fetch is only logged and we continue static-only, not dragging the manga list down
async fn combined_listings(
	state: &AppState,
	id: i64,
	handle: &aidoku_wasm::SourceHandle,
) -> ApiResult<Vec<Listing>> {
	let mut out: Vec<Listing> = Vec::new();
	if let Some(doc) = package_json(state, id, "source.json").await?
		&& let Some(items) = doc.get("listings").and_then(|v| v.as_array())
	{
		out.extend(items.iter().filter_map(listings::parse_static_listing));
	}
	if handle.features().dynamic_listings {
		match handle.get_listings().await {
			Ok(listings) => out.extend(listings),
			Err(SourceError::Unimplemented | SourceError::Unsupported(_)) => {}
			Err(e) => tracing::warn!(source_id = id, "dynamic listings fetch failed, static only: {e}"),
		}
	}
	Ok(out)
}

async fn get_manga_list(
	State(state): State<Arc<AppState>>,
	Path((id, listing_id)): Path<(i64, String)>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let handle = state.get_handle(id).await?;
	// Looks up the real listing by id among the source-declared listings (Aidoku only ever passes objects the source itself gave);
	// when not found, the id itself is passed through and the source decides how to respond, matching existing behavior
	let listing = combined_listings(&state, id, &handle)
		.await?
		.into_iter()
		.find(|l| l.id == listing_id)
		.unwrap_or(Listing {
			id: listing_id,
			name: String::new(),
			kind: aidoku::ListingKind::Default,
		});
	let result = handle.get_manga_list(listing, page).await?;
	let entries = attach_internal_ids(&state, id, result.entries).await?;
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": result.has_next_page,
	})))
}

/// The component array's order is the stable identity: in a streaming request, send_partial_result partials replace
/// by index incrementally without disturbing existing fields; the index is resolved by the wasm layer with upstream title-matching semantics (see partial.rs)
async fn get_home(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let handle = state.get_handle(id).await?;
	if !partial::wants_ndjson(&headers) {
		let home = handle.get_home().await.map_err(dynamic_capability_error)?;
		return Ok(Json(serde_json::to_value(home)?).into_response());
	}
	// A missing capability must return 501 before the stream starts, keeping the same status-code contract as non-streaming
	if !handle.features().home {
		return Err(dynamic_capability_error(SourceError::Unsupported("get_home")));
	}
	let (line_tx, line_rx) = mpsc::unbounded_channel();
	let (partial_tx, partial_rx) = mpsc::unbounded_channel();
	let call = tokio::spawn(async move { handle.get_home_streaming(partial_tx).await });
	tokio::spawn(partial::forward_home(partial_rx, call, line_tx));
	Ok(partial::ndjson_response(line_rx))
}

pub(crate) async fn current_values(state: &AppState, source_id: i64) -> ApiResult<Value> {
	let defaults = load_defaults(&state.pool, source_id).await?;
	let values: serde_json::Map<String, Value> = defaults
		.iter()
		.map(|(key, value)| (key.clone(), default_value_to_json(value)))
		.collect();
	Ok(Value::Object(values))
}

/// Settings-definition loading: static settings.json merged with dynamic get_settings (static first, dynamic appended,
/// same-key deduped — aligned with AidokuRunner's staticSettings + getSettings()); dynamic items also seed
/// default values, aligned with Runner's register(defaults:)
pub(crate) async fn load_settings_items(
	state: &AppState,
	id: i64,
	handle: &aidoku_wasm::SourceHandle,
) -> ApiResult<Vec<Value>> {
	let static_items = package_json(state, id, "settings.json")
		.await?
		.map(|raw| settings::normalize_static_settings(&raw))
		.unwrap_or_default();
	if !handle.features().dynamic_settings {
		return Ok(static_items);
	}
	match handle.get_settings().await {
		Ok(settings) => {
			let dynamic_items = serde_json::to_value(settings)?
				.as_array()
				.cloned()
				.unwrap_or_default();
			crate::source::seed_dynamic_defaults(&state.pool, id, &dynamic_items)
				.await
				.map_err(ApiError::internal)?;
			Ok(settings::merge_settings(static_items, dynamic_items))
		}
		Err(SourceError::Unimplemented | SourceError::Unsupported(_)) => Ok(static_items),
		Err(e) => Err(ApiError::from(e)),
	}
}

/// A local instance's settings document (no wasm package, so the definition is server-synthesized):
/// the roots list and the oneshots marker. Values are the effective — fallback-resolved — ones, so an
/// edit starts from what the instance actually scans; saving materializes them into source_setting
async fn local_settings_doc(state: &AppState, id: i64) -> ApiResult<Value> {
	let paths = state.local_dirs(id).await?;
	let oneshots = state.local_oneshots_dir(id).await?;
	let exclusions = state.local_exclusions(id).await?;
	let force_mtime = state.local_force_modified_time(id).await?;
	let analyze_pages = state.local_analyze_pages(id).await?;
	let analyze_metadata = state.local_analyze_metadata(id).await?;
	let toggle = |default: bool| json!({ "Toggle": { "subtitle": null, "auth_to_disable": null, "default": default } });
	Ok(json!({
		"settings": [
			// unknown-to-Aidoku type on purpose: the value keeps the raw string form (the same
			// convention as unknown static kinds), and the web UI renders the local editor by source kind
			{
				"type": "list", "key": "paths", "title": "Directories",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": "list",
			},
			{
				"type": "text", "key": "oneshotsDir", "title": "Oneshots directory",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": { "Text": {
					"placeholder": null, "autocapitalization_type": null, "autocorrection_disabled": null,
					"keyboard_type": null, "return_key_type": null, "secure": null, "default": null,
				} },
			},
			{
				"type": "list", "key": "exclusions", "title": "Directory exclusions",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": "list",
			},
			{
				"type": "checkbox", "key": "forceModifiedTime", "title": "Force directory modified time",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": toggle(state.config.local_scan_force_modified_time),
			},
			{
				"type": "checkbox", "key": "analyzePages", "title": "Analyze chapter pages",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": toggle(state.config.local_analyze_pages),
			},
			{
				"type": "checkbox", "key": "analyzeMetadata", "title": "Analyze embedded metadata",
				"notification": null, "requires": null, "requires_false": null, "refreshes": null,
				"value": toggle(state.config.local_analyze_metadata),
			},
		],
		"values": {
			"paths": paths.iter().map(|dir| dir.to_string_lossy()).collect::<Vec<_>>(),
			"oneshotsDir": oneshots,
			"exclusions": exclusions,
			"forceModifiedTime": force_mtime,
			"analyzePages": analyze_pages,
			"analyzeMetadata": analyze_metadata,
		},
	}))
}

async fn get_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let row = source_row(&state, id).await?;
	if row.is_local() {
		return Ok(Json(local_settings_doc(&state, id).await?));
	}
	let handle = state.get_handle(id).await?;
	let doc = package_json(&state, id, "source.json")
		.await?
		.unwrap_or_else(|| json!({}));
	let mut items = load_settings_items(&state, id, &handle).await?;
	// Synthetic settings rank before static/dynamic settings, aligned with AidokuRunner's extraSettings (language before base url);
	// when the source itself already declares the same key, the synthetic item is skipped so the list never shows two same-key settings
	let declared = settings::declared_keys(&items);
	let mut extras: Vec<Value> = Vec::new();
	if let Some(language) = settings::language_setting(&doc)
		&& !settings::clashes(&language, &declared)
	{
		extras.push(language);
	}
	if let Some(base_url) = settings::base_url_setting(&doc, row.base_url.as_deref())
		&& !settings::clashes(&base_url, &declared)
	{
		extras.push(base_url);
	}
	for (offset, extra) in extras.into_iter().enumerate() {
		items.insert(offset, extra);
	}
	let values = current_values(&state, id).await?;
	Ok(Json(json!({
		"settings": items,
		"values": values,
	})))
}

/// serde_urlencoded rejects duplicate keys; notify may repeat (one save can change several settings with
/// notifications, delivered one by one), so parse by hand
fn notify_params(raw: Option<&str>) -> Vec<String> {
	raw.map(|q| {
		url::form_urlencoded::parse(q.as_bytes())
			.filter_map(|(k, v)| (k == "notify").then(|| v.into_owned()))
			.collect()
	})
	.unwrap_or_default()
}

async fn put_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	RawQuery(raw): RawQuery,
	Json(body): Json<HashMap<String, Value>>,
) -> ApiResult<Json<Value>> {
	let row = source_row(&state, id).await?;
	// A local instance accepts exactly its synthesized settings: the roots list (validated against the
	// filesystem before the write transaction), the oneshots marker, the exclusion substrings (an explicit
	// empty array turns the rule off) and the three behavior toggles (null clears the override back to the
	// config layer)
	if row.is_local() {
		let mut parsed_paths: Option<Vec<String>> = None;
		// key → Some(value) writes, Some(None) clears the override; scan-affecting keys trigger a rescan
		let mut write_keys: Vec<(&str, Option<Value>)> = Vec::new();
		let mut rescan = false;
		for (key, value) in body.iter() {
			match key.as_str() {
				"paths" => {
					let paths = serde_json::from_value::<Vec<String>>(value.clone())
						.map_err(|_| ApiError::bad_request("paths must be an array of directory paths"))?;
					parsed_paths = Some(crate::local::validate_paths(&paths).await?);
					rescan = true;
				}
				"oneshotsDir" => {
					rescan = true;
					write_keys.push(("oneshotsDir", match value {
						Value::Null => None,
						Value::String(name) => Some(json!(name.trim())),
						_ => return Err(ApiError::bad_request("oneshotsDir must be a string or null")),
					}));
				}
				"exclusions" => {
					rescan = true;
					write_keys.push(("exclusions", match value {
						Value::Null => None,
						Value::Array(_) => {
							let list = serde_json::from_value::<Vec<String>>(value.clone())
								.map_err(|_| ApiError::bad_request("exclusions must be an array of substrings"))?;
							Some(json!(list.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>()))
						}
						_ => return Err(ApiError::bad_request("exclusions must be an array or null")),
					}));
				}
				"forceModifiedTime" | "analyzePages" | "analyzeMetadata" => {
					if key.as_str() == "forceModifiedTime" {
						rescan = true;
					}
					write_keys.push((key.as_str(), match value {
						Value::Null => None,
						Value::Bool(_) => Some(value.clone()),
						_ => return Err(ApiError::bad_request(format!("{key} must be a boolean or null"))),
					}));
				}
				_ => return Err(ApiError::bad_request(format!("unknown local source setting: {key}"))),
			}
		}
		let mut tx = crate::db::begin_write(&state.pool).await?;
		if let Some(dirs) = parsed_paths {
			let raw = serde_json::to_string(&json_to_default_value(&json!(dirs)))?;
			sqlx::query(
				"INSERT INTO source_setting (source_id, key, value) VALUES (?, 'paths', ?)
				 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
			)
			.bind(id)
			.bind(raw)
			.execute(&mut *tx)
			.await?;
		}
		for (key, value) in write_keys {
			match value {
				None => {
					sqlx::query("DELETE FROM source_setting WHERE source_id = ? AND key = ?")
						.bind(id)
						.bind(key)
						.execute(&mut *tx)
						.await?;
				}
				Some(value) => {
					let raw = serde_json::to_string(&json_to_default_value(&value))?;
					sqlx::query(
						"INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)
						 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
					)
					.bind(id)
					.bind(key)
					.bind(raw)
					.execute(&mut *tx)
					.await?;
				}
			}
		}
		tx.commit().await?;
		// kmrs's check_library_should_rescan: editing a scan-affecting setting re-scans the instance
		if rescan
			&& let Err(e) = crate::local::scan::enqueue_scan(&state, Some(id), crate::task::PRIORITY_MANUAL).await
		{
			tracing::warn!(source_id = id, "enqueue rescan on settings change failed: {}", e.message());
		}
		// No instance to restart: the next scan reads the new values
		let values = local_settings_doc(&state, id).await?["values"].clone();
		return Ok(Json(json!({ "values": values })));
	}
	{
		let mut tx = crate::db::begin_write(&state.pool).await?;
		for (key, value) in &body {
			let encoded = serde_json::to_string(&json_to_default_value(value))?;
			sqlx::query(
				"INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)
				 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
			)
			.bind(id)
			.bind(key)
			.bind(encoded)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
	}
	// Hot-restart only when the instance is running; a stopped one picks up the new defaults on its next lazy load
	let handle = state
		.sources
		.read()
		.await
		.get(&id)
		.map(|entry| entry.handle.clone());
	if let Some(handle) = handle {
		let defaults = load_defaults(&state.pool, id).await?;
		let cookies = load_cookies(&state.pool, id).await?;
		let features = handle.restart_with(defaults, cookies).await?;
		if let Some(entry) = state.sources.write().await.get_mut(&id) {
			entry.features = features.clone();
		}
		if features.notification {
			for notify in notify_params(raw.as_deref()) {
				handle.handle_notification(notify).await?;
			}
		}
	}
	let values = current_values(&state, id).await?;
	Ok(Json(json!({ "values": values })))
}

#[derive(Debug, Deserialize)]
pub struct NotifyRequest {
	pub notification: String,
}

/// Sends the notification straight to the running (or lazily started) instance without a restart — the press
/// semantics of a Button setting (Aidoku app: pressing fires handleNotification, instance state kept)
async fn notify_source(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<NotifyRequest>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	if !handle.features().notification {
		return Err(ApiError::unsupported_501("source does not support notifications"));
	}
	handle.handle_notification(body.notification).await?;
	Ok(Json(json!({ "ok": true })))
}

/// Hot-restart only when the instance is running (swapping defaults/cookies); a stopped one picks up the new values on its next lazy load
pub(crate) async fn restart_if_loaded(state: &AppState, id: i64) -> ApiResult<()> {
	let handle = state
		.sources
		.read()
		.await
		.get(&id)
		.map(|entry| entry.handle.clone());
	if let Some(handle) = handle {
		let defaults = load_defaults(&state.pool, id).await?;
		let cookies = load_cookies(&state.pool, id).await?;
		let features = handle.restart_with(defaults, cookies).await?;
		if let Some(entry) = state.sources.write().await.get_mut(&id) {
			entry.features = features;
		}
	}
	Ok(())
}

/// Clears the source's settings overrides and re-seeds defaults from the package's static schema (aligned with reinstall semantics)
async fn reset_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let row = source_row(&state, id).await?;
	if row.is_local() {
		// The roots list has no default to reset to (registration requires it); every other override clears,
		// falling back to the legacy global layer / config
		sqlx::query(
			"DELETE FROM source_setting WHERE source_id = ? AND key IN
			 ('oneshotsDir', 'exclusions', 'forceModifiedTime', 'analyzePages', 'analyzeMetadata')",
		)
		.bind(id)
		.execute(&state.pool)
		.await?;
		let values = local_settings_doc(&state, id).await?["values"].clone();
		return Ok(Json(json!({ "values": values })));
	}
	let settings = package_json(&state, id, "settings.json").await?;
	let doc = package_json(&state, id, "source.json")
		.await?
		.unwrap_or_else(|| json!({}));
	sqlx::query("DELETE FROM source_setting WHERE source_id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	crate::source::seed_source_defaults(&state.pool, id, settings.as_ref(), &doc)
		.await
		.map_err(ApiError::internal)?;
	restart_if_loaded(&state, id).await?;
	let values = current_values(&state, id).await?;
	Ok(Json(json!({ "values": values })))
}

/// Clears the source's login cookies and restarts the instance; cover cache and downloads are untouched
async fn clear_source_cache(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	source_row(&state, id).await?;
	sqlx::query("DELETE FROM source_cookie WHERE source_id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	restart_if_loaded(&state, id).await?;
	Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicLoginRequest {
	pub key: String,
	pub username: String,
	pub password: String,
}

/// Basic login: credentials go to the source for verification; on success the in-source sink persists them (username/password/marker)
async fn basic_login(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<BasicLoginRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let handle = state.get_handle(id).await?;
	if !handle.features().basic_login {
		return Err(ApiError::unsupported_501("source does not support basic login"));
	}
	let ok = handle
		.handle_basic_login(body.key, body.username, body.password)
		.await?;
	if !ok {
		return Err(ApiError::bad_gateway("basic login failed"));
	}
	Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMangaRequest {
	pub source_id: i64,
	#[serde(default)]
	pub manga_key: String,
	/// The search entry in the client's hands; when present it becomes the get_manga_update input,
	/// so fields the source did not change (typically cover) are not lost
	pub manga: Option<Manga>,
}

async fn register_manga(
	State(state): State<Arc<AppState>>,
	headers: HeaderMap,
	Json(body): Json<RegisterMangaRequest>,
) -> ApiResult<Response> {
	let initial = match body.manga {
		Some(manga) => manga,
		None => {
			if body.manga_key.is_empty() {
				return Err(ApiError::bad_request("mangaKey must not be empty"));
			}
			Manga {
				key: body.manga_key.clone(),
				..Manga::default()
			}
		}
	};
	if initial.key.is_empty() {
		return Err(ApiError::bad_request("mangaKey must not be empty"));
	}
	source_row(&state, body.source_id).await?;
	// The local source has no source call to stream: always the buffered path (ndjson clients treat it as a non-streaming terminal)
	if crate::source::source_is_local(&state.pool, body.source_id).await? {
		let manga_id = find_or_register_manga(&state, body.source_id, initial).await?;
		return Ok(Json(json!({ "id": manga_id })).into_response());
	}
	// opds sources same as the local arm: fetching upstream is a catalog feed pull, with no partial stream
	if crate::source::source_is_opds(&state.pool, body.source_id).await? {
		let manga_id = find_or_register_manga(&state, body.source_id, initial).await?;
		return Ok(Json(json!({ "id": manga_id })).into_response());
	}
	if !partial::wants_ndjson(&headers) {
		let manga_id = find_or_register_manga(&state, body.source_id, initial).await?;
		return Ok(Json(json!({ "id": manga_id })).into_response());
	}
	// An already-registered hit has no source call to stream: answer buffered directly, the client treats it as a non-streaming terminal
	let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(body.source_id)
		.bind(&initial.key)
		.fetch_optional(&state.pool)
		.await?;
	if let Some((id,)) = existing {
		return Ok(Json(json!({ "id": id })).into_response());
	}
	let handle = state.get_handle(body.source_id).await?;
	let (line_tx, line_rx) = mpsc::unbounded_channel();
	let (partial_tx, partial_rx) = mpsc::unbounded_channel();
	let source_id = body.source_id;
	let call = tokio::spawn(async move {
		let updated = handle
			.get_manga_update_streaming(initial, true, true, partial_tx)
			.await?;
		let mut conn = state.pool.acquire().await?;
		let manga_id = upsert_manga(&mut conn, source_id, &updated, true, true, true).await?;
		reconcile_chapters(
			&mut conn,
			manga_id,
			&updated.chapters.clone().unwrap_or_default(),
			&[],
		)
		.await?;
		Ok::<_, ApiError>(json!({ "id": manga_id }))
	});
	tokio::spawn(partial::forward_manga_update(partial_rx, call, line_tx));
	Ok(partial::ndjson_response(line_rx))
}

/// When (source, key) already has a manga row, hit it directly; otherwise fetch details + chapters from the source and
/// register (not shelved) — the path shared by opening an entry from the browse page and by deep-link landing
pub(crate) async fn find_or_register_manga(state: &Arc<AppState>, source_id: i64, initial: Manga) -> ApiResult<i64> {
	let existing: Option<(i64, bool)> =
		sqlx::query_as("SELECT id, initialized FROM manga WHERE source_id = ? AND key = ?")
			.bind(source_id)
			.bind(&initial.key)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((id, initialized)) = existing {
		// An indexed-but-unanalyzed local row opens cheap: the metadata chain belongs to the background
		// analysis pass, so opening one just kicks that pass at manual priority (the per-instance dedup
		// id makes repeated opens free); the web UI refreshes on the task SSE event
		if !initialized
			&& crate::source::source_is_local(&state.pool, source_id).await?
			&& let Err(e) = state
				.tasks()
				.enqueue(
					&format!("{}:{source_id}", crate::local::scan::KIND_ANALYZE),
					crate::local::scan::KIND_ANALYZE,
					Some(source_id),
					json!({}),
					crate::task::PRIORITY_MANUAL,
				)
				.await
		{
			tracing::warn!(source_id, "enqueue analysis on open failed: {}", e.message());
		}
		return Ok(id);
	}
	// The local arm registers a never-indexed manga through the full manga_update chain (its first analysis,
	// synchronous so the detail page opens complete); indexed rows never reach here — the existing-hit fast
	// path above already returned, and refreshes of persisted rows are entry-level scans
	if crate::source::source_is_local(&state.pool, source_id).await? {
		let update = crate::local::manga_update(
			&state.local_dirs(source_id).await?,
			&state.local_zip,
			&initial.key,
			&state.local_exclusions(source_id).await?,
		)
		.await?;
		let mut conn = state.pool.acquire().await?;
		let manga_id = upsert_manga(&mut conn, source_id, &update.manga, true, true, true).await?;
		crate::source::reconcile_chapters_detailed(
			&mut conn,
			manga_id,
			&update.manga.chapters.clone().unwrap_or_default(),
			&update.protected,
			true,
		)
		.await?;
		crate::local::store_epub_divina(
			&mut conn,
			manga_id,
			&update.manga.chapters.clone().unwrap_or_default(),
			&update.epub_divina,
		)
		.await?;
		crate::local::store_chapter_metadata(&mut conn, manga_id, &update.chapter_meta).await?;
		return Ok(manga_id);
	}
	if crate::source::source_is_opds(&state.pool, source_id).await? {
		let updated = crate::opds_source::manga_update(state, source_id, initial).await?;
		let mut conn = state.pool.acquire().await?;
		let manga_id = upsert_manga(&mut conn, source_id, &updated, true, true, true).await?;
		reconcile_chapters(
			&mut conn,
			manga_id,
			&updated.chapters.clone().unwrap_or_default(),
			&[],
		)
		.await?;
		return Ok(manga_id);
	}
	let handle = state.get_handle(source_id).await?;
	let updated = handle.get_manga_update(initial, true, true).await?;
	let mut conn = state.pool.acquire().await?;
	let manga_id = upsert_manga(&mut conn, source_id, &updated, true, true, true).await?;
	reconcile_chapters(
		&mut conn,
		manga_id,
		&updated.chapters.clone().unwrap_or_default(),
		&[],
	)
	.await?;
	Ok(manga_id)
}

#[derive(Debug, Deserialize)]
pub struct DeepLinkRequest {
	pub url: String,
}

/// Hands the URL to the source's handle_deep_link and resolves it into a target the frontend can navigate to
/// directly: Manga/Chapter variants register/hit the manga row by (source, key) and return the internal id — when
/// the chapter key is not in the chapter list, chapterId is null and the frontend lands on the manga page; Listing
/// is passed through as-is for the frontend to switch to; when the source does not recognize the URL, result is null
async fn deep_link(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<DeepLinkRequest>,
) -> ApiResult<Json<Value>> {
	if body.url.trim().is_empty() {
		return Err(ApiError::bad_request("url must not be empty"));
	}
	let handle = state.get_handle(id).await?;
	let linked = handle
		.handle_deep_link(body.url)
		.await
		.map_err(dynamic_capability_error)?;
	let Some(linked) = linked else {
		return Ok(Json(json!({ "result": null })));
	};
	let mut result = json!({
		"mangaKey": null,
		"chapterKey": null,
		"listing": null,
		"mangaId": null,
		"chapterId": null,
	});
	match linked {
		DeepLinkResult::Manga { key } => {
			let manga_id = find_or_register_manga(&state, id, Manga {
				key: key.clone(),
				..Manga::default()
			})
			.await?;
			result["mangaKey"] = json!(key);
			result["mangaId"] = json!(manga_id);
		}
		DeepLinkResult::Chapter { manga_key, key } => {
			let manga_id = find_or_register_manga(&state, id, Manga {
				key: manga_key.clone(),
				..Manga::default()
			})
			.await?;
			let chapter_id: Option<i64> =
				sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
					.bind(manga_id)
					.bind(&key)
					.fetch_optional(&state.pool)
					.await?;
			result["mangaKey"] = json!(manga_key);
			result["chapterKey"] = json!(key);
			result["mangaId"] = json!(manga_id);
			result["chapterId"] = json!(chapter_id);
		}
		DeepLinkResult::Listing(listing) => {
			result["listing"] = serde_json::to_value(listing)?;
		}
	}
	Ok(Json(json!({ "result": result })))
}

#[derive(Debug, Deserialize)]
pub struct RefreshQuery {
	pub refresh: Option<String>,
}

/// Query params of the chapter list endpoint: refresh plus the sort key and direction
#[derive(Debug, Deserialize)]
pub struct ChaptersQuery {
	pub refresh: Option<String>,
	pub sort: Option<String>,
	pub order: Option<String>,
}

pub(crate) fn wants_refresh(query: &RefreshQuery) -> bool {
	refresh_flag(query.refresh.as_deref())
}

fn refresh_flag(refresh: Option<&str>) -> bool {
	matches!(refresh, Some("") | Some("1") | Some("true"))
}

/// Chapter list ORDER BY: `sort` picks the key (`source` = source_order, `chapter` = number, `date` = date_uploaded),
/// `order=asc|desc` the direction — absent keeps the key's default: source stays in the source's listing order
/// (ASC), chapter and date default to newest-first (DESC). Numberless chapters count lowest, so they open ASC and
/// close DESC; missing dates sink to the bottom of the default date order (SQLite sorts NULL below any value)
fn chapter_order_by(sort: Option<&str>, order: Option<&str>) -> ApiResult<&'static str> {
	let asc = match order {
		None => None,
		Some("asc") => Some(true),
		Some("desc") => Some(false),
		Some(other) => return Err(ApiError::bad_request(format!("invalid order: {other}"))),
	};
	match (sort, asc) {
		(None, None | Some(true)) | (Some("source"), None | Some(true)) => {
			Ok(" ORDER BY c.source_order ASC, c.id ASC")
		}
		(None, Some(false)) | (Some("source"), Some(false)) => Ok(" ORDER BY c.source_order DESC, c.id ASC"),
		(Some("chapter"), Some(true)) => Ok(" ORDER BY c.number ASC, c.source_order ASC, c.id ASC"),
		(Some("chapter"), None | Some(false)) => Ok(" ORDER BY c.number DESC, c.source_order ASC, c.id ASC"),
		(Some("date"), Some(true)) => Ok(" ORDER BY c.date_uploaded ASC, c.source_order ASC, c.id ASC"),
		(Some("date"), None | Some(false)) => Ok(" ORDER BY c.date_uploaded DESC, c.source_order ASC, c.id ASC"),
		(Some(other), _) => Err(ApiError::bad_request(format!("invalid sort: {other}"))),
	}
}

pub(crate) async fn manga_json(state: &AppState, user_id: i64, row: &MangaRow) -> ApiResult<Value> {
	let (chapter_count,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(row.id)
			.fetch_one(&state.pool)
			.await?;
	let manga_user: Option<(bool, i64, bool)> =
		sqlx::query_as("SELECT in_library, viewer_flags, incognito FROM manga_user WHERE manga_id = ? AND user_id = ?")
			.bind(row.id)
			.bind(user_id)
			.fetch_optional(&state.pool)
			.await?;
	let mut value = serde_json::to_value(manga_from_row(row))?;
	let object = value
		.as_object_mut()
		.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
	if let Some((_, flags, _)) = manga_user {
		apply_viewer_override(object, flags)?;
	}
	object.insert("id".into(), json!(row.id));
	object.insert("sourceId".into(), json!(row.source_id));
	object.insert("initialized".into(), json!(row.initialized));
	// kmrs's deleted flag: trashed local rows stay visible until purged; hiding is the client's choice
	object.insert("trashed".into(), json!(row.trashed_at.is_some()));
	object.insert("chapterCount".into(), json!(chapter_count));
	object.insert(
		"inLibrary".into(),
		json!(manga_user.map(|(flag, _, _)| flag).unwrap_or(false)),
	);
	object.insert(
		"incognito".into(),
		json!(manga_user.map(|(_, _, incognito)| incognito).unwrap_or(false)),
	);
	let category_ids: Vec<i64> =
		sqlx::query_scalar("SELECT category_id FROM category_manga WHERE manga_id = ? AND user_id = ?")
			.bind(row.id)
			.bind(user_id)
			.fetch_all(&state.pool)
			.await?;
	object.insert("categoryIds".into(), json!(category_ids));
	Ok(value)
}

/// A non-zero viewer_flags is the user's reading-direction override: the response's viewer uses the override, the stored source value untouched
fn apply_viewer_override(object: &mut serde_json::Map<String, Value>, viewer_flags: i64) -> ApiResult<()> {
	if viewer_flags != 0 {
		object.insert("viewer".into(), serde_json::to_value(viewer_from_int(viewer_flags))?);
	}
	Ok(())
}

/// Batch manga_json for list endpoints: three IN queries instead of three queries per row, output in input order;
/// missing rows are filled with the single-fetch defaults (0 / false / empty array)
pub(crate) async fn manga_json_batch(state: &AppState, user_id: i64, rows: &[&MangaRow]) -> ApiResult<Vec<Value>> {
	if rows.is_empty() {
		return Ok(Vec::new());
	}
	let placeholders = rows.iter().map(|_| "?").collect::<Vec<_>>().join(",");
	let count_sql = format!(
		"SELECT manga_id, COUNT(*) FROM chapter WHERE manga_id IN ({placeholders}) GROUP BY manga_id"
	);
	let mut q = sqlx::query_as::<_, (i64, i64)>(&count_sql);
	for row in rows {
		q = q.bind(row.id);
	}
	let chapter_counts: HashMap<i64, i64> = q.fetch_all(&state.pool).await?.into_iter().collect();
	let library_sql = format!(
		"SELECT manga_id, in_library, viewer_flags, incognito FROM manga_user WHERE user_id = ? AND manga_id IN ({placeholders})"
	);
	let mut q = sqlx::query_as::<_, (i64, bool, i64, bool)>(&library_sql).bind(user_id);
	for row in rows {
		q = q.bind(row.id);
	}
	let manga_users: HashMap<i64, (bool, i64, bool)> = q
		.fetch_all(&state.pool)
		.await?
		.into_iter()
		.map(|(id, in_library, flags, incognito)| (id, (in_library, flags, incognito)))
		.collect();
	let category_sql = format!(
		"SELECT manga_id, category_id FROM category_manga WHERE user_id = ? AND manga_id IN ({placeholders})"
	);
	let mut q = sqlx::query_as::<_, (i64, i64)>(&category_sql).bind(user_id);
	for row in rows {
		q = q.bind(row.id);
	}
	let mut category_ids: HashMap<i64, Vec<i64>> = HashMap::new();
	for (manga_id, category_id) in q.fetch_all(&state.pool).await? {
		category_ids.entry(manga_id).or_default().push(category_id);
	}
	let mut out = Vec::with_capacity(rows.len());
	for row in rows {
		let mut value = serde_json::to_value(manga_from_row(row))?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
		object.insert("id".into(), json!(row.id));
		object.insert("sourceId".into(), json!(row.source_id));
		object.insert("initialized".into(), json!(row.initialized));
		object.insert("trashed".into(), json!(row.trashed_at.is_some()));
		object.insert(
			"chapterCount".into(),
			json!(chapter_counts.get(&row.id).copied().unwrap_or(0)),
		);
		object.insert(
			"inLibrary".into(),
			json!(manga_users.get(&row.id).map(|(flag, _, _)| *flag).unwrap_or(false)),
		);
		object.insert(
			"incognito".into(),
			json!(manga_users.get(&row.id).map(|(_, _, incognito)| *incognito).unwrap_or(false)),
		);
		if let Some((_, flags, _)) = manga_users.get(&row.id) {
			apply_viewer_override(object, *flags)?;
		}
		object.insert(
			"categoryIds".into(),
			json!(category_ids.get(&row.id).cloned().unwrap_or_default()),
		);
		out.push(value);
	}
	Ok(out)
}

/// The manga column for history/update: batch-fetch rows then batch-serialize, output in ids order; a missing row is the same 404 as manga_row
pub(crate) async fn manga_json_by_ids(state: &AppState, user_id: i64, ids: &[i64]) -> ApiResult<Vec<Value>> {
	let rows = manga_rows_by_ids(&state.pool, ids).await?;
	let refs: Vec<&MangaRow> = ids
		.iter()
		.map(|id| rows.get(id))
		.collect::<Option<Vec<_>>>()
		.ok_or_else(|| ApiError::not_found("manga not found"))?;
	manga_json_batch(state, user_id, &refs).await
}

/// serde_json prints an f32 by widening to f64 with f64's shortest representation (281.1 becomes 281.1000061035156),
/// so re-render with f32's own shortest representation to get the clean decimal back
fn f32_json(value: f64) -> Value {
	format!("{}", value as f32)
		.parse::<f64>()
		.map(Value::from)
		.unwrap_or(Value::Null)
}

pub(crate) fn chapter_json(row: &ChapterRow, progress: Option<&ChapterProgress>) -> ApiResult<Value> {
	let mut value = serde_json::to_value(chapter_from_row(row))?;
	let object = value
		.as_object_mut()
		.ok_or_else(|| ApiError::bad_request("unexpected chapter shape"))?;
	if let Some(number) = row.number {
		object.insert("chapter_number".into(), f32_json(number));
	}
	if let Some(volume) = row.volume {
		object.insert("volume_number".into(), f32_json(volume));
	}
	let progress = progress.copied().unwrap_or_default();
	object.insert("id".into(), json!(row.id));
	object.insert("trashed".into(), json!(row.trashed_at.is_some()));
	object.insert("pageCount".into(), json!(row.page_count));
	object.insert("lastPageRead".into(), json!(progress.last_page_read));
	object.insert("isRead".into(), json!(progress.is_read));
	object.insert("isBookmarked".into(), json!(progress.is_bookmarked));
	object.insert("lastReadAt".into(), json!(progress.last_read_at));
	Ok(value)
}

pub(crate) fn progress_of(row: &ChapterUserRow) -> Option<ChapterProgress> {
	// The last_page_read column is non-null, so a NULL from the LEFT JOIN means the user row does not exist
	row.last_page_read.map(|last_page_read| ChapterProgress {
		last_page_read,
		is_read: row.is_read.unwrap_or(false),
		is_bookmarked: row.is_bookmarked.unwrap_or(false),
		last_read_at: row.last_read_at,
	})
}

async fn get_manga(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let mut row = manga_row(&state.pool, id).await?;
	if wants_refresh(&query) {
		// Local-source refresh = entry-level rescan (enumeration only): chapter changes reconcile here,
		// embedded metadata belongs to the analysis pass the rescan kicks; no source call to stream
		if crate::source::source_is_local(&state.pool, row.source_id).await? {
			crate::local::scan::scan_manga_entry(&state, row.source_id, &row.key).await?;
			row = manga_row(&state.pool, id).await?;
			return Ok(Json(manga_json(&state, user.id, &row).await?).into_response());
		}
		// opds-source refresh = re-pull the entry feed; like the local arm, nothing to stream
		if crate::source::source_is_opds(&state.pool, row.source_id).await? {
			let mut updated = crate::opds_source::manga_update(&state, row.source_id, manga_from_row(&row)).await?;
			updated.update_strategy = crate::model::update_strategy_from_int(row.update_strategy);
			let mut conn = state.pool.acquire().await?;
			upsert_manga(&mut conn, row.source_id, &updated, true, true, false).await?;
			row = manga_row(&state.pool, id).await?;
			return Ok(Json(manga_json(&state, user.id, &row).await?).into_response());
		}
		let handle = state.get_handle(row.source_id).await?;
		if partial::wants_ndjson(&headers) {
			let (line_tx, line_rx) = mpsc::unbounded_channel();
			let (partial_tx, partial_rx) = mpsc::unbounded_channel();
			let input = manga_from_row(&row);
			let source_id = row.source_id;
			let user_id = user.id;
			let call = tokio::spawn(async move {
				let updated = handle.get_manga_update_streaming(input, true, false, partial_tx).await?;
				let mut conn = state.pool.acquire().await?;
				upsert_manga(&mut conn, source_id, &updated, true, true, false).await?;
				let row = manga_row(&state.pool, id).await?;
				manga_json(&state, user_id, &row).await
			});
			tokio::spawn(partial::forward_manga_update(partial_rx, call, line_tx));
			return Ok(partial::ndjson_response(line_rx));
		}
		let updated = handle
			.get_manga_update(manga_from_row(&row), true, false)
			.await?;
		let mut conn = state.pool.acquire().await?;
		upsert_manga(&mut conn, row.source_id, &updated, true, true, false).await?;
		row = manga_row(&state.pool, id).await?;
	}
	Ok(Json(manga_json(&state, user.id, &row).await?).into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMangaRequest {
	pub update_strategy: Option<UpdateStrategy>,
	pub incognito: Option<bool>,
}

/// Parses the body by hand: axum's Json rejection is 422, while the endpoint convention for invalid values is 400; an empty body is a no-op
async fn update_manga(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	manga_row(&state.pool, id).await?;
	let patch = match body {
		None => UpdateMangaRequest::default(),
		Some(Json(value)) => serde_json::from_value(value)
			.map_err(|e| ApiError::bad_request(format!("invalid manga patch: {e}")))?,
	};
	if let Some(strategy) = patch.update_strategy {
		sqlx::query("UPDATE manga SET update_strategy = ? WHERE id = ?")
			.bind(strategy as i64)
			.bind(id)
			.execute(&state.pool)
			.await?;
	}
	if let Some(incognito) = patch.incognito {
		// The manga_user row may not exist (not in library): per-manga incognito does not depend on library state, upsert creates the row
		sqlx::query(
			"INSERT INTO manga_user (manga_id, user_id, incognito) VALUES (?, ?, ?)
			 ON CONFLICT (manga_id, user_id) DO UPDATE SET incognito = excluded.incognito",
		)
		.bind(id)
		.bind(user.id)
		.bind(incognito)
		.execute(&state.pool)
		.await?;
	}
	let row = manga_row(&state.pool, id).await?;
	Ok(Json(manga_json(&state, user.id, &row).await?))
}

async fn get_chapters(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Query(query): Query<ChaptersQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let order_by = chapter_order_by(query.sort.as_deref(), query.order.as_deref())?;
	let manga = manga_row(&state.pool, id).await?;
	if refresh_flag(query.refresh.as_deref()) {
		// Local-source refresh = entry-level rescan (enumeration only): chapter entries that fail to read go
		// into the reconcile protection set, not treated as vanished-and-trashed; metadata/page analysis is
		// the background pass the rescan kicks
		if crate::source::source_is_local(&state.pool, manga.source_id).await? {
			crate::local::scan::scan_manga_entry(&state, manga.source_id, &manga.key).await?;
			sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
				.bind(crate::model::unix_now())
				.bind(id)
				.execute(&state.pool)
				.await?;
			return Ok(Json(chapters_json(&state, user.id, id, order_by).await?).into_response());
		}
		// opds-source refresh = re-pull the entry feed; chapters gone upstream follow the uniform reconcile rules
		if crate::source::source_is_opds(&state.pool, manga.source_id).await? {
			let updated = crate::opds_source::manga_update(&state, manga.source_id, manga_from_row(&manga)).await?;
			let mut conn = state.pool.acquire().await?;
			reconcile_chapters(&mut conn, id, &updated.chapters.clone().unwrap_or_default(), &[]).await?;
			sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
				.bind(crate::model::unix_now())
				.bind(id)
			.execute(&mut *conn)
			.await?;
			return Ok(Json(chapters_json(&state, user.id, id, order_by).await?).into_response());
		}
		let handle = state.get_handle(manga.source_id).await?;
		if partial::wants_ndjson(&headers) {
			let (line_tx, line_rx) = mpsc::unbounded_channel();
			let (partial_tx, partial_rx) = mpsc::unbounded_channel();
			let user_id = user.id;
			let call = tokio::spawn(async move {
				let updated = handle
					.get_manga_update_streaming(manga_from_row(&manga), false, true, partial_tx)
					.await?;
				let mut conn = state.pool.acquire().await?;
				reconcile_chapters(&mut conn, id, &updated.chapters.clone().unwrap_or_default(), &[]).await?;
				sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
					.bind(crate::model::unix_now())
					.bind(id)
					.execute(&mut *conn)
					.await?;
				chapters_json(&state, user_id, id, order_by).await
			});
			tokio::spawn(partial::forward_manga_update(partial_rx, call, line_tx));
			return Ok(partial::ndjson_response(line_rx));
		}
		let updated = handle
			.get_manga_update(manga_from_row(&manga), false, true)
			.await?;
		let mut conn = state.pool.acquire().await?;
		reconcile_chapters(&mut conn, id, &updated.chapters.clone().unwrap_or_default(), &[]).await?;
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(crate::model::unix_now())
			.bind(id)
			.execute(&mut *conn)
			.await?;
	}
	Ok(Json(chapters_json(&state, user.id, id, order_by).await?).into_response())
}

async fn chapters_json(state: &AppState, user_id: i64, manga_id: i64, order_by: &str) -> ApiResult<Value> {
	let rows: Vec<ChapterUserRow> = sqlx::query_as(&format!(
		"SELECT c.*, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter c
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = ?{order_by}"
	))
	.bind(user_id)
	.bind(manga_id)
	.fetch_all(&state.pool)
	.await?;
	let downloaded: HashSet<i64> = sqlx::query_scalar(
		"SELECT chapter_id FROM chapter_user WHERE user_id = ? AND is_downloaded = 1
		 AND chapter_id IN (SELECT id FROM chapter WHERE manga_id = ?)",
	)
	.bind(user_id)
	.bind(manga_id)
	.fetch_all(&state.pool)
	.await?
	.into_iter()
	.collect();
	let mut chapters = Vec::with_capacity(rows.len());
	for row in &rows {
		let mut value = chapter_json(&row.chapter, progress_of(row).as_ref())?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected chapter shape"))?;
		object.insert("isDownloaded".into(), json!(downloaded.contains(&row.chapter.id)));
		chapters.push(value);
	}
	Ok(json!(chapters))
}

pub(crate) async fn get_pages(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	chapter_row(&state.pool, id).await?;
	// A finished download product is the preferred page list: entries are derived from the artifact
	// with no source access, so downloaded chapters stay readable offline. An explicit refresh still
	// pulls from the source; without a product the page cache/source path is unchanged.
	let pages = if wants_refresh(&query) {
		load_pages(&state, id, true).await?
	} else if let Some(pages) = crate::download::pages_from_product(&state, id).await? {
		pages
	} else {
		load_pages(&state, id, false).await?
	};
	if let Err(e) = state.downloads().enqueue_reader_ahead(id, user.id).await {
		tracing::warn!(chapter_id = id, "reader-ahead enqueue failed: {}", e.message());
	}
	let mut out = Vec::with_capacity(pages.len());
	for page in pages {
		let mut value = serde_json::from_str::<Value>(&page.content)?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected cached page shape"))?;
		object.insert("index".into(), json!(page.idx));
		if let (Some(width), Some(height)) = (page.width, page.height) {
			object.insert("width".into(), json!(width));
			object.insert("height".into(), json!(height));
		}
		out.push(value);
	}
	Ok(Json(json!(out)))
}

/// Legacy owner-scoped setting keys: stored under user.1 in the DB and exposed as bare keys through the API;
/// the library.* values reference the admin's category ids. New keys are added on both read and write sides together
pub(crate) const USER_SCOPED_SETTING_KEYS: &[&str] = &[
	"incognito.enabled",
	"library.default_category",
	"library.show_uncategorized",
	"library.excluded_update_categories",
	"library.excluded_download_categories",
];

pub(crate) fn user_setting_key(user_id: i64, key: &str) -> String {
	format!("user.{user_id}.{key}")
}

async fn get_settings(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<server_settings::ServerSettings>> {
	let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM setting ORDER BY key")
		.fetch_all(&state.pool)
		.await?;
	let mut map = serde_json::Map::new();
	let own_prefix = format!("user.{}.", user.id);
	for (key, raw) in rows {
		if key == "sync.instance_id" {
			continue;
		}
		if key.starts_with("user.") {
			if let Some(scoped) = key.strip_prefix(&own_prefix)
				&& USER_SCOPED_SETTING_KEYS.contains(&scoped)
			{
				map.insert(scoped.into(), serde_json::from_str(&raw)?);
			}
			continue;
		}
		map.insert(key, serde_json::from_str(&raw)?);
	}
	let library = crate::update::LibraryUpdateSettings::load(&state.pool, user.id, state.config.update_interval_hours).await?;
	map.insert(crate::update::LIBRARY_SKIP_TITLES.into(), json!(library.skip_titles));
	map.insert(crate::update::LIBRARY_REFRESH_METADATA.into(), json!(library.refresh_metadata));
	map.insert(
		crate::update::LIBRARY_UPDATE_INTERVAL_HOURS.into(),
		json!(library.update_interval_hours),
	);
	map.insert(
		crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES.into(),
		json!(library.excluded_update_categories),
	);
	let download = crate::download::DownloadSettings::load(&state.pool, user.id, &state.config).await?;
	map.insert(crate::download::AUTO_DOWNLOAD_NEW_CHAPTERS.into(), json!(download.auto_download_new_chapters));
	map.insert(crate::download::AUTO_DOWNLOAD_LIMIT.into(), json!(download.auto_download_limit));
	map.insert(crate::download::DOWNLOAD_FORMAT.into(), json!(download.download_format));
	map.insert(crate::download::DOWNLOAD_CONCURRENCY.into(), json!(download.download_concurrency));
	map.insert(crate::download::DELETE_AFTER_MANUAL_READ.into(), json!(download.delete_after_manual_read));
	map.insert(crate::download::DELETE_AFTER_READ.into(), json!(download.delete_after_read));
	map.insert(crate::download::ALLOW_DELETE_BOOKMARKED.into(), json!(download.allow_delete_bookmarked));
	map.insert(crate::download::AUTO_SKIP_MANGA_WITH_UNREAD.into(), json!(download.auto_skip_manga_with_unread));
	map.insert(crate::download::AUTO_IGNORE_REUPLOADS.into(), json!(download.auto_ignore_reuploads));
	map.insert(crate::download::DOWNLOAD_AHEAD.into(), json!(download.download_ahead));
	map.insert(crate::download::DOWNLOAD_AHEAD_COUNT.into(), json!(download.download_ahead_count));
	map.insert(
		crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES.into(),
		json!(download.excluded_download_categories),
	);
	server_settings::ServerSettings::from_flat(&map).map(Json)
}

/// Validate and write a flat settings map. Shared by GET /settings and the backward-compatible
/// flat-map variant of PUT /settings.
pub(crate) async fn apply_settings(
	state: &AppState,
	user: &crate::auth::AuthUser,
	body: &HashMap<String, Value>,
) -> ApiResult<()> {
	let mut tx = crate::db::begin_write(&state.pool).await?;
	for (key, value) in body {
		if value.is_null() && matches!(key.as_str(), "local.oneshots_dir" | "backup.dir") {
			sqlx::query("DELETE FROM setting WHERE key = ?")
				.bind(key)
				.execute(&mut *tx)
				.await?;
			continue;
		}
		if let Some(result) = crate::update::validate_library_setting(key, value) {
			result?;
		}
		if let Some(result) = crate::download::validate_download_setting(key, value) {
			result?;
		}
		// Read-only and instance-owned keys must not be overwritten through the public settings surface.
		if key == "localDirs"
			|| key == "localOneshotsDir"
			|| key == "sync.instance_id"
			|| key == crate::update::LIBRARY_LAST_SCHEDULED_RUN_AT
		{
			continue;
		}
		// local.dirs drives the local source's scan roots (state.local_dirs), so its shape is validated
		// rather than stored raw like the other keys; an empty array clears the override
		if key == "local.dirs"
			&& !value
				.as_array()
				.is_some_and(|dirs| dirs.iter().all(|dir| dir.as_str().is_some_and(|s| !s.trim().is_empty())))
		{
			return Err(ApiError::bad_request("local.dirs must be an array of directory paths"));
		}
		// local.oneshots_dir marks oneshots directories (state.local_oneshots_dir); a string, empty clears the override
		if key == "local.oneshots_dir" && !value.is_string() {
			return Err(ApiError::bad_request("local.oneshots_dir must be a string"));
		}
		// library.default_category names the library page's initial chip: "all" / "uncategorized" / a category id
		// in string form; a since-deleted id is pruned client-side, so any positive id stores
		if key == "library.default_category"
			&& !value.as_str().is_some_and(|v| {
				v == "all" || v == "uncategorized" || v.parse::<i64>().is_ok_and(|id| id > 0)
			})
		{
			return Err(ApiError::bad_request(
				"library.default_category must be \"all\", \"uncategorized\" or a category id",
			));
		}
		if key == "library.show_uncategorized" && !value.is_boolean() {
			return Err(ApiError::bad_request("library.show_uncategorized must be a boolean"));
		}
		// Namespace keys are generated server-side from the request identity; direct writes to user.* are never accepted
		if key.starts_with("user.") {
			continue;
		}
		let stored_key = match USER_SCOPED_SETTING_KEYS.contains(&key.as_str()) {
			true => user_setting_key(user.id, key),
			false => key.clone(),
		};
		sqlx::query(
			"INSERT INTO setting (key, value) VALUES (?, ?)
			 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		)
		.bind(&stored_key)
		.bind(serde_json::to_string(value)?)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	Ok(())
}

async fn put_settings(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(mut body): Json<Value>,
) -> ApiResult<Json<server_settings::ServerSettings>> {
	// The local directories moved to the per-instance source settings (GET/PUT /sources/{id}/settings):
	// a structured body still carrying the old `local` group fails loudly instead of silently doing nothing
	if body.as_object().is_some_and(|map| map.contains_key("local")) {
		return Err(ApiError::bad_request(
			"local directories are now per-source: use the local source's own settings",
		));
	}
	// readOnly was always server-injected; a stale client's full-object write simply drops it
	if let Some(map) = body.as_object_mut() {
		map.remove("readOnly");
	}
	let flat = match serde_json::from_value::<server_settings::ServerSettings>(body.clone()) {
		Ok(settings) => settings.into_flat(),
		Err(e)
			if body.as_object().is_some_and(|map| {
				server_settings::ROOT_GROUPS.iter().any(|key| map.contains_key(*key))
			}) =>
		{
			return Err(ApiError::bad_request(format!("invalid settings body: {e}")));
		}
		Err(_) => {
			// Fallback: accept the legacy flat dotted-key map for backward compatibility.
			let map: HashMap<String, Value> = serde_json::from_value(body)
				.map_err(|e| ApiError::bad_request(format!("invalid settings body: {e}")))?;
			map
		}
	};
	apply_settings(&state, &user, &flat).await?;
	get_settings(State(state), Extension(user)).await
}

#[cfg(test)]
mod tests {
	use super::chapter_order_by;

	#[test]
	fn chapter_order_by_keys_and_directions() {
		assert_eq!(chapter_order_by(None, None).unwrap(), " ORDER BY c.source_order ASC, c.id ASC");
		assert_eq!(chapter_order_by(Some("source"), Some("asc")).unwrap(), " ORDER BY c.source_order ASC, c.id ASC");
		assert_eq!(chapter_order_by(Some("source"), Some("desc")).unwrap(), " ORDER BY c.source_order DESC, c.id ASC");
		assert_eq!(chapter_order_by(Some("chapter"), None).unwrap(), " ORDER BY c.number DESC, c.source_order ASC, c.id ASC");
		assert_eq!(chapter_order_by(Some("chapter"), Some("asc")).unwrap(), " ORDER BY c.number ASC, c.source_order ASC, c.id ASC");
		assert_eq!(chapter_order_by(Some("date"), None).unwrap(), " ORDER BY c.date_uploaded DESC, c.source_order ASC, c.id ASC");
		assert_eq!(chapter_order_by(Some("date"), Some("asc")).unwrap(), " ORDER BY c.date_uploaded ASC, c.source_order ASC, c.id ASC");
	}

	#[test]
	fn chapter_order_by_rejects_unknown_values() {
		assert!(chapter_order_by(Some("title"), None).is_err());
		assert!(chapter_order_by(None, Some("up")).is_err());
	}
}
