//! Per-user tracker OAuth. Providers only accept pre-registered redirect URIs,
//! which a self-hosted instance cannot have, so authorization codes come back
//! through the project-hosted relay (the `Sumidoku/oauth` GitHub Pages site):
//! the provider returns to the relay and the relay bounces the browser to the
//! instance callback carried in the state. The instance-side nonce check (the
//! pending table) is what validates the flow.
//!
//! Token rows are plaintext (see migration 0027). Refresh is lazy at use time
//! and serialized; every conditional write guards on the access token it saw,
//! so a racing logout/login or a second refresh never clobbers the other.

use super::{Provider, TrackerError, dao};
use base64::Engine as _;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

pub const OAUTH_RELAY_ROOT: &str = "https://sumidoku.github.io/oauth";
pub const NATIVE_OAUTH_CALLBACK_ROOT: &str = "/api/v1/trackers/oauth";

pub fn relay_callback_url(relay_root: &str, provider: Provider) -> String {
	format!("{}/{}/callback/", relay_root.trim_end_matches('/'), provider.as_str())
}

pub fn native_callback_url(instance_origin: &str, provider: Provider) -> String {
	format!("{}{}/{}/callback", instance_origin.trim_end_matches('/'), NATIVE_OAUTH_CALLBACK_ROOT, provider.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PkceMethod {
	None,
	Plain,
	S256,
}

#[derive(Debug, Clone)]
pub(crate) struct OAuthApp {
	provider: Provider,
	pub(crate) client_id: Option<String>,
	pub(crate) client_secret: Option<String>,
	pub(crate) authorize_url: String,
	pub(crate) token_url: String,
	pub(crate) userinfo_url: String,
	pkce: PkceMethod,
	scope: Option<&'static str>,
	refresh_with_redirect: bool,
	client_id_env: &'static str,
	client_secret_env: &'static str,
}

fn env_value(value: Option<&'static str>) -> Option<String> {
	value.map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

impl OAuthApp {
	pub(crate) fn for_provider(provider: Provider) -> Self {
		match provider {
			Provider::Anilist => Self {
				provider,
				client_id: env_value(option_env!("SUMIDOKU_OAUTH_ANILIST_CLIENT_ID")),
				client_secret: env_value(option_env!("SUMIDOKU_OAUTH_ANILIST_CLIENT_SECRET")),
				authorize_url: "https://anilist.co/api/v2/oauth/authorize".into(),
				token_url: "https://anilist.co/api/v2/oauth/token".into(),
				userinfo_url: "https://graphql.anilist.co/".into(),
				pkce: PkceMethod::S256,
				scope: None,
				refresh_with_redirect: false,
				client_id_env: "SUMIDOKU_OAUTH_ANILIST_CLIENT_ID",
				client_secret_env: "SUMIDOKU_OAUTH_ANILIST_CLIENT_SECRET",
			},
			Provider::Mal => Self {
				provider,
				client_id: env_value(option_env!("SUMIDOKU_OAUTH_MAL_CLIENT_ID")),
				client_secret: env_value(option_env!("SUMIDOKU_OAUTH_MAL_CLIENT_SECRET")),
				authorize_url: "https://myanimelist.net/v1/oauth2/authorize".into(),
				token_url: "https://myanimelist.net/v1/oauth2/token".into(),
				userinfo_url: "https://api.myanimelist.net/v2/users/@me".into(),
				pkce: PkceMethod::Plain,
				scope: None,
				refresh_with_redirect: false,
				client_id_env: "SUMIDOKU_OAUTH_MAL_CLIENT_ID",
				client_secret_env: "SUMIDOKU_OAUTH_MAL_CLIENT_SECRET",
			},
			Provider::Bangumi => Self {
				provider,
				client_id: env_value(option_env!("SUMIDOKU_OAUTH_BANGUMI_CLIENT_ID")),
				client_secret: env_value(option_env!("SUMIDOKU_OAUTH_BANGUMI_CLIENT_SECRET")),
				authorize_url: "https://bgm.tv/oauth/authorize".into(),
				token_url: "https://bgm.tv/oauth/access_token".into(),
				userinfo_url: "https://api.bgm.tv/v0/me".into(),
				pkce: PkceMethod::None,
				scope: None,
				refresh_with_redirect: true,
				client_id_env: "SUMIDOKU_OAUTH_BANGUMI_CLIENT_ID",
				client_secret_env: "SUMIDOKU_OAUTH_BANGUMI_CLIENT_SECRET",
			},
			Provider::Mangabaka => Self {
				provider,
				client_id: env_value(option_env!("SUMIDOKU_OAUTH_MANGABAKA_CLIENT_ID")),
				client_secret: env_value(option_env!("SUMIDOKU_OAUTH_MANGABAKA_CLIENT_SECRET")),
				authorize_url: "https://mangabaka.org/auth/oauth2/authorize".into(),
				token_url: "https://mangabaka.org/auth/oauth2/token".into(),
				userinfo_url: "https://api.mangabaka.org/v1/my/profile".into(),
				pkce: PkceMethod::S256,
				scope: Some("library.read library.write profile offline_access openid"),
				refresh_with_redirect: true,
				client_id_env: "SUMIDOKU_OAUTH_MANGABAKA_CLIENT_ID",
				client_secret_env: "SUMIDOKU_OAUTH_MANGABAKA_CLIENT_SECRET",
			},
		}
	}

	fn credentials(&self) -> Result<(&str, &str), TrackerError> {
		let client_id = self.client_id.as_deref().ok_or(TrackerError::MissingClientId {
			provider: self.provider,
			variable: self.client_id_env,
		})?;
		let client_secret = self.client_secret.as_deref().ok_or(TrackerError::MissingClientSecret {
			provider: self.provider,
			variable: self.client_secret_env,
		})?;
		Ok((client_id, client_secret))
	}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OAuthStatus {
	pub logged_in: bool,
	pub username: Option<String>,
}

#[derive(Serialize)]
struct AuthorizeState<'a> {
	#[serde(rename = "redirectUrl")]
	redirect_url: &'a str,
	nonce: &'a str,
}

#[derive(Deserialize)]
struct CallbackState {
	nonce: String,
}

#[derive(Deserialize)]
struct TokenResponse {
	access_token: String,
	#[serde(default)]
	refresh_token: Option<String>,
	#[serde(default)]
	token_type: Option<String>,
	#[serde(default)]
	expires_in: Option<i64>,
}

fn now() -> i64 {
	std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

fn random_token() -> String {
	let mut bytes = [0u8; 32];
	OsRng.fill_bytes(&mut bytes);
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct OAuthManager {
	pool: SqlitePool,
	http: reqwest::Client,
	relay_root: String,
	apps: HashMap<Provider, OAuthApp>,
	refresh_lock: tokio::sync::Mutex<()>,
}

impl OAuthManager {
	pub fn new(pool: SqlitePool, http: reqwest::Client) -> Arc<Self> {
		Self::with_relay_root(pool, http, OAUTH_RELAY_ROOT.to_string())
	}

	pub fn with_relay_root(pool: SqlitePool, http: reqwest::Client, relay_root: String) -> Arc<Self> {
		let apps = Provider::ALL.into_iter().map(|provider| (provider, OAuthApp::for_provider(provider))).collect();
		Arc::new(Self { pool, http, relay_root, apps, refresh_lock: tokio::sync::Mutex::new(()) })
	}

	#[cfg(test)]
	pub(crate) fn with_apps(
		pool: SqlitePool,
		http: reqwest::Client,
		relay_root: String,
		apps: HashMap<Provider, OAuthApp>,
	) -> Arc<Self> {
		Arc::new(Self { pool, http, relay_root, apps, refresh_lock: tokio::sync::Mutex::new(()) })
	}

	fn app(&self, provider: Provider) -> &OAuthApp {
		self.apps.get(&provider).expect("every provider has an OAuth application")
	}

	fn user_agent() -> String {
		concat!("sumidoku/", env!("CARGO_PKG_VERSION")).to_string()
	}

	/// Builds the provider authorize URL and records the pending handshake.
	/// The instance callback travels inside the state because the provider only
	/// knows the fixed relay URL.
	pub async fn start(&self, provider: Provider, user_id: i64, instance_origin: &str) -> Result<String, TrackerError> {
		let app = self.app(provider);
		let (client_id, _) = app.credentials()?;
		let verifier = random_token();
		let nonce = random_token();
		let callback_url = native_callback_url(instance_origin, provider);
		let state = serde_json::to_string(&AuthorizeState { redirect_url: &callback_url, nonce: &nonce }).map_err(|error| {
			TrackerError::Provider { provider, message: format!("could not encode OAuth state: {error}") }
		})?;
		let challenge = match app.pkce {
			PkceMethod::None => None,
			PkceMethod::Plain => Some(verifier.clone()),
			PkceMethod::S256 => {
				Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes())))
			}
		};
		let mut url = reqwest::Url::parse(&app.authorize_url).map_err(|error| TrackerError::Provider {
			provider,
			message: format!("invalid OAuth authorize URL: {error}"),
		})?;
		url.query_pairs_mut()
			.append_pair("client_id", client_id)
			.append_pair("response_type", "code")
			.append_pair("redirect_uri", &relay_callback_url(&self.relay_root, provider))
			.append_pair("state", &state);
		if let Some(scope) = app.scope {
			url.query_pairs_mut().append_pair("scope", scope);
		}
		if let Some(challenge) = challenge {
			url.query_pairs_mut().append_pair("code_challenge", &challenge).append_pair(
				"code_challenge_method",
				match app.pkce {
					PkceMethod::S256 => "S256",
					PkceMethod::Plain => "plain",
					PkceMethod::None => unreachable!(),
				},
			);
		}
		dao::insert_pending(&self.pool, provider, &nonce, user_id, &verifier).await?;
		Ok(url.to_string())
	}

	pub async fn status(&self, provider: Provider, user_id: i64) -> Result<OAuthStatus, TrackerError> {
		let token = dao::get_token(&self.pool, provider, user_id).await?;
		Ok(OAuthStatus { logged_in: token.is_some(), username: token.and_then(|token| token.username) })
	}

	/// Reports the login state, backfilling a missing display name from the
	/// provider. MangaBaka display names are profile data, not stable login
	/// handles, so they are refreshed on every read.
	pub async fn status_with_backfill(&self, provider: Provider, user_id: i64) -> Result<OAuthStatus, TrackerError> {
		let status = self.status(provider, user_id).await?;
		if !status.logged_in {
			return Ok(status);
		}
		if provider != Provider::Mangabaka && status.username.is_some() {
			return Ok(status);
		}
		let Some(access_token) = self.access_token(provider, user_id).await? else {
			return self.status(provider, user_id).await;
		};
		let Some(username) = self.fetch_username(provider, &access_token).await else {
			return self.status(provider, user_id).await;
		};
		dao::set_username_if_token_matches(&self.pool, provider, user_id, &access_token, &username).await?;
		self.status(provider, user_id).await
	}

	pub async fn logout(&self, provider: Provider, user_id: i64) -> Result<(), TrackerError> {
		dao::delete_token(&self.pool, provider, user_id).await?;
		dao::delete_pending_for_user(&self.pool, provider, user_id).await?;
		Ok(())
	}

	/// A logout racing an in-flight callback or refresh must not delete the row
	/// the other side just created: only the pre-logout token is removed.
	pub(crate) async fn logout_if_current(&self, provider: Provider, user_id: i64, expected_access_token: &str) -> Result<(), TrackerError> {
		let deleted = dao::delete_token_if_matches(&self.pool, provider, user_id, expected_access_token).await?;
		if deleted {
			dao::delete_pending_for_user(&self.pool, provider, user_id).await?;
		}
		Ok(())
	}

	pub async fn handle_callback(&self, provider: Provider, code: &str, state_raw: &str) -> Result<(), TrackerError> {
		let state: CallbackState = serde_json::from_str(state_raw).map_err(|_| TrackerError::InvalidState)?;
		let (user_id, verifier) = dao::consume_pending(&self.pool, provider, &state.nonce).await?.ok_or(TrackerError::PendingExpired)?;
		let app = self.app(provider);
		let (client_id, client_secret) = app.credentials()?;
		let mut form = vec![
			("grant_type", "authorization_code".to_string()),
			("code", code.to_string()),
			("redirect_uri", relay_callback_url(&self.relay_root, provider)),
			("client_id", client_id.to_string()),
			("client_secret", client_secret.to_string()),
		];
		if app.pkce != PkceMethod::None {
			form.push(("code_verifier", verifier));
		}
		let mut token = self.exchange(provider, &form).await?;
		token.username = self.fetch_username(provider, &token.access_token).await;
		dao::upsert_token(&self.pool, provider, user_id, &token).await?;
		Ok(())
	}

	async fn exchange(&self, provider: Provider, form: &[(&str, String)]) -> Result<dao::TrackerOAuthToken, TrackerError> {
		let app = self.app(provider);
		let response = self
			.http
			.post(&app.token_url)
			.header(reqwest::header::USER_AGENT, Self::user_agent())
			.form(form)
			.send()
			.await
			.map_err(|error| TrackerError::Provider { provider, message: format!("OAuth token request failed: {error}") })?;
		let status = response.status();
		let body = response
			.text()
			.await
			.map_err(|error| TrackerError::Provider { provider, message: format!("OAuth token response could not be read: {error}") })?;
		if !status.is_success() {
			return Err(TrackerError::Upstream { provider, status, message: body });
		}
		let parsed: TokenResponse = serde_json::from_str(&body)
			.map_err(|error| TrackerError::Provider { provider, message: format!("OAuth token response is invalid: {error}") })?;
		if parsed.access_token.is_empty() {
			return Err(TrackerError::Provider { provider, message: "OAuth token response has an empty access token".into() });
		}
		Ok(dao::TrackerOAuthToken {
			access_token: parsed.access_token,
			refresh_token: parsed.refresh_token,
			token_type: parsed.token_type,
			expires_at: parsed.expires_in.map(|seconds| now() + seconds),
			username: None,
		})
	}

	async fn fetch_username(&self, provider: Provider, access_token: &str) -> Option<String> {
		let app = self.app(provider);
		let bearer = format!("Bearer {access_token}");
		let response = match provider {
			Provider::Anilist => self
				.http
				.post(&app.userinfo_url)
				.header(reqwest::header::AUTHORIZATION, bearer)
				.json(&serde_json::json!({ "query": "query { Viewer { name } }" }))
				.send()
				.await
				.ok()?,
			Provider::Mal | Provider::Mangabaka => {
				self.http.get(&app.userinfo_url).header(reqwest::header::AUTHORIZATION, bearer).send().await.ok()?
			}
			Provider::Bangumi => self
				.http
				.get(&app.userinfo_url)
				.header(reqwest::header::AUTHORIZATION, bearer)
				.header(reqwest::header::USER_AGENT, Self::user_agent())
				.send()
				.await
				.ok()?,
		};
		if !response.status().is_success() {
			return None;
		}
		let value = response.json::<serde_json::Value>().await.ok()?;
		match provider {
			Provider::Anilist => value.pointer("/data/Viewer/name").and_then(|value| value.as_str()).map(str::to_string),
			Provider::Mal => value.get("name").and_then(|value| value.as_str()).map(str::to_string),
			Provider::Bangumi => value.get("username").and_then(|value| value.as_str()).map(str::to_string),
			Provider::Mangabaka => ["/data/nickname", "/data/preferred_username"]
				.into_iter()
				.find_map(|pointer| value.pointer(pointer).and_then(|value| value.as_str()))
				.map(str::to_string),
		}
	}

	/// The token to call the provider with, refreshing lazily when expired. A
	/// token without a refresh token is dropped instead; a rejected refresh
	/// (400/401/403) logs the account out, a transient one keeps it.
	pub async fn access_token(&self, provider: Provider, user_id: i64) -> Result<Option<String>, TrackerError> {
		let Some(token) = dao::get_token(&self.pool, provider, user_id).await? else {
			return Ok(None);
		};
		if !expired(&token) {
			return Ok(Some(token.access_token));
		}
		if token.refresh_token.is_none() {
			self.logout_if_current(provider, user_id, &token.access_token).await?;
			return Ok(None);
		}

		let _guard = self.refresh_lock.lock().await;
		let Some(token) = dao::get_token(&self.pool, provider, user_id).await? else {
			return Ok(None);
		};
		if !expired(&token) {
			return Ok(Some(token.access_token));
		}
		let expected_access_token = token.access_token.clone();
		match self.do_refresh(provider, user_id).await? {
			Some(access_token) => Ok(Some(access_token)),
			None => {
				self.logout_if_current(provider, user_id, &expected_access_token).await?;
				Ok(None)
			}
		}
	}

	/// Refreshes immediately (the manual reconnect path); if another caller
	/// already refreshed while this one waited on the lock, its token is used.
	pub async fn refresh_now(&self, provider: Provider, user_id: i64) -> Result<Option<String>, TrackerError> {
		let seen = dao::get_token(&self.pool, provider, user_id).await?.map(|token| token.access_token);
		let _guard = self.refresh_lock.lock().await;
		let Some(token) = dao::get_token(&self.pool, provider, user_id).await? else {
			return Ok(None);
		};
		if Some(token.access_token.as_str()) != seen.as_deref() {
			return Ok(Some(token.access_token));
		}
		self.do_refresh(provider, user_id).await
	}

	async fn do_refresh(&self, provider: Provider, user_id: i64) -> Result<Option<String>, TrackerError> {
		let Some(token) = dao::get_token(&self.pool, provider, user_id).await? else {
			return Ok(None);
		};
		let Some(refresh_token) = token.refresh_token.clone() else {
			return Ok(None);
		};
		let app = self.app(provider);
		let (client_id, client_secret) = app.credentials()?;
		let mut form = vec![
			("grant_type", "refresh_token".to_string()),
			("refresh_token", refresh_token.clone()),
			("client_id", client_id.to_string()),
			("client_secret", client_secret.to_string()),
		];
		if app.refresh_with_redirect {
			form.push(("redirect_uri", relay_callback_url(&self.relay_root, provider)));
		}
		let expected_access_token = token.access_token.clone();
		let mut fresh = match self.exchange(provider, &form).await {
			Ok(token) => token,
			Err(TrackerError::Upstream {
				status: reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN,
				..
			}) => {
				tracing::warn!("OAuth refresh was rejected for {provider}");
				return Ok(None);
			}
			Err(error) => return Err(error),
		};
		if fresh.refresh_token.is_none() {
			fresh.refresh_token = Some(refresh_token);
		}
		fresh.username = token.username;
		let updated =
			dao::update_token_if_matches(&self.pool, provider, user_id, &expected_access_token, &fresh).await?;
		if !updated {
			return Ok(None);
		}
		Ok(Some(fresh.access_token))
	}
}

fn expired(token: &dao::TrackerOAuthToken) -> bool {
	token.expires_at.is_some_and(|expires_at| expires_at <= now())
}


#[cfg(test)]
mod tests {
	use super::*;
	use axum::extract::State as AxumState;
	use axum::http::StatusCode;
	use axum::{Json, Router};
	use std::sync::atomic::{AtomicUsize, Ordering};

	#[derive(Clone)]
	struct TokenEndpoint {
		hits: std::sync::Arc<AtomicUsize>,
		mode: std::sync::Arc<std::sync::Mutex<TokenMode>>,
	}

	#[derive(Clone, Copy)]
	enum TokenMode {
		RefreshNoNewRefreshToken,
		Rejected,
		Transient,
	}

	async fn token_handler(
		AxumState(endpoint): AxumState<TokenEndpoint>,
		body: axum::body::Bytes,
	) -> (StatusCode, Json<serde_json::Value>) {
		endpoint.hits.fetch_add(1, Ordering::SeqCst);
		let form: std::collections::HashMap<String, String> =
			url::form_urlencoded::parse(&body).into_owned().collect();
		assert_eq!(form["grant_type"], "refresh_token");
		assert_eq!(form["refresh_token"], "refresh-old");
		match *endpoint.mode.lock().unwrap() {
			TokenMode::RefreshNoNewRefreshToken => (
				StatusCode::OK,
				Json(serde_json::json!({"access_token": "access-new", "token_type": "Bearer", "expires_in": 3600})),
			),
			TokenMode::Rejected => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid_grant"}))),
			TokenMode::Transient => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({}))),
		}
	}

	async fn serve(router: Router) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		format!("http://{address}")
	}

	struct Fixture {
		oauth: Arc<OAuthManager>,
		pool: SqlitePool,
		endpoint: TokenEndpoint,
		_tmp: tempfile::TempDir,
	}

	async fn fixture(mode: TokenMode) -> Fixture {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let endpoint = TokenEndpoint {
			hits: std::sync::Arc::new(AtomicUsize::new(0)),
			mode: std::sync::Arc::new(std::sync::Mutex::new(mode)),
		};
		let origin = serve(Router::new().route("/token", axum::routing::post(token_handler)).with_state(endpoint.clone())).await;
		let mut app = OAuthApp::for_provider(Provider::Bangumi);
		app.client_id = Some("id".into());
		app.client_secret = Some("secret".into());
		app.token_url = format!("{origin}/token");
		let oauth = OAuthManager::with_apps(
			pool.clone(),
			reqwest::Client::new(),
			OAUTH_RELAY_ROOT.to_string(),
			HashMap::from([(Provider::Bangumi, app)]),
		);
		Fixture { oauth, pool, endpoint, _tmp: tmp }
	}

	async fn seed(pool: &SqlitePool, expires_at: Option<i64>, refresh: bool) {
		dao::upsert_token(
			pool,
			Provider::Bangumi,
			1,
			&dao::TrackerOAuthToken {
				access_token: "access-old".into(),
				refresh_token: refresh.then(|| "refresh-old".to_string()),
				token_type: Some("Bearer".into()),
				expires_at,
				username: Some("alice".into()),
			},
		)
		.await
		.unwrap();
	}

	#[tokio::test]
	async fn access_token_refreshes_lazily_keeping_refresh_and_username() {
		let fixture = fixture(TokenMode::RefreshNoNewRefreshToken).await;
		seed(&fixture.pool, Some(now() - 60), true).await;

		let token = fixture.oauth.access_token(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(token.as_deref(), Some("access-new"));
		assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
		let stored = dao::get_token(&fixture.pool, Provider::Bangumi, 1).await.unwrap().unwrap();
		assert_eq!(stored.access_token, "access-new");
		// the provider issued no new refresh token: the old one is retained
		assert_eq!(stored.refresh_token.as_deref(), Some("refresh-old"));
		assert_eq!(stored.username.as_deref(), Some("alice"));

		// the fresh token is not expired: no second refresh happens
		let token = fixture.oauth.access_token(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(token.as_deref(), Some("access-new"));
		assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 1);
	}

	#[tokio::test]
	async fn expired_without_refresh_token_logs_out() {
		let fixture = fixture(TokenMode::RefreshNoNewRefreshToken).await;
		seed(&fixture.pool, Some(now() - 60), false).await;

		let token = fixture.oauth.access_token(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(token, None);
		assert!(dao::get_token(&fixture.pool, Provider::Bangumi, 1).await.unwrap().is_none());
		assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 0);
	}

	#[tokio::test]
	async fn rejected_refresh_logs_out() {
		let fixture = fixture(TokenMode::Rejected).await;
		seed(&fixture.pool, Some(now() - 60), true).await;

		let token = fixture.oauth.access_token(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(token, None);
		assert!(dao::get_token(&fixture.pool, Provider::Bangumi, 1).await.unwrap().is_none());
	}

	#[tokio::test]
	async fn transient_refresh_failure_keeps_the_account() {
		let fixture = fixture(TokenMode::Transient).await;
		seed(&fixture.pool, Some(now() - 60), true).await;

		let result = fixture.oauth.access_token(Provider::Bangumi, 1).await;
		assert!(matches!(result, Err(TrackerError::Upstream { status: StatusCode::INTERNAL_SERVER_ERROR, .. })));
		assert!(dao::get_token(&fixture.pool, Provider::Bangumi, 1).await.unwrap().is_some());
	}

	#[tokio::test]
	async fn refresh_now_forces_a_refresh_on_every_call() {
		let fixture = fixture(TokenMode::RefreshNoNewRefreshToken).await;
		seed(&fixture.pool, Some(now() - 60), true).await;

		// the manual path is "now" on purpose: each call refreshes, and the
		// seen-token guard only dedups a refresh that lands while a caller is
		// still waiting on the lock
		let first = fixture.oauth.refresh_now(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(first.as_deref(), Some("access-new"));
		let second = fixture.oauth.refresh_now(Provider::Bangumi, 1).await.unwrap();
		assert_eq!(second.as_deref(), Some("access-new"));
		assert_eq!(fixture.endpoint.hits.load(Ordering::SeqCst), 2);
	}
}
