pub mod download;
pub mod library;
pub mod page;
pub mod reader;
pub mod update;

use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterProgress, ChapterRow, ChapterUserRow, MangaRow, SourceRow, chapter_from_row,
	default_value_to_json, json_to_default_value, manga_from_row,
};
use crate::source::cover;
use crate::source::{
	chapter_row, load_pages, manga_row, reconcile_chapters, scan_sources_dir, upsert_manga,
};
use crate::state::{AppState, load_defaults};
use aidoku::{FilterValue, Listing, Manga, UpdateStrategy};
use aidoku_wasm::Error as SourceError;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Json, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
	Router::new()
		.route("/health", get(health))
		.route("/api/v1/sources", get(list_sources))
		.route("/api/v1/sources/scan", post(scan_sources))
		.route("/api/v1/sources/{id}", delete(delete_source))
		.route("/api/v1/sources/{id}/icon", get(get_source_icon))
		.route("/api/v1/sources/{id}/filters", get(get_filters))
		.route("/api/v1/sources/{id}/search", post(search_source))
		.route("/api/v1/sources/{id}/listings", get(get_listings))
		.route("/api/v1/sources/{id}/listings/{listing_id}", get(get_manga_list))
		.route("/api/v1/sources/{id}/settings", get(get_source_settings).put(put_source_settings))
		.route("/api/v1/manga", post(register_manga))
		.route("/api/v1/manga/{id}", get(get_manga).patch(update_manga))
		.route("/api/v1/manga/{id}/chapters", get(get_chapters))
		.route("/api/v1/manga/{id}/cover", get(cover::get_cover))
		.route("/api/v1/manga/{id}/categories", put(library::set_manga_categories))
		.route("/api/v1/chapter/{id}", patch(reader::update_chapter))
		.route("/api/v1/chapter/batch", post(reader::batch_update_chapters))
		.route("/api/v1/chapter/{id}/pages", get(get_pages))
		.route("/api/v1/chapter/{id}/page/{index}", get(page::get_page))
		.route("/api/v1/library", get(library::list_library).post(library::add_to_library))
		.route("/api/v1/library/{manga_id}", delete(library::remove_from_library))
		.route("/api/v1/categories", get(library::list_categories).post(library::create_category))
		.route(
			"/api/v1/categories/{id}",
			patch(library::update_category).delete(library::delete_category),
		)
		.route("/api/v1/categories/reorder", patch(library::reorder_categories))
		.route("/api/v1/history", get(reader::get_history))
		.route(
			"/api/v1/downloads",
			get(crate::api::download::list_downloads).post(crate::api::download::enqueue_downloads),
		)
		.route("/api/v1/downloads/start", post(crate::api::download::start_downloads))
		.route("/api/v1/downloads/stop", post(crate::api::download::stop_downloads))
		.route("/api/v1/downloads/clear", post(crate::api::download::clear_downloads))
		.route("/api/v1/downloads/ws", get(crate::api::download::downloads_ws))
		.route(
			"/api/v1/downloads/{chapter_id}",
			delete(crate::api::download::delete_download),
		)
		.route("/api/v1/chapter/{id}/file", get(crate::api::download::get_chapter_file))
		.route("/api/v1/update/fetch", post(crate::api::update::fetch_updates))
		.route("/api/v1/update/summary", get(crate::api::update::get_summary))
		.route("/api/v1/update/recent", get(crate::api::update::get_recent))
		.route("/api/v1/update/ws", get(crate::api::update::updates_ws))
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
	let outcome = scan_sources_dir(&state.pool, &state.config.sources_dir()).await?;
	Ok(Json(serde_json::to_value(outcome)?))
}

async fn delete_source(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
	source_row(&state, id).await?;
	let entry = state.sources.write().await.remove(&id);
	if let Some(entry) = entry {
		// 有在途调用持有 Arc 时，让位给引用计数归零后 actor 线程自然退出
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

/// 读源包目录里的静态 json；文件缺失或解析失败都按 None 处理
async fn package_json(state: &AppState, id: i64, file: &str) -> ApiResult<Option<Value>> {
	let row = source_row(state, id).await?;
	let Some(dir) = std::path::Path::new(&row.wasm_path).parent() else {
		return Ok(None);
	};
	let Ok(raw) = tokio::fs::read(dir.join(file)).await else {
		return Ok(None);
	};
	Ok(serde_json::from_slice(&raw).ok())
}

async fn get_filters(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	// 对齐 AidokuRunner：静态筛选器在前，动态导出结果追加在后
	let mut out: Vec<Value> = Vec::new();
	if let Some(doc) = package_json(&state, id, "source.json").await?
		&& let Some(filters) = doc.get("filters").and_then(|v| v.as_array())
	{
		out.extend(filters.iter().cloned());
	}
	if out.is_empty()
		&& let Some(Value::Array(filters)) = package_json(&state, id, "filters.json").await?
	{
		out.extend(filters);
	}
	match handle.get_filters().await {
		Ok(filters) => {
			if let Value::Array(items) = serde_json::to_value(filters)? {
				out.extend(items);
			}
			Ok(Json(Value::Array(out)))
		}
		Err(e @ (SourceError::Unimplemented | SourceError::Unsupported(_))) => {
			if out.is_empty() {
				Err(dynamic_capability_error(e))
			} else {
				Ok(Json(Value::Array(out)))
			}
		}
		Err(e) => Err(ApiError::from(e)),
	}
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
	// 对齐 AidokuRunner：静态 listings 在前，动态导出结果追加在后
	let mut out: Vec<Value> = Vec::new();
	if let Some(doc) = package_json(&state, id, "source.json").await?
		&& let Some(listings) = doc.get("listings").and_then(|v| v.as_array())
	{
		out.extend(listings.iter().cloned());
	}
	match handle.get_listings().await {
		Ok(listings) => {
			if let Value::Array(items) = serde_json::to_value(listings)? {
				out.extend(items);
			}
			Ok(Json(Value::Array(out)))
		}
		Err(e @ (SourceError::Unimplemented | SourceError::Unsupported(_))) => {
			if out.is_empty() {
				Err(dynamic_capability_error(e))
			} else {
				Ok(Json(Value::Array(out)))
			}
		}
		Err(e) => Err(ApiError::from(e)),
	}
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
		Err(SourceError::Unimplemented | SourceError::Unsupported(_)) => {
			package_json(&state, id, "settings.json").await?
		}
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
		if let Some(notify) = query.notify
			&& features.notification
		{
			handle.handle_notification(notify).await?;
		}
	}
	let values = current_values(&state, id).await?;
	Ok(Json(json!({ "values": values })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMangaRequest {
	pub source_id: i64,
	#[serde(default)]
	pub manga_key: String,
	/// 客户端手里的搜索条目；有它就以它为 get_manga_update 入参，
	/// 源没改动的字段（典型是 cover）才不会丢
	pub manga: Option<Manga>,
}

async fn register_manga(
	State(state): State<Arc<AppState>>,
	Json(body): Json<RegisterMangaRequest>,
) -> ApiResult<Json<Value>> {
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
	let existing: Option<(i64,)> =
		sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
			.bind(body.source_id)
			.bind(&initial.key)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((id,)) = existing {
		return Ok(Json(json!({ "id": id })));
	}
	let handle = state.get_handle(body.source_id).await?;
	let updated = handle.get_manga_update(initial, true, true).await?;
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

pub(crate) fn wants_refresh(query: &RefreshQuery) -> bool {
	matches!(query.refresh.as_deref(), Some("") | Some("1") | Some("true"))
}

pub(crate) async fn manga_json(state: &AppState, row: &MangaRow) -> ApiResult<Value> {
	let (chapter_count,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(row.id)
			.fetch_one(&state.pool)
			.await?;
	let in_library: Option<(bool,)> =
		sqlx::query_as("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(row.id)
			.fetch_optional(&state.pool)
			.await?;
	let mut value = serde_json::to_value(manga_from_row(row))?;
	let object = value
		.as_object_mut()
		.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
	object.insert("id".into(), json!(row.id));
	object.insert("sourceId".into(), json!(row.source_id));
	object.insert("initialized".into(), json!(row.initialized));
	object.insert("chapterCount".into(), json!(chapter_count));
	object.insert(
		"inLibrary".into(),
		json!(in_library.map(|(flag,)| flag).unwrap_or(false)),
	);
	let category_ids: Vec<i64> =
		sqlx::query_scalar("SELECT category_id FROM category_manga WHERE manga_id = ? AND user_id = 1")
			.bind(row.id)
			.fetch_all(&state.pool)
			.await?;
	object.insert("categoryIds".into(), json!(category_ids));
	Ok(value)
}

/// f32 经 serde_json 会先转 f64 再按 f64 最短表示打印（281.1 变 281.1000061035156），
/// 用 f32 自己的最短表示还原成干净小数
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
	object.insert("pageCount".into(), json!(row.page_count));
	object.insert("lastPageRead".into(), json!(progress.last_page_read));
	object.insert("isRead".into(), json!(progress.is_read));
	object.insert("isBookmarked".into(), json!(progress.is_bookmarked));
	object.insert("lastReadAt".into(), json!(progress.last_read_at));
	Ok(value)
}

pub(crate) fn progress_of(row: &ChapterUserRow) -> Option<ChapterProgress> {
	// last_page_read 列非空，LEFT JOIN 出 NULL 即用户行不存在
	row.last_page_read.map(|last_page_read| ChapterProgress {
		last_page_read,
		is_read: row.is_read.unwrap_or(false),
		is_bookmarked: row.is_bookmarked.unwrap_or(false),
		last_read_at: row.last_read_at,
	})
}

async fn get_manga(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	let mut row = manga_row(&state.pool, id).await?;
	if wants_refresh(&query) {
		let handle = state.get_handle(row.source_id).await?;
		let updated = handle
			.get_manga_update(manga_from_row(&row), true, false)
			.await?;
		upsert_manga(&state.pool, row.source_id, &updated, true, true, false).await?;
		row = manga_row(&state.pool, id).await?;
	}
	Ok(Json(manga_json(&state, &row).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMangaRequest {
	pub update_strategy: Option<UpdateStrategy>,
}

/// 手动解析 body：axum 的 Json 拒绝是 422，端点约定非法值 400；空 body 视为无操作
async fn update_manga(
	State(state): State<Arc<AppState>>,
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
	let row = manga_row(&state.pool, id).await?;
	Ok(Json(manga_json(&state, &row).await?))
}

async fn get_chapters(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, id).await?;
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
	let rows: Vec<ChapterUserRow> = sqlx::query_as(
		"SELECT c.*, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter c
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 WHERE c.manga_id = ? ORDER BY c.source_order ASC",
	)
	.bind(id)
	.fetch_all(&state.pool)
	.await?;
	let downloaded: HashSet<i64> = sqlx::query_scalar(
		"SELECT chapter_id FROM chapter_user WHERE user_id = 1 AND is_downloaded = 1
		 AND chapter_id IN (SELECT id FROM chapter WHERE manga_id = ?)",
	)
	.bind(id)
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
	Ok(Json(json!(chapters)))
}

async fn get_pages(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Json<Value>> {
	chapter_row(&state.pool, id).await?;
	let pages = load_pages(&state, id, wants_refresh(&query)).await?;
	let mut out = Vec::with_capacity(pages.len());
	for (index, content) in pages {
		let mut value = serde_json::from_str::<Value>(&content)?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected cached page shape"))?;
		object.insert("index".into(), json!(index));
		out.push(value);
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
