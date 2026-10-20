use crate::error::{ApiError, ApiResult, Error};
use crate::state::AppState;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use axum::{Extension, Json};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 会话 token 30 天滚动：剩余不足一半时随用续期
const TOKEN_TTL_SECS: i64 = 30 * 24 * 3600;
const TOKEN_RENEW_BELOW_SECS: i64 = TOKEN_TTL_SECS / 2;

/// web 登录是低频人肉操作，内存参数调低于 RFC 推荐值换延迟（OWASP 下限 m=7MiB/t=3）
fn argon() -> Argon2<'static> {
	Argon2::new(
		Algorithm::Argon2id,
		Version::V0x13,
		Params::new(7168, 3, 1, None).expect("argon2 params"),
	)
}

pub fn hash_password(password: &str) -> Result<String, Error> {
	let salt = SaltString::generate(&mut OsRng);
	argon()
		.hash_password(password.as_bytes(), &salt)
		.map(|h| h.to_string())
		.map_err(|e| Error::Auth(format!("hash password: {e}")))
}

fn verify_password(hash: &str, password: &str) -> bool {
	let Ok(parsed) = PasswordHash::new(hash) else {
		return false;
	};
	argon().verify_password(password.as_bytes(), &parsed).is_ok()
}

pub fn sha256_hex(raw: &str) -> String {
	let digest = Sha256::digest(raw.as_bytes());
	let mut out = String::with_capacity(64);
	for b in digest {
		use std::fmt::Write as _;
		let _ = write!(out, "{b:02x}");
	}
	out
}

/// 一次请求解析出的登录身份；require_auth / OPDS basic_auth 注入 request extensions
#[derive(Debug, Clone)]
pub struct AuthUser {
	pub id: i64,
	pub username: String,
	pub role: String,
}

impl AuthUser {
	/// 内置 admin（user 1 的种子值）：免密码部署与配置面 api_keys 都按 admin 身份行事，
	/// 此刻用户名只用于展示，免查库
	pub(crate) fn admin() -> Self {
		Self {
			id: 1,
			username: "admin".into(),
			role: "admin".into(),
		}
	}
}

pub struct AuthState {
	/// user 1 的 argon2 PHC 串内存副本（认证开关只看它：多用户以 admin 先设密码为前提）；
	/// None = 认证关闭（未配置密码，0.2.0 行为）
	admin_password_hash: RwLock<Option<String>>,
	/// 配置面 API key 的 sha256 指纹，内存常驻
	api_key_hashes: HashSet<String>,
	/// OPDS 每页图一次请求，逐次 argon2 太贵；缓存上一次验证通过的 (用户名, 密码指纹, 身份)
	basic_cache: RwLock<Option<(String, String, AuthUser)>>,
}

impl AuthState {
	pub fn new(api_keys: &[String]) -> Self {
		Self {
			admin_password_hash: RwLock::new(None),
			api_key_hashes: api_keys.iter().map(|k| sha256_hex(k)).collect(),
			basic_cache: RwLock::new(None),
		}
	}

	pub async fn enabled(&self) -> bool {
		self.admin_password_hash.read().await.is_some()
	}
}

/// 启动时落实 admin 密码：config/env 配置面优先（覆盖库内哈希），否则沿用库内已存哈希；
/// 库内为空串 = 认证关闭。改密码端点只写库，重启遇配置面密码仍以配置面为准
pub async fn init(state: &AppState) -> Result<(), Error> {
	if let Some(password) = state.config.effective_password() {
		let hash = hash_password(password)?;
		sqlx::query("UPDATE user SET password_hash = ? WHERE id = 1")
			.bind(&hash)
			.execute(&state.pool)
			.await?;
		*state.auth.admin_password_hash.write().await = Some(hash);
		return Ok(());
	}
	let row: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM user WHERE id = 1")
		.fetch_optional(&state.pool)
		.await?;
	*state.auth.admin_password_hash.write().await = row.map(|(hash,)| hash).filter(|hash| !hash.is_empty());
	Ok(())
}

/// 从 Authorization: Bearer 或 access_token query 取凭证（<img>/WebSocket 发不了头）
fn presented_token(req: &Request) -> Option<String> {
	if let Some(value) = req
		.headers()
		.get(axum::http::header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Bearer "))
	{
		return Some(value.to_string());
	}
	let query = req.uri().query()?;
	for pair in query.split('&') {
		if let Some(value) = pair.strip_prefix("access_token=") {
			return Some(value.to_string());
		}
	}
	None
}

/// 用户名+密码核验：用户不存在与密码错误走同一路径，对外只有 401 同形状
async fn verify_user_credentials(
	pool: &sqlx::SqlitePool,
	username: &str,
	password: &str,
) -> ApiResult<Option<AuthUser>> {
	let row: Option<(i64, String, String, String)> =
		sqlx::query_as("SELECT id, username, role, password_hash FROM user WHERE username = ?")
			.bind(username)
			.fetch_optional(pool)
			.await?;
	let Some((id, username, role, hash)) = row else {
		return Ok(None);
	};
	if hash.is_empty() || !verify_password(&hash, password) {
		return Ok(None);
	}
	Ok(Some(AuthUser { id, username, role }))
}

/// 会话 token：sha256 指纹查库，JOIN user 顺带核验归属用户还在；未过期则滚动续期
async fn session_user(state: &AppState, fingerprint: &str) -> ApiResult<Option<AuthUser>> {
	let now = crate::model::unix_now();
	let row: Option<(i64, i64, String, String)> = sqlx::query_as(
		"SELECT t.expires_at, u.id, u.username, u.role FROM auth_token t
		 JOIN user u ON u.id = t.user_id WHERE t.token_hash = ?",
	)
	.bind(fingerprint)
	.fetch_optional(&state.pool)
	.await?;
	let Some((expires_at, id, username, role)) = row else {
		return Ok(None);
	};
	if expires_at <= now {
		return Ok(None);
	}
	if expires_at - now < TOKEN_RENEW_BELOW_SECS {
		sqlx::query("UPDATE auth_token SET expires_at = ? WHERE token_hash = ?")
			.bind(now + TOKEN_TTL_SECS)
			.bind(fingerprint)
			.execute(&state.pool)
			.await?;
	}
	Ok(Some(AuthUser { id, username, role }))
}

async fn bearer_user(state: &AppState, token: Option<String>) -> ApiResult<Option<AuthUser>> {
	let Some(token) = token else {
		return Ok(None);
	};
	let fingerprint = sha256_hex(&token);
	if state.auth.api_key_hashes.contains(&fingerprint) {
		return Ok(Some(AuthUser::admin()));
	}
	session_user(state, &fingerprint).await
}

/// /api/v1 统一认证：认证关闭时一切请求当内置 admin；凭证无效/缺席一律 401 同形状
pub async fn require_auth(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
	if !state.auth.enabled().await {
		req.extensions_mut().insert(AuthUser::admin());
		return next.run(req).await;
	}
	// axum Body 是 UnsyncBoxBody（不 Sync），&Request 不能跨 await 持有，先取走凭证
	let token = presented_token(&req);
	match bearer_user(&state, token).await {
		Ok(Some(user)) => {
			req.extensions_mut().insert(user);
			next.run(req).await
		}
		Ok(None) => ApiError::unauthorized().into_response(),
		Err(e) => e.into_response(),
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
	pub username: String,
	pub password: String,
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
	Json(json!({ "enabled": state.auth.enabled().await }))
}

pub async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginRequest>) -> ApiResult<Json<Value>> {
	if !state.auth.enabled().await {
		return Err(ApiError::bad_request("authentication is not enabled"));
	}
	let Some(user) = verify_user_credentials(&state.pool, &body.username, &body.password).await? else {
		return Err(ApiError::unauthorized());
	};
	let mut raw = [0u8; 32];
	OsRng.fill_bytes(&mut raw);
	let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
	let now = crate::model::unix_now();
	let expires_at = now + TOKEN_TTL_SECS;
	sqlx::query("INSERT INTO auth_token (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
		.bind(sha256_hex(&token))
		.bind(user.id)
		.bind(now)
		.bind(expires_at)
		.execute(&state.pool)
		.await?;
	// 顺手清掉过期 token，表只随活跃会话增长
	sqlx::query("DELETE FROM auth_token WHERE expires_at <= ?")
		.bind(now)
		.execute(&state.pool)
		.await?;
	Ok(Json(json!({
		"token": token,
		"expiresAt": expires_at,
		"user": { "id": user.id, "username": user.username, "role": user.role },
	})))
}

/// 吊销呈上来的 bearer；无凭证/未知 token 也 204，不泄露 token 有效性
pub async fn logout(State(state): State<Arc<AppState>>, req: Request) -> StatusCode {
	if let Some(token) = presented_token(&req) {
		let _ = sqlx::query("DELETE FROM auth_token WHERE token_hash = ?")
			.bind(sha256_hex(&token))
			.execute(&state.pool)
			.await;
	}
	StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChangeRequest {
	pub current_password: String,
	pub new_password: String,
}

/// 改密码：只能改自己的；验旧换新后吊销自己全部会话 token（改密即全端下线）
pub async fn change_password(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<AuthUser>,
	Json(body): Json<PasswordChangeRequest>,
) -> ApiResult<StatusCode> {
	if body.new_password.is_empty() {
		return Err(ApiError::bad_request("newPassword must not be empty"));
	}
	if !state.auth.enabled().await {
		return Err(ApiError::bad_request("authentication is not enabled"));
	}
	let hash: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM user WHERE id = ?")
		.bind(user.id)
		.fetch_optional(&state.pool)
		.await?;
	let Some((hash,)) = hash else {
		return Err(ApiError::unauthorized());
	};
	if hash.is_empty() || !verify_password(&hash, &body.current_password) {
		return Err(ApiError::unauthorized());
	}
	let new_hash = hash_password(&body.new_password).map_err(|e| ApiError::internal(e.to_string()))?;
	sqlx::query("UPDATE user SET password_hash = ? WHERE id = ?")
		.bind(&new_hash)
		.bind(user.id)
		.execute(&state.pool)
		.await?;
	sqlx::query("DELETE FROM auth_token WHERE user_id = ?")
		.bind(user.id)
		.execute(&state.pool)
		.await?;
	if user.id == 1 {
		*state.auth.admin_password_hash.write().await = Some(new_hash);
	}
	*state.auth.basic_cache.write().await = None;
	Ok(StatusCode::NO_CONTENT)
}

/// OPDS Basic 核验真实用户名+密码（§9 恒 Basic 语义不变）；命中缓存免逐次 argon2
pub(crate) async fn basic_user(state: &AppState, headers: &axum::http::HeaderMap) -> ApiResult<Option<AuthUser>> {
	let cred = headers
		.get(axum::http::header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Basic "))
		.and_then(crate::opds::base64_decode)
		.and_then(|raw| String::from_utf8(raw).ok());
	let Some(cred) = cred else {
		return Ok(None);
	};
	let Some((username, password)) = cred.split_once(':') else {
		return Ok(None);
	};
	let fingerprint = sha256_hex(password);
	{
		let cache = state.auth.basic_cache.read().await;
		if let Some((cached_username, cached_fingerprint, user)) = cache.as_ref()
			&& cached_username == username
			&& cached_fingerprint == &fingerprint
		{
			return Ok(Some(user.clone()));
		}
	}
	let Some(user) = verify_user_credentials(&state.pool, username, password).await? else {
		return Ok(None);
	};
	*state.auth.basic_cache.write().await = Some((username.to_string(), fingerprint, user.clone()));
	Ok(Some(user))
}
