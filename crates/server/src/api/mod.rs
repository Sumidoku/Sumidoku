pub mod aidoku_backup;
pub mod backup;
pub mod download;
pub mod import_merge;
pub mod library;
pub mod login;
pub mod migrate;
pub mod oauth;
pub mod page;
pub mod partial;
pub mod pins;
pub mod reader;
pub mod repo;
pub mod settings;
pub mod suwayomi_backup;
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
use axum::Router;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn router(state: Arc<AppState>) -> Router {
	// 公开面：健康检查 + 认证自身（登录都进不来就死锁了）
	let public = Router::new()
		.route("/health", get(health))
		.route("/api/v1/auth/status", get(crate::auth::status))
		.route("/api/v1/auth/login", post(crate::auth::login))
		.route("/api/v1/auth/logout", post(crate::auth::logout));
	let protected = Router::new()
		.route("/api/v1/sources", get(list_sources))
		.route("/api/v1/sources/scan", post(scan_sources))
		.route("/api/v1/sources/reorder-pinned", post(pins::reorder_pinned))
		.route("/api/v1/sources/{id}", delete(delete_source).patch(pins::patch_source))
		.route("/api/v1/sources/{id}/update", post(repo::update_source))
		.route("/api/v1/sources/{id}/icon", get(get_source_icon))
		.route("/api/v1/sources/{id}/image", get(cover::get_source_image))
		.route("/api/v1/sources/{id}/filters", get(get_filters))
		.route("/api/v1/sources/{id}/search", post(search_source))
		.route("/api/v1/sources/{id}/listings", get(get_listings))
		.route("/api/v1/sources/{id}/listings/{listing_id}", get(get_manga_list))
		.route("/api/v1/sources/{id}/home", get(get_home))
		.route("/api/v1/sources/{id}/deep-link", post(deep_link))
		.route("/api/v1/sources/{id}/settings", get(get_source_settings).put(put_source_settings))
		.route("/api/v1/sources/{id}/settings/reset", post(reset_source_settings))
		.route("/api/v1/sources/{id}/notify", post(notify_source))
		.route("/api/v1/sources/{id}/cache/clear", post(clear_source_cache))
		.route("/api/v1/sources/{id}/login/basic", post(basic_login))
		.route("/api/v1/sources/{id}/login/web", post(login::web_login))
		.route("/api/v1/sources/{id}/login/web/browser", post(login::browser_login_start))
		.route(
			"/api/v1/sources/{id}/login/web/browser/complete",
			post(login::browser_login_complete),
		)
		.route(
			"/api/v1/sources/{id}/login/web/browser/cancel",
			post(login::browser_login_cancel),
		)
		.route(
			"/api/v1/sources/{id}/login/web/browser/{session}/stream",
			get(login::browser_login_stream),
		)
		.route("/api/v1/sources/{id}/login/oauth/start", post(oauth::oauth_start))
		.route("/api/v1/sources/{id}/login/oauth/complete", post(oauth::oauth_complete))
		.route("/api/v1/sources/{id}/login/logout", post(login::logout))
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
		.route("/api/v1/library/{manga_id}", delete(library::remove_from_library))
		.route("/api/v1/migrate/sources", get(migrate::migrate_sources))
		.route("/api/v1/migrate/lookup", get(migrate::migrate_lookup))
		.route("/api/v1/migrate", post(migrate::migrate_execute))
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
		.route(
			"/api/v1/backup",
			get(backup::export_backup)
				.post(backup::import_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route("/api/v1/backups", get(backup::list_backups))
		.route("/api/v1/backups/restore", post(backup::restore_backup))
		.route(
			"/api/v1/import/aidoku",
			post(aidoku_backup::import_aidoku_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route(
			"/api/v1/import/suwayomi",
			post(suwayomi_backup::import_suwayomi_backup)
				.route_layer(axum::extract::DefaultBodyLimit::max(backup::BODY_MAX)),
		)
		.route(
			"/api/v1/source-repos",
			get(repo::list_repos).post(repo::add_repo),
		)
		.route("/api/v1/source-repos/{id}", delete(repo::delete_repo))
		.route("/api/v1/source-repos/{id}/refresh", post(repo::refresh_repo))
		.route("/api/v1/source-repos/sources", get(repo::browse_sources))
		.route("/api/v1/source-repos/install", post(repo::install_source))
		.route("/api/v1/source-repos/update-all", post(repo::update_all))
		.route("/api/v1/auth/password", put(crate::auth::change_password))
		.layer(axum::middleware::from_fn_with_state(
			state.clone(),
			crate::auth::require_auth,
		));
	public.merge(protected).with_state(state)
}

async fn health() -> Json<Value> {
	Json(json!({"status": "ok"}))
}

pub(crate) async fn source_row(state: &AppState, id: i64) -> ApiResult<SourceRow> {
	let row: Option<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, breaking_change_version, icon_path,
		 wasm_path, enabled, installed_at, updated_at FROM source WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(&state.pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("source not found"))
}

async fn list_sources(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, breaking_change_version, icon_path,
		 wasm_path, enabled, installed_at, updated_at FROM source ORDER BY id",
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
	state.evict_source(id).await;
	sqlx::query("DELETE FROM source WHERE id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	pins::prune_pinned(&state.pool, &row.key).await?;
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

/// 读源包里的静态 json（source.json/settings.json/filters.json）；
/// 目录源读 wasm_path 同级文件，.aix 源从 zip 内 `Payload/` 取；缺失或解析失败都按 None 处理
async fn package_json(state: &AppState, id: i64, file: &str) -> ApiResult<Option<Value>> {
	let row = source_row(state, id).await?;
	let Some(raw) = read_package_file(&row.wasm_path, file).await else {
		return Ok(None);
	};
	Ok(serde_json::from_slice(&raw).ok())
}

/// 目录源与 .aix 源共用的静态文件读取：先找 wasm_path 同级文件，再退回 zip 内 `Payload/<file>`
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

/// 从 `.aix`（zip）读取 `Payload/` 下的条目；非 zip 或条目缺失都按 None 处理
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

/// 已入库漫画的 key → 内部 id，流式与非流式搜索共用同一份 attach 语义
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

/// 搜索/列表返回的源侧 Manga 附加 sourceId；已入库的再附内部 id
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

/// 组件数组定序即稳定标识：流式请求里 send_partial_result 的部分结果按下标增量替换，
/// 不破坏现有字段；下标由 wasm 层按上游 title 匹配语义解析（见 partial.rs）
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
	// 能力缺失要在流开始前返回 404，保持与非流式相同的状态码契约
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

/// 设置定义加载：静态 settings.json 与动态 get_settings 合并（静态在前、动态追加、
/// 同键去重，对齐 AidokuRunner 的 staticSettings + getSettings()）；动态项顺带播种
/// 默认值，对齐 Runner 的 register(defaults:)
pub(crate) async fn load_settings_items(
	state: &AppState,
	id: i64,
	handle: &aidoku_wasm::SourceHandle,
) -> ApiResult<Vec<Value>> {
	let static_items = package_json(state, id, "settings.json")
		.await?
		.map(|raw| settings::normalize_static_settings(&raw))
		.unwrap_or_default();
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

async fn get_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let handle = state.get_handle(id).await?;
	let doc = package_json(&state, id, "source.json")
		.await?
		.unwrap_or_else(|| json!({}));
	let row = source_row(&state, id).await?;
	let mut items = load_settings_items(&state, id, &handle).await?;
	// 合成设置排在静态/动态设置之前，对齐 AidokuRunner 的 extraSettings（语言在 base url 之前）；
	// 源已自行声明同名键时跳过合成项，避免列表里出现两个同键设置
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
		// webui 据此决定 web 登录项显示「在浏览器中打开登录页」还是只有粘贴区
		"capabilities": {
			"browserLogin": state.browser_logins.get().map(|m| m.configured()).unwrap_or(false),
		},
	})))
}

/// serde_urlencoded 不收重复键；notify 可重复（一次保存可能改多个带
/// notification 的设置，逐个送达），改用手工解析
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

/// 直接给在跑（或懒加载拉起）的实例送 notification，不重启——Button 设置的
/// 按下语义（Aidoku app：按下即 handleNotification，实例状态保留）
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

/// 实例在跑才热重启（换 defaults/cookies）；没跑的下一次懒加载自然吃到新值
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

/// 清空该源的 settings 覆盖值后按包内静态 schema 重新播种默认值（对齐重装语义）
async fn reset_source_settings(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	source_row(&state, id).await?;
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

/// 清该源的登录 cookie 并重启实例；不动封面缓存与下载
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

/// 基本登录：凭据交给源校验，成功后由源内 sink 落库（username/password/marker）
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
	let manga_id = find_or_register_manga(&state, body.source_id, initial).await?;
	Ok(Json(json!({ "id": manga_id })))
}

/// (source, key) 已有 manga 行就直接命中；没有则回源拉详情 + 章节登记（不入库），
/// 浏览页点开条目与深链落地共用这条路径
pub(crate) async fn find_or_register_manga(state: &Arc<AppState>, source_id: i64, initial: Manga) -> ApiResult<i64> {
	let existing: Option<(i64,)> =
		sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
			.bind(source_id)
			.bind(&initial.key)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((id,)) = existing {
		return Ok(id);
	}
	let handle = state.get_handle(source_id).await?;
	let updated = handle.get_manga_update(initial, true, true).await?;
	let mut conn = state.pool.acquire().await?;
	let manga_id = upsert_manga(&mut conn, source_id, &updated, true, true, true).await?;
	reconcile_chapters(
		&mut conn,
		manga_id,
		&updated.chapters.clone().unwrap_or_default(),
	)
	.await?;
	Ok(manga_id)
}

#[derive(Debug, Deserialize)]
pub struct DeepLinkRequest {
	pub url: String,
}

/// 把 URL 交给源的 handle_deep_link 并解析成前端可直接导航的目标：Manga/Chapter
/// 变体按 (source, key) 登记/命中 manga 行后回内部 id——chapter key 不在章节列表里
/// 时 chapterId 为 null，前端落到 manga 页；Listing 原样回传，由前端切到该列表；
/// 源不认这个 URL 时 result 为 null
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

pub(crate) fn wants_refresh(query: &RefreshQuery) -> bool {
	matches!(query.refresh.as_deref(), Some("") | Some("1") | Some("true"))
}

pub(crate) async fn manga_json(state: &AppState, row: &MangaRow) -> ApiResult<Value> {
	let (chapter_count,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(row.id)
			.fetch_one(&state.pool)
			.await?;
	let manga_user: Option<(bool, i64, bool)> =
		sqlx::query_as("SELECT in_library, viewer_flags, incognito FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(row.id)
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
		sqlx::query_scalar("SELECT category_id FROM category_manga WHERE manga_id = ? AND user_id = 1")
			.bind(row.id)
			.fetch_all(&state.pool)
			.await?;
	object.insert("categoryIds".into(), json!(category_ids));
	Ok(value)
}

/// viewer_flags 非零即用户阅读方向覆盖：响应 viewer 用覆盖值，库里的源值不动
fn apply_viewer_override(object: &mut serde_json::Map<String, Value>, viewer_flags: i64) -> ApiResult<()> {
	if viewer_flags != 0 {
		object.insert("viewer".into(), serde_json::to_value(viewer_from_int(viewer_flags))?);
	}
	Ok(())
}

/// 列表端点的批量版 manga_json：三条 IN 查询替代逐行三查询，输出与入参同序；
/// 缺行按单查缺省（0 / false / 空数组）补齐
pub(crate) async fn manga_json_batch(state: &AppState, rows: &[&MangaRow]) -> ApiResult<Vec<Value>> {
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
		"SELECT manga_id, in_library, viewer_flags, incognito FROM manga_user WHERE user_id = 1 AND manga_id IN ({placeholders})"
	);
	let mut q = sqlx::query_as::<_, (i64, bool, i64, bool)>(&library_sql);
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
		"SELECT manga_id, category_id FROM category_manga WHERE user_id = 1 AND manga_id IN ({placeholders})"
	);
	let mut q = sqlx::query_as::<_, (i64, i64)>(&category_sql);
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

/// history/update 的漫画列：批量取行再批量序列化，输出与 ids 同序；缺行同 manga_row 的 404
pub(crate) async fn manga_json_by_ids(state: &AppState, ids: &[i64]) -> ApiResult<Vec<Value>> {
	let rows = manga_rows_by_ids(&state.pool, ids).await?;
	let refs: Vec<&MangaRow> = ids
		.iter()
		.map(|id| rows.get(id))
		.collect::<Option<Vec<_>>>()
		.ok_or_else(|| ApiError::not_found("manga not found"))?;
	manga_json_batch(state, &refs).await
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
		let mut conn = state.pool.acquire().await?;
		upsert_manga(&mut conn, row.source_id, &updated, true, true, false).await?;
		row = manga_row(&state.pool, id).await?;
	}
	Ok(Json(manga_json(&state, &row).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMangaRequest {
	pub update_strategy: Option<UpdateStrategy>,
	pub incognito: Option<bool>,
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
	if let Some(incognito) = patch.incognito {
		// manga_user 行可能不存在（未入库）：单本隐身不依赖入库状态，upsert 建行
		sqlx::query(
			"INSERT INTO manga_user (manga_id, user_id, incognito) VALUES (?, 1, ?)
			 ON CONFLICT (manga_id, user_id) DO UPDATE SET incognito = excluded.incognito",
		)
		.bind(id)
		.bind(incognito)
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
		let mut conn = state.pool.acquire().await?;
		reconcile_chapters(&mut conn, id, &updated.chapters.clone().unwrap_or_default()).await?;
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(crate::model::unix_now())
			.bind(id)
			.execute(&mut *conn)
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
