//! 用户管理（admin 专属面）：列表/建号/改号/删号。
//! 免认证部署没有登录概念，全部端点以先设密码为前提（未启用认证一律 400）

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

const ROLES: &[&str] = &["admin", "reader"];

async fn refuse_when_auth_disabled(state: &AppState) -> ApiResult<()> {
	if !state.auth.enabled().await {
		return Err(ApiError::bad_request("authentication is not enabled"));
	}
	Ok(())
}

fn user_json(id: i64, username: &str, role: &str, created_at: i64) -> Value {
	json!({ "id": id, "username": username, "role": role, "createdAt": created_at })
}

/// 列表不带 password_hash：凭证材料永不出库
pub async fn list_users(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	refuse_when_auth_disabled(&state).await?;
	let rows: Vec<(i64, String, String, i64)> =
		sqlx::query_as("SELECT id, username, role, created_at FROM user ORDER BY id")
			.fetch_all(&state.pool)
			.await?;
	let users: Vec<Value> = rows
		.into_iter()
		.map(|(id, username, role, created_at)| user_json(id, &username, &role, created_at))
		.collect();
	Ok(Json(json!(users)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
	pub username: String,
	pub password: String,
	pub role: String,
}

fn validate(username: &str, role: &str) -> ApiResult<()> {
	if username.is_empty() {
		return Err(ApiError::bad_request("username must not be empty"));
	}
	if !ROLES.contains(&role) {
		return Err(ApiError::bad_request(format!(
			"role must be one of: {}",
			ROLES.join(", ")
		)));
	}
	Ok(())
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
	e.as_database_error()
		.is_some_and(|db| db.is_unique_violation())
}

pub async fn create_user(
	State(state): State<Arc<AppState>>,
	Json(body): Json<CreateUserRequest>,
) -> ApiResult<Json<Value>> {
	refuse_when_auth_disabled(&state).await?;
	let username = body.username.trim();
	validate(username, &body.role)?;
	if body.password.is_empty() {
		return Err(ApiError::bad_request("password must not be empty"));
	}
	let hash = crate::auth::hash_password(&body.password).map_err(|e| ApiError::internal(e.to_string()))?;
	let created_at = crate::model::unix_now();
	let result = sqlx::query("INSERT INTO user (username, password_hash, role, created_at) VALUES (?, ?, ?, ?)")
		.bind(username)
		.bind(&hash)
		.bind(&body.role)
		.bind(created_at)
		.execute(&state.pool)
	.await;
	match result {
		Ok(done) => Ok(Json(user_json(
			done.last_insert_rowid(),
			username,
			&body.role,
			created_at,
		))),
		Err(e) if is_unique_violation(&e) => Err(ApiError::duplicate(format!("username {username} is taken"))),
		Err(e) => Err(ApiError::from(e)),
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
	pub username: Option<String>,
	pub role: Option<String>,
	pub password: Option<String>,
}

/// 改号：用户名/角色/重置密码按字段缺席与否逐项生效；重置密码吊销该用户全部会话
pub async fn update_user(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<UpdateUserRequest>,
) -> ApiResult<Json<Value>> {
	refuse_when_auth_disabled(&state).await?;
	let row: Option<(String, String, i64)> =
		sqlx::query_as("SELECT username, role, created_at FROM user WHERE id = ?")
			.bind(id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((mut username, mut role, created_at)) = row else {
		return Err(ApiError::not_found("user not found"));
	};
	if let Some(name) = &body.username {
		let name = name.trim();
		validate(name, &role)?;
		if name != username {
			let result = sqlx::query("UPDATE user SET username = ? WHERE id = ?")
				.bind(name)
				.bind(id)
				.execute(&state.pool)
				.await;
			match result {
				Ok(_) => username = name.to_string(),
				Err(e) if is_unique_violation(&e) => {
					return Err(ApiError::duplicate(format!("username {name} is taken")));
				}
				Err(e) => return Err(ApiError::from(e)),
			}
		}
	}
	if let Some(new_role) = &body.role {
		validate(&username, new_role)?;
		sqlx::query("UPDATE user SET role = ? WHERE id = ?")
			.bind(new_role)
			.bind(id)
			.execute(&state.pool)
			.await?;
		role = new_role.clone();
	}
	if let Some(password) = &body.password {
		if password.is_empty() {
			return Err(ApiError::bad_request("password must not be empty"));
		}
		let hash = crate::auth::hash_password(password).map_err(|e| ApiError::internal(e.to_string()))?;
		sqlx::query("UPDATE user SET password_hash = ? WHERE id = ?")
			.bind(&hash)
			.bind(id)
			.execute(&state.pool)
			.await?;
		// 与自助改密同语义：重置即全端下线
		sqlx::query("DELETE FROM auth_token WHERE user_id = ?")
			.bind(id)
			.execute(&state.pool)
			.await?;
		if id == 1 {
			state.auth.set_admin_password_hash(hash).await;
		}
	}
	if body.username.is_some() || body.role.is_some() || body.password.is_some() {
		state.auth.invalidate_basic_cache().await;
	}
	Ok(Json(user_json(id, &username, &role, created_at)))
}

/// 删号：user 1 是内置 admin 不可删。用户态行（库/进度/分类/下载/token）全部 FK 级联，
/// 下载产物文件是多用户共享的确定性路径，不随删号清理
pub async fn delete_user(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
	refuse_when_auth_disabled(&state).await?;
	if id == 1 {
		return Err(ApiError::bad_request("the built-in admin user cannot be deleted"));
	}
	let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM user WHERE id = ?")
		.bind(id)
		.fetch_optional(&state.pool)
		.await?;
	if exists.is_none() {
		return Err(ApiError::not_found("user not found"));
	}
	let mut tx = state.pool.begin().await?;
	// user.<id>.* 命名空间键在 setting 表里没有外键，级联够不着
	sqlx::query("DELETE FROM setting WHERE key LIKE ?")
		.bind(format!("user.{id}.%"))
		.execute(&mut *tx)
		.await?;
	sqlx::query("DELETE FROM user WHERE id = ?")
		.bind(id)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	state.auth.invalidate_basic_cache().await;
	Ok(StatusCode::NO_CONTENT)
}
