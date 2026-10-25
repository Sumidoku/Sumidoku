use crate::error::{ApiError, ApiResult, Error};
use crate::state::AppState;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
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

/// The login identity resolved from one request; require_auth injects it into request extensions
#[derive(Debug, Clone)]
pub struct AuthUser {
	pub id: i64,
	pub username: String,
}

pub struct AuthState {
	/// Built-in admin's login username (config-side; "admin" when unset); passwordless deployments and
	/// config-side api_keys display it without a DB lookup
	admin_username: String,
	/// In-memory copy of user 1's argon2 PHC string; None = auth disabled (no password configured)
	admin_password_hash: RwLock<Option<String>>,
	/// sha256 fingerprints of the config-side API keys, resident in memory
	api_key_hashes: HashSet<String>,
}

impl AuthState {
	pub fn new(api_keys: &[String], username: &str) -> Self {
		Self {
			admin_username: username.to_string(),
			admin_password_hash: RwLock::new(None),
			api_key_hashes: api_keys.iter().map(|k| sha256_hex(k)).collect(),
		}
	}

	/// Built-in admin (user 1): passwordless deployments and config-side api_keys both act as admin
	pub(crate) fn admin_user(&self) -> AuthUser {
		AuthUser { id: 1, username: self.admin_username.clone() }
	}

	pub async fn enabled(&self) -> bool {
		self.admin_password_hash.read().await.is_some()
	}

	pub(crate) async fn set_admin_password_hash(&self, hash: String) {
		*self.admin_password_hash.write().await = Some(hash);
	}
}

/// Establishes the admin username and password at startup: the config/env side wins (a configured username is
/// written onto user 1, a configured password overrides the stored hash), otherwise keeps what is already in
/// the DB; an empty string in the DB = auth disabled. The change-password endpoint only writes the DB; on
/// restart a config-side password still wins
pub async fn init(state: &AppState) -> Result<(), Error> {
	if state.config.auth.username.is_some() {
		sqlx::query("UPDATE user SET username = ? WHERE id = 1")
			.bind(state.config.effective_username())
			.execute(&state.pool)
			.await?;
	}
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

const SESSION_COOKIE: &str = "sumidoku.session";

fn is_https(uri: &Uri, headers: &HeaderMap) -> bool {
	uri.scheme_str() == Some("https")
		|| headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

fn session_cookie_value(token: &str, https: bool) -> String {
	let secure = if https { "; Secure" } else { "" };
	format!("{SESSION_COOKIE}={token}; Path=/; Max-Age={TOKEN_TTL_SECS}; HttpOnly; SameSite=Strict{secure}")
}

fn session_cookie_clear(https: bool) -> String {
	let secure = if https { "; Secure" } else { "" };
	format!("{SESSION_COOKIE}=deleted; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{secure}")
}

fn set_cookie_header(response: &mut Response, value: String) {
	response.headers_mut().insert(axum::http::header::SET_COOKIE, HeaderValue::from_str(&value).expect("valid cookie"));
}

/// Takes the credential from Authorization: Bearer or the browser session cookie
fn presented_token(req: &Request) -> Option<String> {
	if let Some(value) = req
		.headers()
		.get(axum::http::header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Bearer "))
	{
		return Some(value.to_string());
	}
	req.headers()
		.get(axum::http::header::COOKIE)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.split(';').find_map(|part| part.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('=')))
		.map(str::to_string)
}

/// Username+password verification: unknown user and wrong password take the same path, so externally there is only one uniform 401
async fn verify_user_credentials(
	pool: &sqlx::SqlitePool,
	username: &str,
	password: &str,
) -> ApiResult<Option<AuthUser>> {
	let row: Option<(i64, String, String)> =
		sqlx::query_as("SELECT id, username, password_hash FROM user WHERE id = 1 AND username = ?")
			.bind(username)
			.fetch_optional(pool)
			.await?;
	let Some((id, username, hash)) = row else {
		return Ok(None);
	};
	if hash.is_empty() || !verify_password(&hash, password) {
		return Ok(None);
	}
	Ok(Some(AuthUser { id, username }))
}

/// Session token: look up by sha256 fingerprint, JOIN user to verify the owner still exists; renewed on a rolling basis while unexpired
async fn session_user(state: &AppState, fingerprint: &str) -> ApiResult<Option<AuthUser>> {
	let now = crate::model::unix_now();
	let row: Option<(i64, i64, String)> = sqlx::query_as(
		"SELECT t.expires_at, u.id, u.username FROM auth_token t
		 JOIN user u ON u.id = t.user_id WHERE t.token_hash = ? AND u.id = 1",
	)
	.bind(fingerprint)
	.fetch_optional(&state.pool)
	.await?;
	let Some((expires_at, id, username)) = row else {
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
	Ok(Some(AuthUser { id, username }))
}

/// UI-managed API key: look up by sha256 fingerprint; a hit acts as the built-in admin and refreshes last_used_at
async fn api_key_user(state: &AppState, fingerprint: &str) -> ApiResult<Option<AuthUser>> {
	let hit: Option<(i64,)> = sqlx::query_as("SELECT created_at FROM api_key WHERE key_hash = ?")
		.bind(fingerprint)
		.fetch_optional(&state.pool)
		.await?;
	if hit.is_none() {
		return Ok(None);
	}
	sqlx::query("UPDATE api_key SET last_used_at = ? WHERE key_hash = ?")
		.bind(crate::model::unix_now())
		.bind(fingerprint)
		.execute(&state.pool)
		.await?;
	Ok(Some(state.auth.admin_user()))
}

async fn bearer_user(state: &AppState, token: Option<String>) -> ApiResult<Option<AuthUser>> {
	let Some(token) = token else {
		return Ok(None);
	};
	let fingerprint = sha256_hex(&token);
	if state.auth.api_key_hashes.contains(&fingerprint) {
		return Ok(Some(state.auth.admin_user()));
	}
	if let Some(user) = api_key_user(state, &fingerprint).await? {
		return Ok(Some(user));
	}
	session_user(state, &fingerprint).await
}

/// Uniform /api/v1 auth: with auth disabled every request is the built-in admin; invalid/absent credentials always get the same uniform 401
pub async fn require_auth(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
	if !state.auth.enabled().await {
		req.extensions_mut().insert(state.auth.admin_user());
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
	let mut body = json!({
		"enabled": enabled,
		"user": Value::Null,
	});
	if let Some(user) = user {
		body["username"] = Value::String(user.username.clone());
		body["user"] = json!({ "id": user.id, "username": user.username });
	}
	Ok(Json(body))
}

pub async fn login(State(state): State<Arc<AppState>>, uri: Uri, headers: HeaderMap, Json(body): Json<LoginRequest>) -> ApiResult<Response> {
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
	let mut response = Json(json!({
		"token": token,
		"expiresAt": expires_at,
		"user": { "id": user.id, "username": user.username },
	}))
	.into_response();
	set_cookie_header(&mut response, session_cookie_value(&token, is_https(&uri, &headers)));
	Ok(response)
}

/// Revokes the presented bearer; missing credential/unknown token also gets 204, not leaking token validity
pub async fn logout(State(state): State<Arc<AppState>>, req: Request) -> Response {
	if let Some(token) = presented_token(&req) {
		let _ = sqlx::query("DELETE FROM auth_token WHERE token_hash = ?")
			.bind(sha256_hex(&token))
			.execute(&state.pool)
			.await;
	}
	let mut response = StatusCode::NO_CONTENT.into_response();
	set_cookie_header(&mut response, session_cookie_clear(is_https(req.uri(), req.headers())));
	response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChangeRequest {
	pub current_password: String,
	pub new_password: String,
}

/// Password change: after verifying the old one and storing the new, revoke every admin session token
pub async fn change_password(
	State(state): State<Arc<AppState>>,
	uri: Uri,
	headers: HeaderMap,
	Extension(user): Extension<AuthUser>,
	Json(body): Json<PasswordChangeRequest>,
) -> ApiResult<Response> {
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
	state.auth.set_admin_password_hash(new_hash).await;
	let mut response = StatusCode::NO_CONTENT.into_response();
	set_cookie_header(&mut response, session_cookie_clear(is_https(&uri, &headers)));
	Ok(response)
}
