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

/// Session tokens roll for 30 days: renewed on use once less than half remains
const TOKEN_TTL_SECS: i64 = 30 * 24 * 3600;
const TOKEN_RENEW_BELOW_SECS: i64 = TOKEN_TTL_SECS / 2;

/// Web login is a low-frequency human operation, so the memory parameter is tuned below the RFC recommendation for latency (OWASP floor m=7MiB/t=3)
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

/// The login identity resolved from one request; require_auth / OPDS basic_auth inject it into request extensions
#[derive(Debug, Clone)]
pub struct AuthUser {
	pub id: i64,
	pub username: String,
	pub role: String,
}

impl AuthUser {
	/// Built-in admin (seed value of user 1): passwordless deployments and config-side api_keys both act as
	/// admin; the username here is display-only, no DB lookup
	pub(crate) fn admin() -> Self {
		Self {
			id: 1,
			username: "admin".into(),
			role: "admin".into(),
		}
	}
}

pub struct AuthState {
	/// In-memory copy of user 1's argon2 PHC string (the auth switch reads only it: multi-user presupposes
	/// admin sets a password first); None = auth disabled (no password configured, 0.2.0 behavior)
	admin_password_hash: RwLock<Option<String>>,
	/// sha256 fingerprints of the config-side API keys, resident in memory
	api_key_hashes: HashSet<String>,
	/// OPDS fires one request per page image, so per-request argon2 is too expensive; cache the last verified (username, password fingerprint, identity)
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

	/// After a user record changes (deletion/password reset/role change), the OPDS-cached identity is voided too
	pub(crate) async fn invalidate_basic_cache(&self) {
		*self.basic_cache.write().await = None;
	}

	pub(crate) async fn set_admin_password_hash(&self, hash: String) {
		*self.admin_password_hash.write().await = Some(hash);
	}
}

/// Establishes the admin password at startup: the config/env side wins (overriding the stored hash), otherwise
/// keeps the hash already in the DB; an empty string in the DB = auth disabled. The change-password endpoint
/// only writes the DB; on restart a config-side password still wins
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

/// Takes the credential from Authorization: Bearer or the access_token query (<img>/WebSocket cannot send headers)
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

/// Username+password verification: unknown user and wrong password take the same path, so externally there is only one uniform 401
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

/// Session token: look up by sha256 fingerprint, JOIN user to verify the owner still exists; renewed on a rolling basis while unexpired
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

/// Uniform /api/v1 auth: with auth disabled every request is the built-in admin; invalid/absent credentials always get the same uniform 401
pub async fn require_auth(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
	if !state.auth.enabled().await {
		req.extensions_mut().insert(AuthUser::admin());
		return next.run(req).await;
	}
	// axum Body is UnsyncBoxBody (not Sync), so &Request cannot be held across await; take the credential first
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

/// Uniform gate for admin-only surfaces: runs after require_auth and reads the injected role, readers always get 403;
/// with auth disabled the injected identity is the built-in admin, so passwordless deployments behave unchanged
pub async fn require_admin(req: Request, next: Next) -> Response {
	match req.extensions().get::<AuthUser>() {
		Some(user) if user.role == "admin" => next.run(req).await,
		_ => ApiError::forbidden("admin role required").into_response(),
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
	pub username: String,
	pub password: String,
}

/// Auth switch + current credential identity: after an SPA refresh only the token remains, and this restores the account display; with auth disabled user is always null
pub async fn status(State(state): State<Arc<AppState>>, req: Request) -> ApiResult<Json<Value>> {
	let enabled = state.auth.enabled().await;
	let user = match enabled {
		true => bearer_user(&state, presented_token(&req)).await?,
		false => None,
	};
	Ok(Json(json!({
		"enabled": enabled,
		"user": user.map(|u| json!({ "id": u.id, "username": u.username, "role": u.role })),
	})))
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
	// Also sweep expired tokens along the way, so the table grows only with active sessions
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

/// Revokes the presented bearer; missing credential/unknown token also gets 204, not leaking token validity
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

/// Password change: self only; after verifying the old one and storing the new, revoke all own session tokens (a password change logs out every client)
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
		state.auth.set_admin_password_hash(new_hash).await;
	}
	state.auth.invalidate_basic_cache().await;
	Ok(StatusCode::NO_CONTENT)
}

/// OPDS Basic verifies real username+password (§9 keeps Basic semantics); a cache hit avoids per-request argon2
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
