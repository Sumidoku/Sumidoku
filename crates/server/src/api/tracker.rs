//! Tracker REST surface (#35): bindings, ledger, remote search/state/update
//! and the OAuth handshake. The callback is browser-facing (the relay bounces
//! the provider redirect here); it stays behind the session cookie like every
//! other protected route.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::tracker::core::{RemoteState, TrackLedgerItem, TrackMode, TrackSearchItem, TrackUpdate};
use crate::tracker::oauth::OAuthStatus;
use crate::tracker::{Provider, TrackerError, TrackerService, dao};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::{Extension, Json};
use serde::Deserialize;
use std::sync::Arc;

fn parse_provider(name: &str) -> ApiResult<Provider> {
	Provider::parse(name).ok_or_else(|| ApiError::not_found(format!("tracker provider '{name}' is not supported")))
}

fn tracker_for(state: &AppState, provider: Provider) -> ApiResult<Arc<dyn TrackerService>> {
	let services = state
		.tracker
		.get()
		.ok_or_else(|| ApiError::internal("tracker services are not initialized"))?;
	services
		.get(provider)
		.ok_or_else(|| ApiError::not_found(format!("tracker provider '{provider}' is not supported")))
}

fn oauth_manager(state: &AppState) -> ApiResult<Arc<crate::tracker::oauth::OAuthManager>> {
	Ok(state.tracker.get().ok_or_else(|| ApiError::internal("tracker services are not initialized"))?.oauth().clone())
}

fn tracker_error(error: TrackerError) -> ApiError {
	match error {
		// distinct from auth's reason-free 401: the web UI keys on this code to
		// offer the connect-account flow instead of a re-login
		TrackerError::NotLoggedIn { provider } => {
			ApiError::conflict("tracker_auth", format!("{provider} tracker account is not connected"))
		}
		TrackerError::InvalidState | TrackerError::PendingExpired => ApiError::bad_request(error.to_string()),
		TrackerError::Database(error) => error.into(),
		TrackerError::MissingClientId { .. } | TrackerError::MissingClientSecret { .. } => ApiError::internal(error.to_string()),
		TrackerError::Upstream { message, .. } | TrackerError::Provider { message, .. } => ApiError::bad_gateway(message),
	}
}

async fn require_login(service: &Arc<dyn TrackerService>, provider: Provider, user_id: i64) -> ApiResult<()> {
	if service.is_logged_in(user_id).await.map_err(tracker_error)? {
		Ok(())
	} else {
		Err(tracker_error(TrackerError::NotLoggedIn { provider }))
	}
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerLinkDto {
	pub manga_id: i64,
	pub provider: String,
	pub track_id: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	pub track_mode: String,
	pub chapter_offset: i32,
	pub created_at: i64,
	pub updated_at: i64,
}

impl From<dao::TrackerLink> for TrackerLinkDto {
	fn from(link: dao::TrackerLink) -> Self {
		TrackerLinkDto {
			manga_id: link.manga_id,
			provider: link.provider,
			track_id: link.track_id,
			title: link.title,
			track_mode: link.track_mode.as_str().to_string(),
			chapter_offset: link.chapter_offset,
			created_at: link.created_at,
			updated_at: link.updated_at,
		}
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerLinkUpsertDto {
	pub provider: String,
	pub track_id: String,
	#[serde(default)]
	pub title: Option<String>,
	#[serde(default)]
	pub track_mode: Option<String>,
	#[serde(default)]
	pub chapter_offset: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerLinkPageDto {
	pub entries: Vec<TrackerLinkDto>,
	pub has_next_page: bool,
}

pub async fn list_all_tracker_links(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<super::PageQuery>,
) -> ApiResult<Json<TrackerLinkPageDto>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let offset = (i64::from(page) - 1) * crate::PAGE_SIZE;
	let mut links = dao::list_links_by_user_page(&state.pool, user.id, crate::PAGE_SIZE + 1, offset).await?;
	let has_next_page = links.len() as i64 > crate::PAGE_SIZE;
	links.truncate(crate::PAGE_SIZE as usize);
	Ok(Json(TrackerLinkPageDto {
		entries: links.into_iter().map(TrackerLinkDto::from).collect(),
		has_next_page,
	}))
}

pub async fn list_tracker_links(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(manga_id): Path<i64>,
) -> ApiResult<Json<Vec<TrackerLinkDto>>> {
	let links = dao::list_links_by_manga_and_user(&state.pool, manga_id, user.id).await?;
	Ok(Json(links.into_iter().map(TrackerLinkDto::from).collect()))
}

pub async fn put_tracker_link(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(manga_id): Path<i64>,
	Json(body): Json<TrackerLinkUpsertDto>,
) -> ApiResult<Json<TrackerLinkDto>> {
	parse_provider(&body.provider)?;
	let track_id = body.track_id.trim();
	if track_id.is_empty() {
		return Err(ApiError::bad_request("trackId must not be blank"));
	}
	let track_mode = match body.track_mode.as_deref() {
		None | Some("auto") => TrackMode::Auto,
		Some("chapter") => TrackMode::Chapter,
		Some("volume") => TrackMode::Volume,
		Some(other) => return Err(ApiError::bad_request(format!("'{other}' is not a valid track mode"))),
	};
	let chapter_offset = body.chapter_offset.unwrap_or(0);
	if !(-10000..=10000).contains(&chapter_offset) {
		return Err(ApiError::bad_request("chapterOffset must be between -10000 and 10000"));
	}
	dao::upsert_link(
		&state.pool,
		&dao::NewTrackerLink {
			manga_id,
			user_id: user.id,
			provider: &body.provider,
			track_id,
			title: body.title.as_deref().map(str::trim).filter(|t| !t.is_empty()),
			track_mode,
			chapter_offset,
		},
	)
	.await?;
	let links = dao::list_links_by_manga_and_user(&state.pool, manga_id, user.id).await?;
	let link = links.into_iter().find(|l| l.provider == body.provider).expect("upserted binding is readable");
	Ok(Json(TrackerLinkDto::from(link)))
}

pub async fn delete_tracker_link(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path((manga_id, provider)): Path<(i64, String)>,
) -> ApiResult<StatusCode> {
	let provider = parse_provider(&provider)?;
	dao::delete_link(&state.pool, manga_id, user.id, provider).await?;
	Ok(StatusCode::NO_CONTENT)
}

pub async fn tracker_ledger(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Vec<TrackLedgerItem>>> {
	Ok(Json(dao::list_ledger(&state.pool, user.id).await?))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
	name: String,
	nsfw: Option<bool>,
}

pub async fn tracker_search(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
	Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Vec<TrackSearchItem>>> {
	let provider = parse_provider(&provider)?;
	let service = tracker_for(&state, provider)?;
	require_login(&service, provider, user.id).await?;
	let results = service.search(user.id, &query.name, query.nsfw.unwrap_or(true)).await.map_err(tracker_error)?;
	Ok(Json(results))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateQuery {
	track_id: String,
}

pub async fn tracker_state(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
	Query(query): Query<StateQuery>,
) -> ApiResult<Json<RemoteState>> {
	let provider = parse_provider(&provider)?;
	let service = tracker_for(&state, provider)?;
	require_login(&service, provider, user.id).await?;
	let remote = service.get_state(user.id, &query.track_id).await.map_err(tracker_error)?;
	Ok(Json(remote))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerUpdateRequestDto {
	pub track_id: String,
	#[serde(flatten)]
	pub update: TrackUpdate,
}

pub async fn tracker_update(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
	Json(body): Json<TrackerUpdateRequestDto>,
) -> ApiResult<StatusCode> {
	let provider = parse_provider(&provider)?;
	let service = tracker_for(&state, provider)?;
	require_login(&service, provider, user.id).await?;
	service.update(user.id, &body.track_id, &body.update).await.map_err(tracker_error)?;
	dao::upsert_ledger(&state.pool, provider, user.id, &body.track_id, None, tracker_entry_url(provider, &body.track_id).as_deref(), None)
		.await?;
	Ok(StatusCode::NO_CONTENT)
}

fn tracker_entry_url(provider: Provider, track_id: &str) -> Option<String> {
	match provider {
		Provider::Anilist => Some(format!("https://anilist.co/manga/{track_id}")),
		Provider::Mal => Some(format!("https://myanimelist.net/manga/{track_id}")),
		Provider::Bangumi => Some(format!("https://bgm.tv/subject/{track_id}")),
		Provider::Mangabaka => Some(format!("https://mangabaka.org/{track_id}")),
	}
}

pub async fn tracker_oauth_start(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
	headers: HeaderMap,
) -> ApiResult<Redirect> {
	let provider = parse_provider(&provider)?;
	let origin = instance_origin(&headers)?;
	let authorize_url = oauth_manager(&state)?.start(provider, user.id, &origin).await.map_err(tracker_error)?;
	Ok(Redirect::to(&authorize_url))
}

pub async fn tracker_oauth_status(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
) -> ApiResult<Json<OAuthStatus>> {
	let provider = parse_provider(&provider)?;
	let status = oauth_manager(&state)?.status_with_backfill(provider, user.id).await.map_err(tracker_error)?;
	Ok(Json(status))
}

pub async fn tracker_oauth_logout(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
) -> ApiResult<StatusCode> {
	let provider = parse_provider(&provider)?;
	oauth_manager(&state)?.logout(provider, user.id).await.map_err(tracker_error)?;
	Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
	code: String,
	state: String,
}

pub async fn tracker_oauth_callback(
	State(state): State<Arc<AppState>>,
	Extension(_user): Extension<crate::auth::AuthUser>,
	Path(provider): Path<String>,
	Query(query): Query<CallbackQuery>,
) -> ApiResult<Redirect> {
	let provider = parse_provider(&provider)?;
	let target = match oauth_manager(&state)?.handle_callback(provider, &query.code, &query.state).await {
		Ok(()) => "/settings?section=trackers&oauth=success".to_string(),
		Err(error) => {
			tracing::warn!("tracker OAuth callback failed for {provider}: {error}");
			let message = url::form_urlencoded::byte_serialize(error.to_string().as_bytes()).collect::<String>();
			format!("/settings?section=trackers&oauth=error&message={message}")
		}
	};
	Ok(Redirect::to(&target))
}

/// The public origin of this instance, needed for the OAuth state so the
/// relay can bounce the provider callback back here. Forwarded headers win
/// because a reverse proxy is the common deployment.
fn instance_origin(headers: &HeaderMap) -> ApiResult<String> {
	let scheme = headers
		.get("x-forwarded-proto")
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.split(',').next())
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.unwrap_or("http");
	let host = headers
		.get("x-forwarded-host")
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.split(',').next())
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.or_else(|| headers.get(axum::http::header::HOST).and_then(|value| value.to_str().ok()))
		.ok_or_else(|| ApiError::bad_request("cannot determine instance host"))?;
	Ok(format!("{scheme}://{host}"))
}


#[cfg(test)]
mod tests {
	use super::*;
	use crate::tracker::TrackerServices;
	use crate::tracker::oauth::{OAuthApp, OAuthManager};
	use axum::Router;
	use axum::body::Body;
	use axum::http::Request;
	use std::collections::HashMap;
	use tower::ServiceExt as _;

	async fn serve(router: Router) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		format!("http://{address}")
	}

	async fn mock_oauth_server() -> String {
		let app = Router::new()
			.route(
				"/token/anilist",
				axum::routing::post(|| async {
					Json(serde_json::json!({
						"access_token": "access-token",
						"refresh_token": "refresh-token",
						"token_type": "Bearer",
						"expires_in": 3600
					}))
				}),
			)
			.route(
				"/userinfo/anilist",
				axum::routing::post(|| async { Json(serde_json::json!({"data": {"Viewer": {"name": "alice"}}})) }),
			);
		serve(app).await
	}

	async fn test_app(origin: &str) -> (Router, sqlx::SqlitePool, tempfile::TempDir) {
		test_app_with_password(origin, None).await
	}

	async fn test_app_with_password(origin: &str, password: Option<&str>) -> (Router, sqlx::SqlitePool, tempfile::TempDir) {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let (defaults_tx, _defaults_rx) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = tokio::sync::mpsc::unbounded_channel();
		let mut config = crate::config::Config { config_dir: tmp.path().to_path_buf(), ..crate::config::Config::default() };
		config.auth.password = password.map(str::to_string);
		let state = AppState::new(pool.clone(), config, defaults_tx, cookie_tx);
		crate::auth::init(&state).await.unwrap();

		let mut app = OAuthApp::for_provider(Provider::Anilist);
		app.client_id = Some("id-anilist".into());
		app.client_secret = Some("secret".into());
		app.authorize_url = format!("{origin}/authorize/anilist");
		app.token_url = format!("{origin}/token/anilist");
		app.userinfo_url = format!("{origin}/userinfo/anilist");
		let oauth = OAuthManager::with_apps(pool, reqwest::Client::new(), crate::tracker::oauth::OAUTH_RELAY_ROOT.to_string(), HashMap::from([(Provider::Anilist, app)]));
		assert!(state.tracker.set(Arc::new(TrackerServices::new(oauth, Vec::new()))).is_ok());
		(crate::api::router(state.clone()), state.pool.clone(), tmp)
	}

	async fn get(app: &Router, path: &str, host: Option<&str>) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
		get_with_cookie(app, path, host, None).await
	}

	async fn get_with_cookie(
		app: &Router,
		path: &str,
		host: Option<&str>,
		session: Option<&str>,
	) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
		let mut builder = Request::builder().method("GET").uri(path);
		if let Some(host) = host {
			builder = builder.header("host", host);
		}
		if let Some(session) = session {
			builder = builder.header("cookie", format!("sumidoku.session={session}"));
		}
		let response = app.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
		let status = response.status();
		let headers = response.headers().clone();
		let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
		(status, headers, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
	}

	async fn login(app: &Router, password: &str) -> (String, String) {
		let request = Request::builder()
			.method("POST")
			.uri("/api/v1/auth/login")
			.header("content-type", "application/json")
			.body(Body::from(serde_json::json!({"username": "admin", "password": password}).to_string()))
			.unwrap();
		let response = app.clone().oneshot(request).await.unwrap();
		assert_eq!(response.status(), StatusCode::OK);
		let headers = response.headers().clone();
		let set_cookie = headers.get(axum::http::header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
		let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
		let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
		(body["token"].as_str().unwrap().to_string(), set_cookie)
	}

	#[tokio::test]
	async fn oauth_full_flow_and_replay_protection() {
		let origin = mock_oauth_server().await;
		let (app, _pool, _tmp) = test_app(&origin).await;

		let (status, _, body) = get(&app, "/api/v1/trackers/oauth/anilist/status", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body, serde_json::json!({"logged_in": false, "username": null}));

		let (status, headers, _) = get(&app, "/api/v1/trackers/oauth/anilist/start", Some("sumidoku.test")).await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		let location = headers.get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
		let authorize = url::Url::parse(location).unwrap();
		assert_eq!(authorize.path(), "/authorize/anilist");
		let query: HashMap<_, _> = authorize.query_pairs().into_owned().collect();
		assert_eq!(query["client_id"], "id-anilist");
		assert_eq!(query["redirect_uri"], "https://sumidoku.github.io/oauth/anilist/callback/");
		assert!(query.contains_key("code_challenge"));
		assert_eq!(query["code_challenge_method"], "S256");
		let state: serde_json::Value = serde_json::from_str(&query["state"]).unwrap();
		assert_eq!(state["redirectUrl"], "http://sumidoku.test/api/v1/trackers/oauth/anilist/callback");

		let encoded_state = url::form_urlencoded::byte_serialize(query["state"].as_bytes()).collect::<String>();
		let callback = format!("/api/v1/trackers/oauth/anilist/callback?code=ok&state={encoded_state}");
		let (status, headers, _) = get(&app, &callback, None).await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		assert_eq!(headers.get(axum::http::header::LOCATION).unwrap(), "/settings?section=trackers&oauth=success");

		let (_, _, body) = get(&app, "/api/v1/trackers/oauth/anilist/status", None).await;
		assert_eq!(body, serde_json::json!({"logged_in": true, "username": "alice"}));

		// a replayed callback finds the nonce consumed and bounces to the error page
		let (status, headers, _) = get(&app, &callback, None).await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		let location = headers.get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
		assert!(location.starts_with("/settings?section=trackers&oauth=error"), "{location}");

		let request = Request::builder()
			.method("POST")
			.uri("/api/v1/trackers/oauth/anilist/logout")
			.body(Body::empty())
			.unwrap();
		let response = app.clone().oneshot(request).await.unwrap();
		assert_eq!(response.status(), StatusCode::NO_CONTENT);
		let (_, _, body) = get(&app, "/api/v1/trackers/oauth/anilist/status", None).await;
		assert_eq!(body, serde_json::json!({"logged_in": false, "username": null}));
	}

	#[tokio::test]
	async fn oauth_callback_requires_lax_session_and_one_shot_state() {
		let origin = mock_oauth_server().await;
		let (app, pool, _tmp) = test_app_with_password(&origin, Some("secret")).await;
		let (session, set_cookie) = login(&app, "secret").await;

		let (status, _, _) = get(&app, "/api/v1/trackers/oauth/anilist/status", None).await;
		assert_eq!(status, StatusCode::UNAUTHORIZED);
		let (status, headers, _) = get_with_cookie(
			&app,
			"/api/v1/trackers/oauth/anilist/start",
			Some("sumidoku.test"),
			Some(&session),
		)
		.await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		let location = headers.get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
		let authorize = url::Url::parse(location).unwrap();
		let query: HashMap<_, _> = authorize.query_pairs().into_owned().collect();
		let encoded_state = url::form_urlencoded::byte_serialize(query["state"].as_bytes()).collect::<String>();
		let callback = format!("/api/v1/trackers/oauth/anilist/callback?code=ok&state={encoded_state}");

		let (status, _, _) = get(&app, &callback, None).await;
		assert_eq!(status, StatusCode::UNAUTHORIZED);
		let (status, headers, _) = get_with_cookie(&app, &callback, None, Some(&session)).await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		assert_eq!(headers.get(axum::http::header::LOCATION).unwrap(), "/settings?section=trackers&oauth=success");
		let token = dao::get_token(&pool, Provider::Anilist, 1).await.unwrap().expect("tracker token");
		assert_eq!(token.access_token, "access-token");

		let (status, headers, _) = get_with_cookie(&app, &callback, None, Some(&session)).await;
		assert_eq!(status, StatusCode::SEE_OTHER);
		let location = headers.get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
		assert!(location.starts_with("/settings?section=trackers&oauth=error"), "{location}");

		let request = Request::builder()
			.method("POST")
			.uri("/api/v1/auth/logout")
			.header("cookie", format!("sumidoku.session={session}"))
			.body(Body::empty())
			.unwrap();
		let response = app.clone().oneshot(request).await.unwrap();
		assert_eq!(response.status(), StatusCode::NO_CONTENT);
		let cleared = response.headers().get(axum::http::header::SET_COOKIE).unwrap().to_str().unwrap();
		assert!(set_cookie.contains("SameSite=Lax"));
		assert!(cleared.contains("SameSite=Lax"));
	}

	#[tokio::test]
	async fn oauth_status_backfills_a_missing_username() {
		let origin = mock_oauth_server().await;
		let (app, pool, _tmp) = test_app(&origin).await;
		dao::upsert_token(
			&pool,
			Provider::Anilist,
			1,
			&dao::TrackerOAuthToken {
				access_token: "access-token".into(),
				refresh_token: Some("refresh-token".into()),
				token_type: Some("Bearer".into()),
				expires_at: None,
				username: None,
			},
		)
		.await
		.unwrap();

		let (status, _, body) = get(&app, "/api/v1/trackers/oauth/anilist/status", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body, serde_json::json!({"logged_in": true, "username": "alice"}));
		let token = dao::get_token(&pool, Provider::Anilist, 1).await.unwrap().unwrap();
		assert_eq!(token.username.as_deref(), Some("alice"));
	}

}
