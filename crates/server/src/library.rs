use crate::api::manga_json;
use crate::error::{ApiError, ApiResult};
use crate::model::{LibraryEntryRow, unix_now};
use crate::service::manga_row;
use crate::state::AppState;
use crate::{PAGE_SIZE, USER_ID};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::sync::Arc;

const LIBRARY_SELECT: &str = "SELECT m.*, mu.in_library_at FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1
	 WHERE mu.in_library = 1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
	pub category_id: Option<i64>,
	pub page: Option<i64>,
}

pub async fn list_library(
	State(state): State<Arc<AppState>>,
	Query(query): Query<LibraryQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	// categoryId=0 指「在库但未分类」，其余 id 指该分类下；缺省不过滤
	let (sql, category) = match query.category_id {
		None => (
			format!("{LIBRARY_SELECT} ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"),
			None,
		),
		Some(0) => (
			format!(
				"{LIBRARY_SELECT} AND NOT EXISTS (SELECT 1 FROM category_manga cm
				 WHERE cm.manga_id = m.id AND cm.user_id = 1)
				 ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
			),
			None,
		),
		Some(category_id) => {
			category_row(&state.pool, category_id).await?;
			(
				format!(
					"{LIBRARY_SELECT} AND EXISTS (SELECT 1 FROM category_manga cm
					 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND cm.category_id = ?)
					 ORDER BY mu.in_library_at DESC, m.id ASC LIMIT ? OFFSET ?"
				),
				Some(category_id),
			)
		}
	};
	let mut q = sqlx::query_as::<_, LibraryEntryRow>(&sql);
	if let Some(category_id) = category {
		q = q.bind(category_id);
	}
	let rows = q
		.bind(PAGE_SIZE + 1)
		.bind((page - 1) * PAGE_SIZE)
		.fetch_all(&state.pool)
		.await?;
	let has_next_page = rows.len() as i64 > PAGE_SIZE;
	let mut entries = Vec::with_capacity(rows.len().min(PAGE_SIZE as usize));
	for row in rows.iter().take(PAGE_SIZE as usize) {
		let mut value = manga_json(&state, &row.manga).await?;
		let object = value
			.as_object_mut()
			.ok_or_else(|| ApiError::bad_request("unexpected manga shape"))?;
		object.insert("inLibrary".into(), json!(true));
		object.insert("inLibraryAt".into(), json!(row.in_library_at));
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
	manga_count: i64,
}

const CATEGORY_SELECT: &str = "SELECT c.id, c.name, c.sort_order, c.is_default,
	 (SELECT COUNT(*) FROM category_manga cm
	  WHERE cm.category_id = c.id AND cm.user_id = 1) AS manga_count
	 FROM category c WHERE c.user_id = 1";

fn category_json(row: &CategoryRow) -> Value {
	json!({
		"id": row.id,
		"name": row.name,
		"sortOrder": row.sort_order,
		"isDefault": row.is_default,
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

pub async fn update_category(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<CategoryNameRequest>,
) -> ApiResult<Json<Value>> {
	category_row(&state.pool, id).await?;
	let name = body.name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	ensure_unique_name(&state.pool, name, Some(id)).await?;
	sqlx::query("UPDATE category SET name = ? WHERE id = ? AND user_id = 1")
		.bind(name)
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
