use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterRow, MangaRow, SourceRow, chapter_from_row, default_value_to_json,
	json_to_default_value, manga_from_row,
};
use crate::service::{reconcile_chapters, scan_sources_dir, upsert_manga};
use crate::state::{AppState, load_defaults};
use aidoku::{FilterValue, Listing, Manga};
use aidoku_wasm::Error as SourceError;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
	Router::new()
		.route("/health", get(health))
		.route("/api/v1/sources", get(list_sources))
		.route("/api/v1/sources/scan", post(scan_sources))
		.route("/api/v1/sources/{id}", delete(delete_source))
		.route("/api/v1/sources/{id}/filters", get(get_filters))
		.route("/api/v1/sources/{id}/search", post(search_source))
		.route("/api/v1/sources/{id}/listings", get(get_listings))
		.route("/api/v1/sources/{id}/listings/{listing_id}", get(get_manga_list))
		.route("/api/v1/sources/{id}/settings", get(get_source_settings).put(put_source_settings))
		.route("/api/v1/manga", post(register_manga))
		.route("/api/v1/manga/{id}", get(get_manga))
		.route("/api/v1/manga/{id}/chapters", get(get_chapters))
		.route("/api/v1/chapter/{id}/pages", get(get_pages))
		.route("/api/v1/settings", get(get_settings).put(put_settings))
		.with_state(state)
}

async fn health() -> Json<Value> {
	Json(json!({"status": "ok"}))
}

async fn source_row(state: &AppState, id: i64) -> ApiResult<SourceRow> {
	let row: Option<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, icon_path,
		 wasm_path, enabled, installed_at, updated_at FROM source WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("source not found"))
}

async fn manga_row(state: &AppState, id: i64) -> ApiResult<MangaRow> {
	let row: Option<MangaRow> = sqlx::query_as(
		"SELECT id, source_id, key, title, authors, artists, description, tags, status,
		 content_rating, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at FROM manga WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("manga not found"))
}

async fn chapter_row(state: &AppState, id: i64) -> ApiResult<ChapterRow> {
	let row: Option<ChapterRow> = sqlx::query_as(
		"SELECT id, manga_id, key, title, number, volume, scanlators, language, url,
		 date_uploaded, source_order, page_count, locked, fetched_at FROM chapter WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("chapter not found"))
}

async fn list_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, icon_path,
		 wasm_path, enabled, installed_at, updated_at FROM source ORDER BY id",
	)
	.fetch_all(&state.pool)
	.await?;
	let sources: Vec<Value> = rows
		.iter()
		.map(|row| {
			json!({
				"id": row.id,
				"key": row.key,
				"name": row.name,
				"version": row.version,
				"languages": row.languages.0,
				"contentRating": row.content_rating,
				"baseUrl": row.base_url,
				"enabled": row.enabled,
				"installedAt": row.installed_at,
				"updatedAt": row.updated_at,
			})
		})
		.collect();
	Ok(Json(json!(sources)))
}

async fn scan_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let outcome = scan_sources_dir(&state.pool, &state.config.sources_dir).await?;
	Ok(Json(serde_json::to_value(outcome)?))
}

async fn delete_source(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
	source_row(&state, id).await?;
	let entry = state.sources.write().await.remove(&id);
	if let Some(entry) = entry {
		// 有在途调用持有 Arc 时shutdown 让位给引用计数归零自然退出
		if let Ok(handle) = Arc::try_unwrap(entry.handle) {
			handle.shutdown().await;
		}
	}
	sqlx::query("DELETE FROM source WHERE id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	Ok(StatusCode::NO_CONTENT)
}

/// 动态能力（filters/listings）不支持时按端点约定 404 unsupported
fn dynamic_capability_error(e: SourceError) -> ApiError {
	match e {
		SourceError::Unimplemented => ApiError::unsupported_404("feature not supported by source"),
		SourceError::Unsupported(name) => {
			ApiError::unsupported_404(format!("feature not supported by source: {name}"))
		}
		other => ApiError::from(other),
	}
}

async fn get_filters(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	let filters = handle.get_filters().await.map_err(dynamic_capability_error)?;
	Ok(Json(serde_json::to_value(filters)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
	pub query: Option<String>,
	pub page: Option<i32>,
	pub filters: Option<Vec<FilterValue>>,
}

/// 搜索/列表返回的源侧 Manga 附加 sourceId；已入库的再附内部 id
async fn attach_internal_ids(
	state: &AppState,
	source_id: i64,
	entries: Vec<Manga>,
) -> ApiResult<Vec<Value>> {
	let known: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_all(&state.pool)
		.await?;
	let known: HashMap<String, i64> = known.into_iter().map(|(id, key)| (key, id)).collect();
	let mut out = Vec::with_capacity(entries.len());
	for manga in entries {
		let mut value = serde_json::to_value(&manga)?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
		object.insert("sourceId".into(), json!(source_id));
		if let Some(id) = known.get(&manga.key) {
			object.insert("id".into(), json!(id));
		}
		out.push(value);
	}
	Ok(out)
}

async fn search_source(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<SearchRequest>,
) -> ApiResult<Json<Value>> {
	let page = body.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let handle = state.get_handle(id).await?;
	let result = handle
		.get_search_manga_list(body.query, page, body.filters.unwrap_or_default())
		.await?;
	let entries = attach_internal_ids(&state, id, result.entries).await?;
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": result.has_next_page,
	})))
}

async fn get_listings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	let listings = handle.get_listings().await.map_err(dynamic_capability_error)?;
	Ok(Json(serde_json::to_value(listings)?))
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
	pub page: Option<i32>,
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
	let listing = Listing {
		id: listing_id,
		name: String::new(),
		kind: aidoku::ListingKind::Default,
	};
	let result = handle.get_manga_list(listing, page).await?;
	let entries = attach_internal_ids(&state, id, result.entries).await?;
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": result.has_next_page,
	})))
}

async fn current_values(state: &AppState, source_id: i64) -> ApiResult<Value> {
	let defaults = load_defaults(&state.pool, source_id).await?;
	let values: serde_json::Map<String, Value> = defaults
		.iter()
		.map(|(key, value)| (key.clone(), default_value_to_json(value)))
		.collect();
	Ok(Value::Object(values))
}

async fn get_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	let settings = match handle.get_settings().await {
		Ok(settings) => Some(serde_json::to_value(settings)?),
		Err(SourceError::Unimplemented | SourceError::Unsupported(_)) => None,
		Err(e) => return Err(ApiError::from(e)),
	};
	let values = current_values(&state, id).await?;
	Ok(Json(json!({
		"settings": settings,
		"values": values,
	})))
}

#[derive(Debug, Deserialize)]
pub struct NotifyQuery {
	pub notify: Option<String>,
}

async fn put_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<NotifyQuery>,
	Json(body): Json<HashMap<String, Value>>,
) -> ApiResult<Json<Value>> {
	source_row(&state, id).await?;
	{
		let mut tx = state.pool.begin().await?;
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
	// 实例在跑才热重启；没跑的下一次懒加载自然吃到新 defaults
	let handle = state
		.sources
		.read()
		.await
		.get(&id)
		.map(|entry| entry.handle.clone());
	if let Some(handle) = handle {
		let defaults = load_defaults(&state.pool, id).await?;
		let features = handle.restart(defaults).await?;
		if let Some(entry) = state.sources.write().await.get_mut(&id) {
			entry.features = features.clone();
		}
		if let Some(notify) = query.notify {
			if features.notification {
				handle.handle_notification(notify).await?;
			}
		}
	}
	let values = current_values(&state, id).await?;
	Ok(Json(json!({ "values": values })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMangaRequest {
	pub source_id: i64,
	pub manga_key: String,
}

async fn register_manga(
	State(state): State<Arc<AppState>>,
	Json(body): Json<RegisterMangaRequest>,
) -> ApiResult<Json<Value>> {
	if body.manga_key.is_empty() {
		return Err(ApiError::bad_request("mangaKey must not be empty"));
	}
	source_row(&state, body.source_id).await?;
	let existing: Option<(i64,)> =
		sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
			.bind(body.source_id)
			.bind(&body.manga_key)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((id,)) = existing {
		return Ok(Json(json!({ "id": id })));
	}
	let handle = state.get_handle(body.source_id).await?;
	let placeholder = Manga {
		key: body.manga_key.clone(),
		..Manga::default()
	};
	let updated = handle.get_manga_update(placeholder, true, true).await?;
	let manga_id = upsert_manga(&state.pool, body.source_id, &updated, true, true, true).await?;
	reconcile_chapters(
		&state.pool,
		manga_id,
		&updated.chapters.clone().unwrap_or_default(),
	)
	.await?;
	Ok(Json(json!({ "id": manga_id })))
}

#[derive(Debug, Deserialize)]
pub struct RefreshQuery {
	pub refresh: Option<String>,
}

fn wants_refresh(query: &RefreshQuery) -> bool {
	matches!(query.refresh.as_deref(), Some("1") | Some("true"))
}

async fn manga_json(state: &AppState, row: &MangaRow) -> ApiResult<Value> {
	let (chapter_count,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(row.id)
			.fetch_one(&state.pool)
			.await?;
	let mut value = serde_json::to_value(manga_from_row(row))?;
	let object = value
		.as_object_mut()
		.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
	object.insert("id".into(), json!(row.id));
	object.insert("sourceId".into(), json!(row.source_id));
	object.insert("initialized".into(), json!(row.initialized));
	object.insert("chapterCount".into(), json!(chapter_count));
	Ok(value)
}

async fn get_manga(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	let mut row = manga_row(&state, id).await?;
	if wants_refresh(&query) {
		let handle = state.get_handle(row.source_id).await?;
		let updated = handle
			.get_manga_update(manga_from_row(&row), true, false)
			.await?;
		upsert_manga(&state.pool, row.source_id, &updated, true, true, false).await?;
		row = manga_row(&state, id).await?;
	}
	Ok(Json(manga_json(&state, &row).await?))
}

async fn get_chapters(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state, id).await?;
	if wants_refresh(&query) {
		let handle = state.get_handle(manga.source_id).await?;
		let updated = handle
			.get_manga_update(manga_from_row(&manga), false, true)
			.await?;
		reconcile_chapters(&state.pool, id, &updated.chapters.clone().unwrap_or_default()).await?;
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(crate::model::unix_now())
			.bind(id)
			.execute(&state.pool)
			.await?;
	}
	let rows: Vec<ChapterRow> = sqlx::query_as(
		"SELECT id, manga_id, key, title, number, volume, scanlators, language, url,
		 date_uploaded, source_order, page_count, locked, fetched_at
		 FROM chapter WHERE manga_id = ? ORDER BY source_order ASC",
	)
	.bind(id)
	.fetch_all(&state.pool)
	.await?;
	let mut chapters = Vec::with_capacity(rows.len());
	for row in &rows {
		let mut value = serde_json::to_value(chapter_from_row(row))?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected chapter shape"))?;
		object.insert("id".into(), json!(row.id));
		object.insert("pageCount".into(), json!(row.page_count));
		chapters.push(value);
	}
	Ok(Json(json!(chapters)))
}

async fn get_pages(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	let chapter = chapter_row(&state, id).await?;
	let cached: Vec<(i64, String)> = sqlx::query_as(
		"SELECT idx, content FROM page WHERE chapter_id = ? ORDER BY idx ASC",
	)
	.bind(id)
	.fetch_all(&state.pool)
	.await?;
	if !cached.is_empty() && !wants_refresh(&query) {
		let mut pages = Vec::with_capacity(cached.len());
		for (index, content) in cached {
			pages.push(json!({
				"index": index,
				"content": serde_json::from_str::<Value>(&content)?,
				"thumbnail": Value::Null,
				"hasDescription": false,
				"description": Value::Null,
			}));
		}
		return Ok(Json(json!(pages)));
	}
	let manga = manga_row(&state, chapter.manga_id).await?;
	let handle = state.get_handle(manga.source_id).await?;
	let pages = handle
		.get_page_list(manga_from_row(&manga), chapter_from_row(&chapter))
		.await?;
	{
		let mut tx = state.pool.begin().await?;
		sqlx::query("DELETE FROM page WHERE chapter_id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		for (index, page) in pages.iter().enumerate() {
			sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, ?, ?)")
				.bind(id)
				.bind(index as i64)
				.bind(serde_json::to_string(&page.content)?)
				.execute(&mut *tx)
				.await?;
		}
		tx.commit().await?;
	}
	let mut out = Vec::with_capacity(pages.len());
	for (index, page) in pages.into_iter().enumerate() {
		out.push(json!({
			"index": index,
			"content": serde_json::to_value(&page.content)?,
			"thumbnail": page.thumbnail,
			"hasDescription": page.has_description,
			"description": page.description,
		}));
	}
	Ok(Json(json!(out)))
}

async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM setting ORDER BY key")
		.fetch_all(&state.pool)
		.await?;
	let mut map = serde_json::Map::new();
	for (key, raw) in rows {
		map.insert(key, serde_json::from_str(&raw)?);
	}
	Ok(Json(Value::Object(map)))
}

async fn put_settings(
	State(state): State<Arc<AppState>>,
	Json(body): Json<HashMap<String, Value>>,
) -> ApiResult<Json<Value>> {
	let mut tx = state.pool.begin().await?;
	for (key, value) in &body {
		sqlx::query(
			"INSERT INTO setting (key, value) VALUES (?, ?)
			 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		)
		.bind(key)
		.bind(serde_json::to_string(value)?)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	get_settings(State(state)).await
}
