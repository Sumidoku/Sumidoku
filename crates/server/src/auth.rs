use crate::error::{ApiError, ApiResult, Error};
use crate::state::AppState;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
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
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
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
	argon()
		.hash_password(password.as_bytes())
		.map(|h| h.to_string())
		.map_err(|e| Error::Auth(format!("hash password: {e}")))
}

fn verify_password(hash: &str, password: &str) -> bool {
	#[cfg(test)]
	ARGON_VERIFICATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
	argon().verify_password(password.as_bytes(), hash).is_ok()
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
	login_throttle: LoginThrottle,
}

/// Failed logins a client may make before each further attempt has to wait
const LOGIN_FREE_FAILURES: u32 = 5;
/// The wait doubles per failure past the free ones and stops here, so a guesser is held to about one try a minute
/// while an admin who mistyped is never locked out for long
const LOGIN_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A client's failures are forgotten this long after its last attempt
const LOGIN_FAILURE_RESET: Duration = Duration::from_secs(15 * 60);
/// Bounds the table an address-rotating client could otherwise grow
const LOGIN_CLIENTS_MAX: usize = 4096;

/// Per-client login backoff. The client is the TCP peer address: forwarded headers are not trusted, since without a
/// configured proxy anyone can set them, so behind a reverse proxy every client shares the proxy's entry
#[derive(Default)]
struct LoginThrottle {
	clients: Mutex<HashMap<Option<IpAddr>, LoginFailures>>,
}

struct LoginFailures {
	count: u32,
	last: Instant,
}

impl LoginThrottle {
	fn backoff(count: u32) -> Option<Duration> {
		let over = count.checked_sub(LOGIN_FREE_FAILURES)?;
		Some(Duration::from_secs(1u64 << over.min(16)).min(LOGIN_BACKOFF_MAX))
	}

	/// Counts the attempt as a failure up front, in the same step as the check, so concurrent attempts cannot all
	/// pass before the first one is recorded; `Err` carries the seconds left to wait
	fn begin(&self, client: Option<IpAddr>, now: Instant) -> Result<(), u64> {
		let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
		if let Some(failures) = clients.get(&client) {
			let elapsed = now.saturating_duration_since(failures.last);
			if elapsed < LOGIN_FAILURE_RESET
				&& let Some(backoff) = Self::backoff(failures.count)
				&& elapsed < backoff
			{
				return Err((backoff - elapsed).as_secs_f64().ceil() as u64);
			}
		}
		if !clients.contains_key(&client) && clients.len() >= LOGIN_CLIENTS_MAX {
			clients.retain(|_, failures| now.saturating_duration_since(failures.last) < LOGIN_FAILURE_RESET);
			if clients.len() >= LOGIN_CLIENTS_MAX
				&& let Some(oldest) = clients.iter().min_by_key(|(_, failures)| failures.last).map(|(key, _)| *key)
			{
				clients.remove(&oldest);
			}
		}
		let failures = clients.entry(client).or_insert(LoginFailures { count: 0, last: now });
		if now.saturating_duration_since(failures.last) >= LOGIN_FAILURE_RESET {
			failures.count = 0;
		}
		failures.count += 1;
		failures.last = now;
		Ok(())
	}

	fn succeeded(&self, client: Option<IpAddr>) {
		self.clients.lock().unwrap_or_else(PoisonError::into_inner).remove(&client);
	}
}

/// An IPv6 client usually holds a whole /64, so the backoff keys on that prefix rather than the single address
fn login_client(peer: Option<SocketAddr>) -> Option<IpAddr> {
	Some(match peer?.ip().to_canonical() {
		IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
		v4 => v4,
	})
}

impl AuthState {
	pub fn new(username: &str) -> Self {
		Self {
			admin_username: username.to_string(),
			admin_password_hash: RwLock::new(None),
			login_throttle: LoginThrottle::default(),
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

pub async fn login(
	State(state): State<Arc<AppState>>,
	peer: Option<Extension<axum::extract::ConnectInfo<SocketAddr>>>,
	uri: Uri,
	headers: HeaderMap,
	Json(body): Json<LoginRequest>,
) -> ApiResult<Response> {
	if !state.auth.enabled().await {
		return Err(ApiError::bad_request("authentication is not enabled"));
	}
	let client = login_client(peer.map(|Extension(axum::extract::ConnectInfo(addr))| addr));
	if let Err(wait) = state.auth.login_throttle.begin(client, Instant::now()) {
		let mut response = ApiError::too_many_requests("too many failed logins; try again later").into_response();
		response.headers_mut().insert(axum::http::header::RETRY_AFTER, HeaderValue::from(wait));
		return Ok(response);
	}
	let Some(user) = verify_user_credentials(&state.pool, &body.username, &body.password).await? else {
		return Err(ApiError::unauthorized());
	};
	state.auth.login_throttle.succeeded(client);
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

	/// Stored hashes outlive the argon2 crate: this one was written by argon2 0.5.3 with the login
	/// parameters, and an upgrade that stops verifying it locks every existing user out
	#[test]
	fn a_hash_from_an_older_argon2_release_still_verifies() {
		let stored = "$argon2id$v=19$m=7168,t=3,p=1$EjrR/7kbk2lV5C7QS8w1TA$UXKMauY9bI8OoQtqorA2tgGySpD2NmyefOsGLN/Qvfc";
		assert!(verify_password(stored, "legacy-password"));
		assert!(!verify_password(stored, "wrong-password"));
		assert!(!verify_password("not a phc string", "legacy-password"));
	}

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

	#[test]
	fn login_backoff_starts_after_the_free_failures_and_is_capped() {
		let throttle = LoginThrottle::default();
		let client = Some(IpAddr::from([192, 0, 2, 1]));
		let start = Instant::now();
		for _ in 0..LOGIN_FREE_FAILURES {
			assert_eq!(throttle.begin(client, start), Ok(()));
		}
		assert_eq!(throttle.begin(client, start), Err(1), "the attempt past the free failures waits a second");
		assert_eq!(throttle.begin(Some(IpAddr::from([192, 0, 2, 2])), start), Ok(()), "other clients are not held");
		let mut now = start;
		for _ in 0..12 {
			now += LOGIN_BACKOFF_MAX;
			assert_eq!(throttle.begin(client, now), Ok(()));
		}
		assert_eq!(throttle.begin(client, now), Err(LOGIN_BACKOFF_MAX.as_secs()));
		assert_eq!(throttle.begin(client, now + LOGIN_FAILURE_RESET), Ok(()), "old failures are forgotten");
		assert_eq!(throttle.begin(client, now + LOGIN_FAILURE_RESET), Ok(()));
	}

	#[test]
	fn a_successful_login_clears_the_backoff() {
		let throttle = LoginThrottle::default();
		let client = Some(IpAddr::from([192, 0, 2, 1]));
		let now = Instant::now();
		for _ in 0..LOGIN_FREE_FAILURES {
			throttle.begin(client, now).expect("free attempt");
		}
		throttle.succeeded(client);
		assert_eq!(throttle.begin(client, now), Ok(()));
	}

	#[test]
	fn the_client_table_stays_bounded() {
		let throttle = LoginThrottle::default();
		let now = Instant::now();
		for i in 0..LOGIN_CLIENTS_MAX as u32 + 10 {
			throttle.begin(Some(IpAddr::from(i.to_be_bytes())), now).expect("first attempt");
		}
		assert_eq!(throttle.clients.lock().expect("lock").len(), LOGIN_CLIENTS_MAX);
	}

	#[test]
	fn ipv6_clients_share_their_64_prefix() {
		let a: SocketAddr = "[2001:db8::1]:1000".parse().expect("addr");
		let b: SocketAddr = "[2001:db8::ffff:2]:2000".parse().expect("addr");
		let mapped: SocketAddr = "[::ffff:192.0.2.1]:3000".parse().expect("addr");
		assert_eq!(login_client(Some(a)), login_client(Some(b)));
		assert_eq!(login_client(Some(mapped)), Some(IpAddr::from([192, 0, 2, 1])));
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
