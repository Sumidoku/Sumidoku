use crate::api::{manga_json, manga_json_batch};
use crate::error::{ApiError, ApiResult};
use crate::model::{LibraryEntryRow, MangaRow, unix_now};
use crate::source::manga_row;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

const LIBRARY_SELECT: &str = "SELECT m.*, mu.in_library_at FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ?
	 WHERE mu.in_library = 1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
	pub category_id: Option<i64>,
	/// The migration batch flow filters entries by source
	pub source_id: Option<i64>,
	pub page: Option<i64>,
	pub search: Option<String>,
	pub sort: Option<String>,
	pub filter: Option<String>,
	/// Comma-separated exclusion set for boolean filters (same vocabulary as filter): unread excluded = everything read, and so on
	pub filter_exclude: Option<String>,
	/// Comma-separated manga-level rating values (1..=3), inclusion set
	pub content_rating: Option<String>,
	pub content_rating_exclude: Option<String>,
	/// Comma-separated source ids, inclusion set
	pub source: Option<String>,
	pub source_exclude: Option<String>,
	/// Comma-separated category ids, inclusion set; 0 means uncategorized (no category rows of the user's own)
	pub category: Option<String>,
	pub category_exclude: Option<String>,
	/// A persisted filter group whose rules compile into the same WHERE clause, ANDed with the ad-hoc filters
	pub group_id: Option<i64>,
}

/// LIKE's %/_ are wildcards, so literal occurrences and the escape character itself in the user's search term must be escaped upfront
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

/// Returns the ORDER BY clause and whether it carries a user_id bind (the lastRead/unread subqueries read per user)
fn library_order_by(sort: Option<&str>) -> ApiResult<(&'static str, bool)> {
	const LAST_READ: &str = " ORDER BY (SELECT MAX(cu.last_read_at) FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = m.id) DESC, m.id ASC";
	// Default-unread: a chapter with no chapter_user row counts as unread, same as the unreadCount badge
	const UNREAD: &str = " ORDER BY (SELECT COUNT(*) FROM chapter c
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_read = 1
		 WHERE c.manga_id = m.id AND cu.chapter_id IS NULL) DESC, m.id ASC";
	match sort {
		None | Some("added") => Ok((" ORDER BY mu.in_library_at DESC, m.id ASC", false)),
		Some("title") => Ok((" ORDER BY m.title COLLATE NOCASE ASC, m.id ASC", false)),
		// In SQLite NULL sorts below any value, so DESC naturally sinks it to the end
		Some("lastRead") => Ok((LAST_READ, true)),
		Some("unread") => Ok((UNREAD, true)),
		Some(other) => Err(ApiError::bad_request(format!("invalid sort: {other}"))),
	}
}

fn library_filter_clause(filter: &str, exclude: bool) -> ApiResult<String> {
	const EXISTS: &str = " AND EXISTS (SELECT 1 FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = m.id AND ";
	const NOT_EXISTS: &str = " AND NOT EXISTS (SELECT 1 FROM chapter c
		 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = m.id AND ";
	let column = match filter {
		"unread" => "cu.is_read = 0",
		"downloaded" => "cu.is_downloaded = 1",
		"bookmarked" => "cu.is_bookmarked = 1",
		other => return Err(ApiError::bad_request(format!("invalid filter: {other}"))),
	};
	let head = if exclude { NOT_EXISTS } else { EXISTS };
	Ok(format!("{head}{column})"))
}

/// Comma-separated integer list: an all-blank value or blank segments count as absent, a non-blank illegal segment is a 400 (same style as sort/filter)
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
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<LibraryQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let (order_by, order_user_bind) = library_order_by(query.sort.as_deref())?;
	let search = query.search.filter(|s| !s.is_empty());
	// bind order follows the SQL clauses: user → search → categoryId → category include → category exclude →
	// rating include → rating exclude → source include → source exclude → sourceId → one user per filter clause →
	// the group's binds in compiled order → user for the ORDER BY clause
	let group_rules = match query.group_id {
		None => Vec::new(),
		Some(group_id) => crate::api::filter_groups::rules_for_library(&state.pool, user.id, group_id).await?,
	};
	let (group_sql, group_binds) = crate::api::filter_groups::compile_group(&group_rules)?;
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
	let cat_in = match &query.category {
		Some(raw) => parse_int_list(raw, "category", |v| v >= 0)?,
		None => Vec::new(),
	};
	let cat_ex = match &query.category_exclude {
		Some(raw) => parse_int_list(raw, "categoryExclude", |v| v >= 0)?,
		None => Vec::new(),
	};
	// category sets split into real ids (IN / NOT IN over the user's category rows) and the 0 arm
	// (uncategorized): include-0 is NOT EXISTS, exclude-0 is EXISTS
	let cat_in_links: Vec<i64> = cat_in.iter().copied().filter(|&id| id != 0).collect();
	let cat_ex_links: Vec<i64> = cat_ex.iter().copied().filter(|&id| id != 0).collect();
	let mut sql = LIBRARY_SELECT.to_string();
	if search.is_some() {
		sql.push_str(" AND m.title LIKE '%'||?||'%' ESCAPE '\\'");
	}
	// categoryId=0 means "in library but uncategorized", any other id means under that category; absent means no filtering
	match query.category_id {
		None => {}
		Some(0) => sql.push_str(
			" AND NOT EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ?)",
		),
		Some(category_id) => {
			category_row(&state.pool, user.id, category_id).await?;
			sql.push_str(
				" AND EXISTS (SELECT 1 FROM category_manga cm
				 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id = ?)",
			);
		}
	}
	// OR within the include set: real ids and the 0 arm (uncategorized) are alternatives,
	// so mixed sets must be one parenthesized OR group, not separate ANDed clauses
	let mut cat_in_arms: Vec<String> = Vec::new();
	if !cat_in_links.is_empty() {
		cat_in_arms.push(format!(
			"EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id IN ({}))",
			in_placeholders(cat_in_links.len())
		));
	}
	if cat_in.contains(&0) {
		cat_in_arms.push(
			"NOT EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ?)"
				.into(),
		);
	}
	if cat_in_arms.len() == 1 {
		sql.push_str(" AND ");
		sql.push_str(&cat_in_arms[0]);
	} else if !cat_in_arms.is_empty() {
		sql.push_str(" AND (");
		sql.push_str(&cat_in_arms.join(" OR "));
		sql.push(')');
	}
	if !cat_ex_links.is_empty() {
		sql.push_str(&format!(
			" AND NOT EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id IN ({}))",
			in_placeholders(cat_ex_links.len())
		));
	}
	if cat_ex.contains(&0) {
		sql.push_str(
			" AND EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ?)",
		);
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
	let mut filter_user_binds = 0usize;
	if let Some(raw) = &query.filter {
		for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
			sql.push_str(&library_filter_clause(token, false)?);
			filter_user_binds += 1;
		}
	}
	if let Some(raw) = &query.filter_exclude {
		for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
			sql.push_str(&library_filter_clause(token, true)?);
			filter_user_binds += 1;
		}
	}
	sql.push_str(&group_sql);
	sql.push_str(order_by);
	sql.push_str(" LIMIT ? OFFSET ?");
	let mut q = sqlx::query_as::<_, LibraryEntryRow>(&sql).bind(user.id);
	if let Some(search) = &search {
		q = q.bind(escape_like(search));
	}
	match query.category_id {
		None => {}
		Some(0) => q = q.bind(user.id),
		Some(category_id) => q = q.bind(user.id).bind(category_id),
	}
	if !cat_in_links.is_empty() {
		q = q.bind(user.id);
		for v in &cat_in_links {
			q = q.bind(v);
		}
	}
	if cat_in.contains(&0) {
		q = q.bind(user.id);
	}
	if !cat_ex_links.is_empty() {
		q = q.bind(user.id);
		for v in &cat_ex_links {
			q = q.bind(v);
		}
	}
	if cat_ex.contains(&0) {
		q = q.bind(user.id);
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
	for _ in 0..filter_user_binds {
		q = q.bind(user.id);
	}
	for bind in &group_binds {
		q = match bind {
			crate::api::filter_groups::GroupBind::User => q.bind(user.id),
			crate::api::filter_groups::GroupBind::Id(v) => q.bind(*v),
		};
	}
	if order_user_bind {
		q = q.bind(user.id);
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
		// Default-unread, same as the manga detail page: a chapter with no chapter_user row counts as unread
		let unread_sql = format!(
			"SELECT c.manga_id, COUNT(*) FROM chapter c
			 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_read = 1
			 WHERE c.manga_id IN ({placeholders}) AND cu.chapter_id IS NULL GROUP BY c.manga_id"
		);
		let mut q = sqlx::query_as::<_, (i64, i64)>(&unread_sql).bind(user.id);
		for row in &page_rows {
			q = q.bind(row.manga.id);
		}
		unread = q.fetch_all(&state.pool).await?.into_iter().collect();
	}
	let manga_refs: Vec<&MangaRow> = page_rows.iter().map(|row| &row.manga).collect();
	let values = manga_json_batch(&state, user.id, &manga_refs).await?;
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
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<AddToLibraryRequest>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, body.manga_id).await?;
	if let Some(category_ids) = &body.category_ids {
		for category_id in category_ids {
			category_row(&state.pool, user.id, *category_id).await?;
		}
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	// Entries already in the library keep their first-shelved time
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, ?, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(manga.id)
	.bind(user.id)
	.bind(unix_now())
	.execute(&mut *tx)
	.await?;
	if let Some(category_ids) = &body.category_ids {
		if category_ids.is_empty() {
			sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?")
				.bind(manga.id)
				.bind(user.id)
				.execute(&mut *tx)
				.await?;
		} else {
			let sql = format!(
				"DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?
				 AND category_id NOT IN ({})",
				in_placeholders(category_ids.len())
			);
			let mut delete = sqlx::query(&sql).bind(manga.id).bind(user.id);
			for category_id in category_ids {
				delete = delete.bind(category_id);
			}
			delete.execute(&mut *tx).await?;
		}
		for category_id in category_ids {
			sqlx::query(
				"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
				 VALUES (?, ?, ?)",
			)
			.bind(category_id)
			.bind(manga.id)
			.bind(user.id)
			.execute(&mut *tx)
			.await?;
		}
	}
	tx.commit().await?;
	Ok(Json(manga_json(&state, user.id, &manga).await?))
}

pub async fn remove_from_library(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	manga_row(&state.pool, id).await?;
	let mut tx = crate::db::begin_write(&state.pool).await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, ?, 0)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
	)
	.bind(id)
	.bind(user.id)
	.execute(&mut *tx)
	.await?;
	sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?")
		.bind(id)
		.bind(user.id)
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
	include_in_download: i64,
	manga_count: i64,
}

const CATEGORY_SELECT: &str = "SELECT c.id, c.name, c.sort_order, c.is_default,
	 c.include_in_download,
	 (SELECT COUNT(*) FROM category_manga cm
	  WHERE cm.category_id = c.id AND cm.user_id = ?) AS manga_count
	 FROM category c WHERE c.user_id = ?";

fn category_json(row: &CategoryRow) -> Value {
	json!({
		"id": row.id,
		"name": row.name,
		"sortOrder": row.sort_order,
		"isDefault": row.is_default,
		"includeInDownload": row.include_in_download,
		"mangaCount": row.manga_count,
	})
}

/// Categories belong to the requesting user: someone else's category id and a nonexistent one both get 404, not leaking existence
async fn category_row(pool: &SqlitePool, user_id: i64, id: i64) -> ApiResult<CategoryRow> {
	let row: Option<CategoryRow> =
		sqlx::query_as(&format!("{CATEGORY_SELECT} AND c.id = ?"))
			.bind(user_id)
			.bind(user_id)
			.bind(id)
			.fetch_optional(pool)
			.await?;
	row.ok_or_else(|| ApiError::not_found("category not found"))
}

/// Entry count per source in the library (for the library source-filter dropdown's counts and auto-hiding).
/// Includes entries of disabled sources — library manga do not vanish when a source is disabled
pub async fn source_counts(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let rows: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT m.source_id, COUNT(*) FROM manga m
		 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ?
		 WHERE mu.in_library = 1 GROUP BY m.source_id",
	)
	.bind(user.id)
	.fetch_all(&state.pool)
	.await?;
	let out: Vec<Value> = rows
		.into_iter()
		.map(|(source_id, count)| json!({ "sourceId": source_id, "count": count }))
		.collect();
	Ok(Json(json!(out)))
}

pub async fn list_categories(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	Ok(Json(categories_json(&state, user.id).await?))
}

async fn categories_json(state: &AppState, user_id: i64) -> ApiResult<Value> {
	let rows: Vec<CategoryRow> =
		sqlx::query_as(&format!("{CATEGORY_SELECT} ORDER BY c.sort_order ASC, c.id ASC"))
			.bind(user_id)
			.bind(user_id)
			.fetch_all(&state.pool)
			.await?;
	Ok(json!(rows.iter().map(category_json).collect::<Vec<_>>()))
}

#[derive(Debug, Deserialize)]
pub struct CategoryNameRequest {
	pub name: String,
}

async fn ensure_unique_name(pool: &SqlitePool, user_id: i64, name: &str, except_id: Option<i64>) -> ApiResult<()> {
	let duplicate: Option<(i64,)> = match except_id {
		Some(id) => {
			sqlx::query_as("SELECT id FROM category WHERE user_id = ? AND name = ? AND id != ?")
				.bind(user_id)
				.bind(name)
				.bind(id)
				.fetch_optional(pool)
				.await?
		}
		None => {
			sqlx::query_as("SELECT id FROM category WHERE user_id = ? AND name = ?")
				.bind(user_id)
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
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<CategoryNameRequest>,
) -> ApiResult<Json<Value>> {
	let name = body.name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	ensure_unique_name(&state.pool, user.id, name, None).await?;
	let (sort_order,): (i64,) =
		sqlx::query_as("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM category WHERE user_id = ?")
			.bind(user.id)
			.fetch_one(&state.pool)
			.await?;
	sqlx::query("INSERT INTO category (user_id, name, sort_order) VALUES (?, ?, ?)")
		.bind(user.id)
		.bind(name)
		.bind(sort_order)
		.execute(&state.pool)
	.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM category WHERE user_id = ? AND name = ?")
		.bind(user.id)
		.bind(name)
		.fetch_one(&state.pool)
		.await?;
	Ok(Json(category_json(&category_row(&state.pool, user.id, id).await?)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryRequest {
	pub name: Option<String>,
	pub include_in_download: Option<i64>,
}

fn check_include_in(field: &str, value: i64) -> ApiResult<i64> {
	match value {
		v @ 0..=2 => Ok(v),
		v => Err(ApiError::bad_request(format!("{field} must be 0, 1 or 2, got {v}"))),
	}
}

/// Parses the body by hand: axum's Json rejection is 422, while the endpoint convention for invalid values is 400; an empty body is a no-op
pub async fn update_category(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let row = category_row(&state.pool, user.id, id).await?;
	let patch = match body {
		None => UpdateCategoryRequest::default(),
		Some(Json(value)) => {
			if value.get("includeInUpdate").is_some() {
				return Err(ApiError::bad_request(
					"includeInUpdate is no longer supported; use the library.excluded_update_categories setting",
				));
			}
			serde_json::from_value(value)
				.map_err(|e| ApiError::bad_request(format!("invalid category patch: {e}")))?
		}
	};
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
			ensure_unique_name(&state.pool, user.id, name, Some(id)).await?;
			Some(name.to_string())
		}
		None => None,
	};
	if name.is_none() && include_in_download.is_none() {
		return Ok(Json(category_json(&row)));
	}
	sqlx::query(
		"UPDATE category SET name = ?, include_in_download = ?
		 WHERE id = ? AND user_id = ?",
	)
	.bind(name.as_deref().unwrap_or(&row.name))
	.bind(include_in_download.unwrap_or(row.include_in_download))
	.bind(id)
	.bind(user.id)
	.execute(&state.pool)
	.await?;
	Ok(Json(category_json(&category_row(&state.pool, user.id, id).await?)))
}

pub async fn delete_category(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	category_row(&state.pool, user.id, id).await?;
	sqlx::query("DELETE FROM category WHERE id = ? AND user_id = ?")
		.bind(id)
		.bind(user.id)
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
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<ReorderRequest>,
) -> ApiResult<Json<Value>> {
	let mut tx = crate::db::begin_write(&state.pool).await?;
	for (index, id) in body.ids.iter().enumerate() {
		sqlx::query("UPDATE category SET sort_order = ? WHERE id = ? AND user_id = ?")
			.bind(index as i64)
			.bind(id)
			.bind(user.id)
			.execute(&mut *tx)
			.await?;
	}
	tx.commit().await?;
	Ok(Json(categories_json(&state, user.id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCategoriesRequest {
	pub category_ids: Vec<i64>,
}

pub async fn set_manga_categories(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Json(body): Json<SetCategoriesRequest>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, id).await?;
	let in_library: Option<(bool,)> =
		sqlx::query_as("SELECT in_library FROM manga_user WHERE manga_id = ? AND user_id = ?")
			.bind(id)
			.bind(user.id)
			.fetch_optional(&state.pool)
			.await?;
	if !in_library.map(|(flag,)| flag).unwrap_or(false) {
		return Err(ApiError::not_in_library("manga not in library"));
	}
	for category_id in &body.category_ids {
		category_row(&state.pool, user.id, *category_id).await?;
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	if body.category_ids.is_empty() {
		sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?")
			.bind(id)
			.bind(user.id)
			.execute(&mut *tx)
			.await?;
	} else {
		let sql = format!(
			"DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?
			 AND category_id NOT IN ({})",
			in_placeholders(body.category_ids.len())
		);
		let mut delete = sqlx::query(&sql).bind(id).bind(user.id);
		for category_id in &body.category_ids {
			delete = delete.bind(category_id);
		}
		delete.execute(&mut *tx).await?;
	}
	for category_id in &body.category_ids {
		sqlx::query(
			"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
			 VALUES (?, ?, ?)",
		)
		.bind(category_id)
		.bind(id)
		.bind(user.id)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	Ok(Json(manga_json(&state, user.id, &manga).await?))
}
