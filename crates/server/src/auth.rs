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
	#[cfg(test)]
	ARGON_VERIFICATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
	let Ok(parsed) = PasswordHash::new(hash) else {
		return false;
	};
	argon().verify_password(password.as_bytes(), &parsed).is_ok()
}

#[cfg(test)]
static ARGON_VERIFICATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A real PHC string with the production parameters: verifying against it when there is no usable hash makes an
/// unknown username cost the same argon2 run as a wrong password, so response time cannot enumerate usernames
static DUMMY_HASH: std::sync::LazyLock<String> =
	std::sync::LazyLock::new(|| hash_password("sumidoku-no-such-user").expect("dummy argon2 hash"));

/// argon2 is deliberately slow CPU work: it runs off the async workers, and a small permit pool keeps a flood of
/// login attempts from occupying every blocking thread and core
static ARGON_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// `None` verifies against the dummy hash and always fails, at the same cost as a real mismatch
async fn verify_password_blocking(hash: Option<String>, password: &str) -> bool {
	let _permit = ARGON_PERMITS.acquire().await.expect("argon2 permits never close");
	let password = password.to_string();
	tokio::task::spawn_blocking(move || match hash {
		Some(hash) => verify_password(&hash, &password),
		None => {
			verify_password(&DUMMY_HASH, &password);
			false
		}
	})
	.await
	.unwrap_or(false)
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
	/// Built-in admin's login username (config-side; "admin" when unset)
	admin_username: String,
	/// In-memory copy of user 1's argon2 PHC string; None = auth disabled (no password configured)
	admin_password_hash: RwLock<Option<String>>,
}

impl AuthState {
	pub fn new(username: &str) -> Self {
		Self {
			admin_username: username.to_string(),
			admin_password_hash: RwLock::new(None),
		}
	}

	/// Built-in admin (user 1): passwordless deployments act as admin
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
	format!("{SESSION_COOKIE}={token}; Path=/; Max-Age={TOKEN_TTL_SECS}; HttpOnly; SameSite=Lax{secure}")
}

fn session_cookie_clear(https: bool) -> String {
	let secure = if https { "; Secure" } else { "" };
	format!("{SESSION_COOKIE}=deleted; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}")
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
	let Some((id, username, hash)) = row.filter(|(_, _, hash)| !hash.is_empty()) else {
		verify_password_blocking(None, password).await;
		return Ok(None);
	};
	if !verify_password_blocking(Some(hash), password).await {
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

#[cfg(test)]
const REVOCATION_TEST_FINGERPRINT: &str = "revocation-test";
#[cfg(test)]
static REVOCATION_TEST_CHECKED: tokio::sync::Notify = tokio::sync::Notify::const_new();
#[cfg(test)]
static REVOCATION_TEST_RESUME: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// UI-managed API key: the atomic update checks existence and refreshes last_used_at without leaving a revocation window
async fn api_key_user(state: &AppState, fingerprint: &str) -> ApiResult<Option<AuthUser>> {
	#[cfg(test)]
	if fingerprint == REVOCATION_TEST_FINGERPRINT {
		REVOCATION_TEST_CHECKED.notify_one();
		REVOCATION_TEST_RESUME.notified().await;
	}
	let hit: Option<(String,)> = sqlx::query_as("UPDATE api_key SET last_used_at = ? WHERE key_hash = ? RETURNING key_hash")
		.bind(crate::model::unix_now())
		.bind(fingerprint)
		.fetch_optional(&state.pool)
		.await?;
	if hit.is_none() {
		return Ok(None);
	}
	let row: Option<(String,)> = sqlx::query_as("SELECT username FROM user WHERE id = 1")
		.fetch_optional(&state.pool)
		.await?;
	Ok(row.map(|(username,)| AuthUser { id: 1, username }))
}

async fn bearer_user(state: &AppState, token: Option<String>) -> ApiResult<Option<AuthUser>> {
	let Some(token) = token else {
		return Ok(None);
	};
	let fingerprint = sha256_hex(&token);
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
	if hash.is_empty() || !verify_password_blocking(Some(hash), &body.current_password).await {
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

#[cfg(test)]
mod tests {
	use super::*;

	/// An unknown username must pay the same argon2 verification as a wrong password, or response time
	/// tells an attacker which usernames exist
	#[tokio::test]
	async fn unknown_username_costs_an_argon2_verification() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let hash = hash_password("right").expect("hash");
		sqlx::query("UPDATE user SET username = 'admin', password_hash = ? WHERE id = 1")
			.bind(&hash)
			.execute(&pool)
			.await
			.expect("seed admin");
		let before = ARGON_VERIFICATIONS.load(std::sync::atomic::Ordering::SeqCst);
		assert!(verify_user_credentials(&pool, "nobody", "wrong").await.expect("verify").is_none());
		let unknown = ARGON_VERIFICATIONS.load(std::sync::atomic::Ordering::SeqCst) - before;
		assert!(verify_user_credentials(&pool, "admin", "wrong").await.expect("verify").is_none());
		let wrong = ARGON_VERIFICATIONS.load(std::sync::atomic::Ordering::SeqCst) - before - unknown;
		assert_eq!((unknown, wrong), (1, 1), "unknown user and wrong password must both run argon2");
		assert!(verify_user_credentials(&pool, "admin", "right").await.expect("verify").is_some());
	}

	#[tokio::test]
	async fn revocation_before_key_check_is_not_authenticated() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let (defaults_tx, _defaults_rx) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = tokio::sync::mpsc::unbounded_channel();
		let state = AppState::new(pool, crate::config::Config::default(), defaults_tx, cookie_tx);
		sqlx::query("INSERT INTO api_key (key_hash, name, created_at) VALUES (?, 'test', 1)")
			.bind(REVOCATION_TEST_FINGERPRINT)
			.execute(&state.pool)
			.await
			.expect("insert key");

		let lookup_state = state.clone();
		let lookup = tokio::spawn(async move { api_key_user(&lookup_state, REVOCATION_TEST_FINGERPRINT).await });
		REVOCATION_TEST_CHECKED.notified().await;
		sqlx::query("DELETE FROM api_key WHERE key_hash = ?")
			.bind(REVOCATION_TEST_FINGERPRINT)
			.execute(&state.pool)
			.await
			.expect("revoke key");
		REVOCATION_TEST_RESUME.notify_one();
		let user = lookup.await.expect("lookup task").expect("lookup");
		assert!(user.is_none(), "a key revoked before the check must not authenticate");
	}
}
