use crate::api::{manga_json, manga_json_batch};
use crate::error::{ApiError, ApiResult};
use crate::model::{LibraryEntryRow, MangaRow, unix_now};
use crate::source::manga_row;
use crate::state::AppState;
use crate::{PAGE_SIZE, USER_ID};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

const LIBRARY_SELECT: &str = "SELECT m.*, mu.in_library_at FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1
	 WHERE mu.in_library = 1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
	pub category_id: Option<i64>,
	/// 迁移批量流程按源筛条目
	pub source_id: Option<i64>,
	pub page: Option<i64>,
	pub search: Option<String>,
	pub sort: Option<String>,
	pub filter: Option<String>,
	/// 逗号分隔的 manga 级分级值（1..=3），包含集
	pub content_rating: Option<String>,
	pub content_rating_exclude: Option<String>,
	/// 逗号分隔的源 id，包含集
	pub source: Option<String>,
	pub source_exclude: Option<String>,
}

/// LIKE 的 %/_ 是通配符，用户搜索词里的字面量与转义符本身都要前置转义
fn escape_like(raw: &str) -> String {
	let mut out = String::with_capacity(raw.len());
	for c in raw.chars() {
		if matches!(c, '\\' | '%' | '_') {
			out.push('\\');
		}
		out.push(c);
	}
	out
}

fn library_order_by(sort: Option<&str>) -> ApiResult<&'static str> {
	const LAST_READ: &str = " ORDER BY (SELECT MAX(cu.last_read_at) FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 WHERE c.manga_id = m.id) DESC, m.id ASC";
	const UNREAD: &str = " ORDER BY (SELECT COUNT(*) FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 WHERE c.manga_id = m.id AND cu.is_read = 0) DESC, m.id ASC";
	match sort {
		None | Some("added") => Ok(" ORDER BY mu.in_library_at DESC, m.id ASC"),
		Some("title") => Ok(" ORDER BY m.title COLLATE NOCASE ASC, m.id ASC"),
		// SQLite 里 NULL 比任何值小，DESC 天然落最后
		Some("lastRead") => Ok(LAST_READ),
		Some("unread") => Ok(UNREAD),
		Some(other) => Err(ApiError::bad_request(format!("invalid sort: {other}"))),
	}
}

fn library_filter_clause(filter: &str) -> ApiResult<String> {
	const EXISTS: &str = " AND EXISTS (SELECT 1 FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 WHERE c.manga_id = m.id AND ";
	let column = match filter {
		"unread" => "cu.is_read = 0",
		"downloaded" => "cu.is_downloaded = 1",
		"bookmarked" => "cu.is_bookmarked = 1",
		other => return Err(ApiError::bad_request(format!("invalid filter: {other}"))),
	};
	Ok(format!("{EXISTS}{column})"))
}

/// 逗号分隔的整数列表：整段空白或空段视为缺席，非空段非法即 400（同 sort/filter 风格）
fn parse_int_list(raw: &str, param: &str, valid: impl Fn(i64) -> bool) -> ApiResult<Vec<i64>> {
	let mut out = Vec::new();
	for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
		match token.parse::<i64>() {
			Ok(v) if valid(v) => out.push(v),
			_ => return Err(ApiError::bad_request(format!("invalid {param}: {token}"))),
		}
	}
	Ok(out)
}

fn in_placeholders(len: usize) -> String {
	vec!["?"; len].join(",")
}

pub async fn list_library(
	State(state): State<Arc<AppState>>,
	Query(query): Query<LibraryQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let order_by = library_order_by(query.sort.as_deref())?;
	let search = query.search.filter(|s| !s.is_empty());
	// bind 顺序与 SQL 子句一致：search → categoryId → 分级包含 → 分级排除 → 源包含 → 源排除
	let rating_in = match &query.content_rating {
		Some(raw) => parse_int_list(raw, "contentRating", |v| (1..=3).contains(&v))?,
		None => Vec::new(),
	};
	let rating_ex = match &query.content_rating_exclude {
		Some(raw) => parse_int_list(raw, "contentRatingExclude", |v| (1..=3).contains(&v))?,
		None => Vec::new(),
	};
	let source_in = match &query.source {
		Some(raw) => parse_int_list(raw, "source", |_| true)?,
		None => Vec::new(),
	};
	let source_ex = match &query.source_exclude {
		Some(raw) => parse_int_list(raw, "sourceExclude", |_| true)?,
		None => Vec::new(),
	};
	let mut sql = LIBRARY_SELECT.to_string();
	if search.is_some() {
		sql.push_str(" AND m.title LIKE '%'||?||'%' ESCAPE '\\'");
	}
	// categoryId=0 指「在库但未分类」，其余 id 指该分类下；缺省不过滤
	let mut category = None;
	match query.category_id {
		None => {}
		Some(0) => sql.push_str(
			" AND NOT EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = 1)",
		),
		Some(category_id) => {
			category_row(&state.pool, category_id).await?;
			sql.push_str(
				" AND EXISTS (SELECT 1 FROM category_manga cm
				 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND cm.category_id = ?)",
			);
			category = Some(category_id);
		}
	}
	if !rating_in.is_empty() {
		sql.push_str(&format!(" AND m.content_rating IN ({})", in_placeholders(rating_in.len())));
	}
	if !rating_ex.is_empty() {
		sql.push_str(&format!(
			" AND m.content_rating NOT IN ({})",
			in_placeholders(rating_ex.len())
		));
	}
	if !source_in.is_empty() {
		sql.push_str(&format!(" AND m.source_id IN ({})", in_placeholders(source_in.len())));
	}
	if !source_ex.is_empty() {
		sql.push_str(&format!(
			" AND m.source_id NOT IN ({})",
			in_placeholders(source_ex.len())
		));
	}
	if query.source_id.is_some() {
		sql.push_str(" AND m.source_id = ?");
	}
	if let Some(raw) = &query.filter {
		for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
			sql.push_str(&library_filter_clause(token)?);
		}
	}
	sql.push_str(order_by);
	sql.push_str(" LIMIT ? OFFSET ?");
	let mut q = sqlx::query_as::<_, LibraryEntryRow>(&sql);
	if let Some(search) = &search {
		q = q.bind(escape_like(search));
	}
	if let Some(category_id) = category {
		q = q.bind(category_id);
	}
	for v in rating_in {
		q = q.bind(v);
	}
	for v in rating_ex {
		q = q.bind(v);
	}
	for v in source_in {
		q = q.bind(v);
	}
	for v in source_ex {
		q = q.bind(v);
	}
	if let Some(source_id) = query.source_id {
		q = q.bind(source_id);
	}
	let rows = q
		.bind(PAGE_SIZE + 1)
		.bind((page - 1) * PAGE_SIZE)
		.fetch_all(&state.pool)
		.await?;
	let has_next_page = rows.len() as i64 > PAGE_SIZE;
	let page_rows: Vec<&LibraryEntryRow> = rows.iter().take(PAGE_SIZE as usize).collect();
	let mut unread: HashMap<i64, i64> = HashMap::new();
	if !page_rows.is_empty() {
		let placeholders = page_rows.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let unread_sql = format!(
			"SELECT c.manga_id, COUNT(*) FROM chapter c
			 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1 AND cu.is_read = 0
			 WHERE c.manga_id IN ({placeholders}) GROUP BY c.manga_id"
		);
		let mut q = sqlx::query_as::<_, (i64, i64)>(&unread_sql);
		for row in &page_rows {
			q = q.bind(row.manga.id);
		}
		unread = q.fetch_all(&state.pool).await?.into_iter().collect();
	}
	let manga_refs: Vec<&MangaRow> = page_rows.iter().map(|row| &row.manga).collect();
	let values = manga_json_batch(&state, &manga_refs).await?;
	let mut entries = Vec::with_capacity(page_rows.len());
	for (row, mut value) in page_rows.into_iter().zip(values) {
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
		object.insert("inLibrary".into(), json!(true));
		object.insert("inLibraryAt".into(), json!(row.in_library_at));
		object.insert(
			"unreadCount".into(),
			json!(unread.get(&row.manga.id).copied().unwrap_or(0)),
		);
		entries.push(value);
	}
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": has_next_page,
	})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddToLibraryRequest {
	pub manga_id: i64,
	pub category_ids: Option<Vec<i64>>,
}

pub async fn add_to_library(
	State(state): State<Arc<AppState>>,
	Json(body): Json<AddToLibraryRequest>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, body.manga_id).await?;
	if let Some(category_ids) = &body.category_ids {
		for category_id in category_ids {
			category_row(&state.pool, *category_id).await?;
		}
	}
	let mut tx = state.pool.begin().await?;
	// 已在库的保留首次入库时间
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, 1, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(manga.id)
	.bind(unix_now())
	.execute(&mut *tx)
	.await?;
	if let Some(category_ids) = &body.category_ids {
		sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = 1")
			.bind(manga.id)
			.execute(&mut *tx)
			.await?;
		for category_id in category_ids {
			sqlx::query(
				"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
				 VALUES (?, ?, 1)",
			)
			.bind(category_id)
			.bind(manga.id)
			.execute(&mut *tx)
			.await?;
		}
	}
	tx.commit().await?;
	Ok(Json(manga_json(&state, &manga).await?))
}

pub async fn remove_from_library(
	State(state): State<Arc<AppState>>,
	Path(manga_id): Path<i64>,
) -> ApiResult<StatusCode> {
	manga_row(&state.pool, manga_id).await?;
	let mut tx = state.pool.begin().await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, 1, 0)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
	)
	.bind(manga_id)
	.execute(&mut *tx)
	.await?;
	sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = 1")
		.bind(manga_id)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, sqlx::FromRow)]
struct CategoryRow {
	id: i64,
	name: String,
	sort_order: i64,
	is_default: bool,
	include_in_update: i64,
	include_in_download: i64,
	manga_count: i64,
}

const CATEGORY_SELECT: &str = "SELECT c.id, c.name, c.sort_order, c.is_default,
	 c.include_in_update, c.include_in_download,
	 (SELECT COUNT(*) FROM category_manga cm
	  WHERE cm.category_id = c.id AND cm.user_id = 1) AS manga_count
	 FROM category c WHERE c.user_id = 1";

fn category_json(row: &CategoryRow) -> Value {
	json!({
		"id": row.id,
		"name": row.name,
		"sortOrder": row.sort_order,
		"isDefault": row.is_default,
		"includeInUpdate": row.include_in_update,
		"includeInDownload": row.include_in_download,
		"mangaCount": row.manga_count,
	})
}

async fn category_row(pool: &SqlitePool, id: i64) -> ApiResult<CategoryRow> {
	let row: Option<CategoryRow> =
		sqlx::query_as(&format!("{CATEGORY_SELECT} AND c.id = ?"))
			.bind(id)
			.fetch_optional(pool)
			.await?;
	row.ok_or_else(|| ApiError::not_found("category not found"))
}

/// 书库内每个源的条目数（书库图源筛选下拉的计数与自动隐藏用）。
/// 含已停用源的条目——库内漫画不随源停用消失
pub async fn source_counts(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT m.source_id, COUNT(*) FROM manga m
		 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1
		 WHERE mu.in_library = 1 GROUP BY m.source_id",
	)
	.fetch_all(&state.pool)
	.await?;
	let out: Vec<Value> = rows
		.into_iter()
		.map(|(source_id, count)| json!({ "sourceId": source_id, "count": count }))
		.collect();
	Ok(Json(json!(out)))
}

pub async fn list_categories(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let rows: Vec<CategoryRow> =
		sqlx::query_as(&format!("{CATEGORY_SELECT} ORDER BY c.sort_order ASC, c.id ASC"))
			.fetch_all(&state.pool)
			.await?;
	Ok(Json(json!(rows.iter().map(category_json).collect::<Vec<_>>())))
}

#[derive(Debug, Deserialize)]
pub struct CategoryNameRequest {
	pub name: String,
}

async fn ensure_unique_name(pool: &SqlitePool, name: &str, except_id: Option<i64>) -> ApiResult<()> {
	let duplicate: Option<(i64,)> = match except_id {
		Some(id) => {
			sqlx::query_as("SELECT id FROM category WHERE user_id = 1 AND name = ? AND id != ?")
				.bind(name)
				.bind(id)
				.fetch_optional(pool)
				.await?
		}
		None => {
			sqlx::query_as("SELECT id FROM category WHERE user_id = 1 AND name = ?")
				.bind(name)
				.fetch_optional(pool)
				.await?
		}
	};
	if duplicate.is_some() {
		return Err(ApiError::duplicate("category name already exists"));
	}
	Ok(())
}

pub async fn create_category(
	State(state): State<Arc<AppState>>,
	Json(body): Json<CategoryNameRequest>,
) -> ApiResult<Json<Value>> {
	let name = body.name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	ensure_unique_name(&state.pool, name, None).await?;
	let (sort_order,): (i64,) =
		sqlx::query_as("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM category WHERE user_id = 1")
			.fetch_one(&state.pool)
			.await?;
	sqlx::query("INSERT INTO category (user_id, name, sort_order) VALUES (?, ?, ?)")
		.bind(USER_ID)
		.bind(name)
		.bind(sort_order)
		.execute(&state.pool)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM category WHERE user_id = 1 AND name = ?")
		.bind(name)
		.fetch_one(&state.pool)
		.await?;
	Ok(Json(category_json(&category_row(&state.pool, id).await?)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryRequest {
	pub name: Option<String>,
	pub include_in_update: Option<i64>,
	pub include_in_download: Option<i64>,
}

fn check_include_in(field: &str, value: i64) -> ApiResult<i64> {
	match value {
		v @ 0..=2 => Ok(v),
		v => Err(ApiError::bad_request(format!("{field} must be 0, 1 or 2, got {v}"))),
	}
}

/// 手动解析 body：axum 的 Json 拒绝是 422，端点约定非法值 400；空 body 视为无操作
pub async fn update_category(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let row = category_row(&state.pool, id).await?;
	let patch = match body {
		None => UpdateCategoryRequest::default(),
		Some(Json(value)) => serde_json::from_value(value)
			.map_err(|e| ApiError::bad_request(format!("invalid category patch: {e}")))?,
	};
	let include_in_update = patch
		.include_in_update
		.map(|v| check_include_in("includeInUpdate", v))
		.transpose()?;
	let include_in_download = patch
		.include_in_download
		.map(|v| check_include_in("includeInDownload", v))
		.transpose()?;
	let name = match patch.name {
		Some(name) => {
			let name = name.trim();
			if name.is_empty() {
				return Err(ApiError::bad_request("name must not be empty"));
			}
			ensure_unique_name(&state.pool, name, Some(id)).await?;
			Some(name.to_string())
		}
		None => None,
	};
	if name.is_none() && include_in_update.is_none() && include_in_download.is_none() {
		return Ok(Json(category_json(&row)));
	}
	sqlx::query(
		"UPDATE category SET name = ?, include_in_update = ?, include_in_download = ?
		 WHERE id = ? AND user_id = 1",
	)
	.bind(name.as_deref().unwrap_or(&row.name))
	.bind(include_in_update.unwrap_or(row.include_in_update))
	.bind(include_in_download.unwrap_or(row.include_in_download))
	.bind(id)
	.execute(&state.pool)
	.await?;
	Ok(Json(category_json(&category_row(&state.pool, id).await?)))
}

pub async fn delete_category(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	category_row(&state.pool, id).await?;
	sqlx::query("DELETE FROM category WHERE id = ? AND user_id = 1")
		.bind(id)
		.execute(&state.pool)
		.await?;
	Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ReorderRequest {
	pub ids: Vec<i64>,
}

pub async fn reorder_categories(
	State(state): State<Arc<AppState>>,
	Json(body): Json<ReorderRequest>,
) -> ApiResult<Json<Value>> {
	let mut tx = state.pool.begin().await?;
	for (index, id) in body.ids.iter().enumerate() {
		sqlx::query("UPDATE category SET sort_order = ? WHERE id = ? AND user_id = 1")
			.bind(index as i64)
			.bind(id)
			.execute(&mut *tx)
			.await?;
	}
	tx.commit().await?;
	list_categories(State(state)).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCategoriesRequest {
	pub category_ids: Vec<i64>,
}

pub async fn set_manga_categories(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<SetCategoriesRequest>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, id).await?;
	let in_library: Option<(bool,)> =
		sqlx::query_as("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = 1")
			.bind(id)
			.fetch_optional(&state.pool)
			.await?;
	if !in_library.map(|(flag,)| flag).unwrap_or(false) {
		return Err(ApiError::not_in_library("manga not in library"));
	}
	for category_id in &body.category_ids {
		category_row(&state.pool, *category_id).await?;
	}
	let mut tx = state.pool.begin().await?;
	sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = 1")
		.bind(id)
		.execute(&mut *tx)
		.await?;
	for category_id in &body.category_ids {
		sqlx::query(
			"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
			 VALUES (?, ?, 1)",
		)
		.bind(category_id)
		.bind(id)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	Ok(Json(manga_json(&state, &manga).await?))
}
