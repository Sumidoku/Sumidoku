use crate::error::{ApiError, ApiResult, Error};
use crate::state::AppState;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use axum::Json;
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

pub struct AuthState {
	/// argon2 PHC 串；None = 认证关闭（未配置密码，0.2.0 行为）
	password_hash: RwLock<Option<String>>,
	/// 配置面 API key 的 sha256 指纹，内存常驻
	api_key_hashes: HashSet<String>,
	/// OPDS 每页图一次请求，逐次 argon2 太贵；缓存上一次验证通过的密码指纹
	basic_cache: RwLock<Option<String>>,
}

impl AuthState {
	pub fn new(api_keys: &[String]) -> Self {
		Self {
			password_hash: RwLock::new(None),
			api_key_hashes: api_keys.iter().map(|k| sha256_hex(k)).collect(),
			basic_cache: RwLock::new(None),
		}
	}

	pub async fn enabled(&self) -> bool {
		self.password_hash.read().await.is_some()
	}

	pub async fn verify_basic(&self, password: &str) -> bool {
		let fingerprint = sha256_hex(password);
		if self.basic_cache.read().await.as_deref() == Some(fingerprint.as_str()) {
			return true;
		}
		let guard = self.password_hash.read().await;
		let Some(hash) = guard.as_deref() else {
			return false;
		};
		if !verify_password(hash, password) {
			return false;
		}
		drop(guard);
		*self.basic_cache.write().await = Some(fingerprint);
		true
	}
}

/// 启动时落实密码：config/env 配置面优先（覆盖库内哈希），否则沿用库内已存哈希；
/// 库内也没有 = 认证关闭。改密码端点只写库，重启遇配置面密码仍以配置面为准
pub async fn init(state: &AppState) -> Result<(), Error> {
	if let Some(password) = state.config.effective_password() {
		let hash = hash_password(password)?;
		sqlx::query(
			"INSERT INTO auth_credential (id, password_hash) VALUES (1, ?)
			 ON CONFLICT (id) DO UPDATE SET password_hash = excluded.password_hash",
		)
		.bind(&hash)
		.execute(&state.pool)
		.await?;
		*state.auth.password_hash.write().await = Some(hash);
		return Ok(());
	}
	let row: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM auth_credential WHERE id = 1")
		.fetch_optional(&state.pool)
		.await?;
	*state.auth.password_hash.write().await = row.map(|(hash,)| hash);
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

/// 会话 token：sha256 指纹查库，顺带滚动续期
async fn valid_session_token(state: &AppState, fingerprint: &str) -> ApiResult<bool> {
	let now = crate::model::unix_now();
	let row: Option<(i64,)> = sqlx::query_as("SELECT expires_at FROM auth_token WHERE token_hash = ?")
		.bind(fingerprint)
		.fetch_optional(&state.pool)
		.await?;
	let Some((expires_at,)) = row else {
		return Ok(false);
	};
	if expires_at <= now {
		return Ok(false);
	}
	if expires_at - now < TOKEN_RENEW_BELOW_SECS {
		sqlx::query("UPDATE auth_token SET expires_at = ? WHERE token_hash = ?")
			.bind(now + TOKEN_TTL_SECS)
			.bind(fingerprint)
			.execute(&state.pool)
			.await?;
	}
	Ok(true)
}

async fn bearer_granted(state: &AppState, token: Option<String>) -> ApiResult<bool> {
	let Some(token) = token else {
		return Ok(false);
	};
	let fingerprint = sha256_hex(&token);
	if state.auth.api_key_hashes.contains(&fingerprint) {
		return Ok(true);
	}
	valid_session_token(state, &fingerprint).await
}

/// /api/v1 统一认证：认证关闭时放行；凭证无效/缺席一律 401 同形状
pub async fn require_auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
	if !state.auth.enabled().await {
		return next.run(req).await;
	}
	// axum Body 是 UnsyncBoxBody（不 Sync），&Request 不能跨 await 持有，先取走凭证
	let token = presented_token(&req);
	match bearer_granted(&state, token).await {
		Ok(true) => next.run(req).await,
		Ok(false) => ApiError::unauthorized().into_response(),
		Err(e) => e.into_response(),
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
	pub password: String,
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
	Json(json!({ "enabled": state.auth.enabled().await }))
}

pub async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginRequest>) -> ApiResult<Json<Value>> {
	let hash = state.auth.password_hash.read().await.clone();
	let Some(hash) = hash else {
		return Err(ApiError::bad_request("authentication is not enabled"));
	};
	if !verify_password(&hash, &body.password) {
		return Err(ApiError::unauthorized());
	}
	let mut raw = [0u8; 32];
	OsRng.fill_bytes(&mut raw);
	let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
	let now = crate::model::unix_now();
	let expires_at = now + TOKEN_TTL_SECS;
	sqlx::query("INSERT INTO auth_token (token_hash, created_at, expires_at) VALUES (?, ?, ?)")
		.bind(sha256_hex(&token))
		.bind(now)
		.bind(expires_at)
		.execute(&state.pool)
		.await?;
	// 顺手清掉过期 token，表只随活跃会话增长
	sqlx::query("DELETE FROM auth_token WHERE expires_at <= ?")
		.bind(now)
		.execute(&state.pool)
		.await?;
	Ok(Json(json!({ "token": token, "expiresAt": expires_at })))
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

/// 改密码：验旧换新，同时吊销全部会话 token（改密即全端下线）
pub async fn change_password(
	State(state): State<Arc<AppState>>,
	Json(body): Json<PasswordChangeRequest>,
) -> ApiResult<StatusCode> {
	if body.new_password.is_empty() {
		return Err(ApiError::bad_request("newPassword must not be empty"));
	}
	let hash = state.auth.password_hash.read().await.clone();
	let Some(hash) = hash else {
		return Err(ApiError::bad_request("authentication is not enabled"));
	};
	if !verify_password(&hash, &body.current_password) {
		return Err(ApiError::unauthorized());
	}
	let new_hash = hash_password(&body.new_password)
		.map_err(|e| ApiError::internal(e.to_string()))?;
	sqlx::query(
		"INSERT INTO auth_credential (id, password_hash) VALUES (1, ?)
		 ON CONFLICT (id) DO UPDATE SET password_hash = excluded.password_hash",
	)
	.bind(&new_hash)
	.execute(&state.pool)
	.await?;
	sqlx::query("DELETE FROM auth_token")
		.execute(&state.pool)
		.await?;
	*state.auth.password_hash.write().await = Some(new_hash);
	*state.auth.basic_cache.write().await = None;
	Ok(StatusCode::NO_CONTENT)
}

/// OPDS Basic 统一验同一密码；用户名任意非空、密码匹配即过（§9 恒 Basic 语义不变）
pub(crate) async fn basic_granted(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
	let cred = headers
		.get(axum::http::header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Basic "))
		.and_then(crate::opds::base64_decode)
		.and_then(|raw| String::from_utf8(raw).ok());
	let Some(cred) = cred else {
		return false;
	};
	match cred.split_once(':') {
		Some((user, password)) => !user.is_empty() && state.auth.verify_basic(password).await,
		None => false,
	}
}
