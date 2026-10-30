//! Tracker REST surface (#35): bindings, ledger, remote search/state/update
//! and the OAuth handshake. The callback is browser-facing (the relay bounces
//! the provider redirect here); it stays behind the session cookie like every
//! other protected route.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::tracker::core::{self, ChapterPullInput, RemoteState, TrackLedgerItem, TrackMode, TrackSearchItem, TrackUpdate};
use crate::tracker::oauth::OAuthStatus;
use crate::tracker::{Provider, TrackerError, TrackerService, dao};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::{Extension, Json};
use serde::Deserialize;
use std::collections::HashMap;
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
pub struct TrackerBindingDto {
	#[serde(flatten)]
	pub link: TrackerLinkDto,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub manga_title: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub manga_cover: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerBindingCountsDto {
	pub manga: i64,
	pub providers: HashMap<String, i64>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerLinkPageDto {
	pub entries: Vec<TrackerBindingDto>,
	pub has_next_page: bool,
	pub total_pages: i64,
	/// Always unfiltered, so the provider filter keeps showing every option
	pub counts: TrackerBindingCountsDto,
}

#[derive(Debug, Deserialize)]
pub struct TrackerLinkPageQuery {
	pub page: Option<i32>,
	pub provider: Option<String>,
}

// One manga can span several rows, so a page is shorter than the 50-row list default
const BINDINGS_PAGE_SIZE: i64 = 20;

pub async fn list_all_tracker_links(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<TrackerLinkPageQuery>,
) -> ApiResult<Json<TrackerLinkPageDto>> {
	let page = i64::from(query.page.unwrap_or(1));
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let provider = query
		.provider
		.as_deref()
		.map(|name| Provider::parse(name).ok_or_else(|| ApiError::bad_request(format!("tracker provider '{name}' is not supported"))))
		.transpose()?;
	let bindings = dao::list_bindings_page(&state.pool, user.id, provider, BINDINGS_PAGE_SIZE, (page - 1) * BINDINGS_PAGE_SIZE).await?;
	let (manga, providers) = dao::binding_counts(&state.pool, user.id).await?;
	let providers: HashMap<String, i64> = providers.into_iter().collect();
	let matching = provider.map_or(manga, |provider| providers.get(provider.as_str()).copied().unwrap_or(0));
	let total_pages = (matching + BINDINGS_PAGE_SIZE - 1) / BINDINGS_PAGE_SIZE;
	Ok(Json(TrackerLinkPageDto {
		entries: bindings
			.into_iter()
			.map(|binding| TrackerBindingDto {
				link: binding.link.into(),
				manga_title: binding.manga_title,
				manga_cover: binding.manga_cover,
			})
			.collect(),
		has_next_page: page < total_pages,
		total_pages,
		counts: TrackerBindingCountsDto { manga, providers },
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerSyncRequestDto {
	/// A preview only computes the plan; nothing is written and no push runs.
	#[serde(default)]
	pub preview: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerSyncResultDto {
	/// The tracker progress the chapters were matched against (0 when the
	/// tracker has no readable progress).
	pub progress: f64,
	/// How many previously unread chapters are (preview: would be) marked read.
	pub marked: usize,
	/// The ids of those chapters, oldest first.
	pub chapter_ids: Vec<i64>,
}

/// Aidoku's "Sync Local History": pull the tracker's progress onto the local
/// history of one manga. The algorithm lives in `tracker::core::plan_pull`;
/// applying it marks every matching unread chapter read (locked or not, like
/// Aidoku, unlike the batch endpoint) and then pushes the new progress to the
/// manga's other bindings, skipping the tracker it came from.
pub async fn sync_tracker_link(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path((manga_id, provider)): Path<(i64, String)>,
	Json(body): Json<TrackerSyncRequestDto>,
) -> ApiResult<Json<TrackerSyncResultDto>> {
	let provider = parse_provider(&provider)?;
	let service = tracker_for(&state, provider)?;
	require_login(&service, provider, user.id).await?;
	let links = dao::list_links_by_manga_and_user(&state.pool, manga_id, user.id).await?;
	let link = links
		.into_iter()
		.find(|link| link.provider == provider.as_str())
		.ok_or_else(|| ApiError::not_found("no tracker binding for this manga"))?;
	let remote = service.get_state(user.id, &link.track_id).await.map_err(tracker_error)?;
	let chapters = sqlx::query_as::<_, (i64, Option<f64>, Option<f64>, i64)>(
		"SELECT c.id, c.number, c.volume, COALESCE(cu.is_read, 0)
		 FROM chapter c
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
		 WHERE c.manga_id = ? AND c.trashed_at IS NULL",
	)
	.bind(user.id)
	.bind(manga_id)
	.fetch_all(&state.pool)
	.await?;
	let inputs: Vec<ChapterPullInput> = chapters
		.iter()
		.map(|&(id, number, volume, read)| ChapterPullInput { id, number, volume, read: read != 0 })
		.collect();
	let plan = core::plan_pull(link.track_mode, link.chapter_offset, &remote, &inputs);
	let planned: std::collections::HashSet<i64> = plan.chapter_ids.into_iter().collect();
	let mut to_mark: Vec<i64> = inputs.iter().filter(|c| !c.read && planned.contains(&c.id)).map(|c| c.id).collect();
	to_mark.sort_unstable();
	if !body.preview && !to_mark.is_empty() {
		let patch = super::reader::ProgressPatch { is_read: Some(true), ..Default::default() };
		let mut tx = state.pool.begin().await?;
		for chapter_id in &to_mark {
			patch.apply(&mut tx, user.id, *chapter_id).await?;
		}
		tx.commit().await?;
		super::reader::notify_tracker_read(&state, manga_id, user.id, Some(provider));
	}
	Ok(Json(TrackerSyncResultDto { progress: plan.progress, marked: to_mark.len(), chapter_ids: to_mark }))
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

	struct FixedTracker {
		remote: RemoteState,
	}

	#[async_trait::async_trait]
	impl TrackerService for FixedTracker {
		fn provider(&self) -> Provider {
			Provider::Anilist
		}

		async fn is_logged_in(&self, _user_id: i64) -> Result<bool, TrackerError> {
			Ok(true)
		}

		async fn search(&self, _user_id: i64, _query: &str, _nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
			Ok(Vec::new())
		}

		async fn get_state(&self, _user_id: i64, _track_id: &str) -> Result<RemoteState, TrackerError> {
			Ok(self.remote.clone())
		}

		async fn update(&self, _user_id: i64, _track_id: &str, _update: &TrackUpdate) -> Result<(), TrackerError> {
			Ok(())
		}
	}

	async fn test_app_with_tracker(remote: RemoteState) -> (Router, sqlx::SqlitePool, tempfile::TempDir) {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let (defaults_tx, _defaults_rx) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = tokio::sync::mpsc::unbounded_channel();
		let config = crate::config::Config { config_dir: tmp.path().to_path_buf(), ..crate::config::Config::default() };
		let state = AppState::new(pool.clone(), config, defaults_tx, cookie_tx);
		crate::auth::init(&state).await.unwrap();
		let oauth = OAuthManager::new(pool.clone(), reqwest::Client::new());
		let services = TrackerServices::new(oauth, vec![Arc::new(FixedTracker { remote })]);
		assert!(state.tracker.set(Arc::new(services)).is_ok());
		(crate::api::router(state.clone()), state.pool.clone(), tmp)
	}

	async fn seed_manga_with_chapters(pool: &sqlx::SqlitePool, numbers: &[Option<f64>]) -> (i64, Vec<i64>) {
		let source_id: i64 = sqlx::query_scalar("INSERT INTO source (key, name, wasm_path) VALUES ('en.test', 'Test', '') RETURNING id")
			.fetch_one(pool)
			.await
			.unwrap();
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'm', 'M') RETURNING id")
			.bind(source_id)
			.fetch_one(pool)
			.await
			.unwrap();
		let mut ids = Vec::new();
		for (index, number) in numbers.iter().enumerate() {
			let id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key, number) VALUES (?, ?, ?) RETURNING id")
				.bind(manga_id)
				.bind(format!("ch-{index}"))
				.bind(number)
				.fetch_one(pool)
				.await
				.unwrap();
			ids.push(id);
		}
		(manga_id, ids)
	}

	async fn post_json(app: &Router, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
		let request = Request::builder()
			.method("POST")
			.uri(path)
			.header("content-type", "application/json")
			.body(Body::from(body.to_string()))
			.unwrap();
		let response = app.clone().oneshot(request).await.unwrap();
		let status = response.status();
		let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
		(status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
	}

	/// Aidoku's "Sync Local History": the tracker is ahead, so applying marks
	/// every chapter up to its progress read (locked state does not matter),
	/// while a preview computes the same plan without writing.
	#[tokio::test]
	async fn sync_pull_marks_chapters_read_and_preview_stays_read_only() {
		let remote = RemoteState {
			status: Some(core::TrackStatus::Reading),
			last_read_chapter: Some(2.0),
			total_chapters: Some(3),
			..RemoteState::default()
		};
		let (app, pool, _tmp) = test_app_with_tracker(remote).await;
		let (manga_id, ids) = seed_manga_with_chapters(&pool, &[Some(1.0), Some(2.0), Some(3.0)]).await;
		dao::upsert_link(
			&pool,
			&dao::NewTrackerLink {
				manga_id,
				user_id: 1,
				provider: "anilist",
				track_id: "42",
				title: None,
				track_mode: TrackMode::Auto,
				chapter_offset: 0,
			},
		)
		.await
		.unwrap();

		let path = format!("/api/v1/trackers/links/{manga_id}/anilist/sync");
		let (status, body) = post_json(&app, &path, serde_json::json!({"preview": true})).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["marked"], serde_json::json!(2));
		assert_eq!(body["chapterIds"], serde_json::json!([ids[0], ids[1]]));
		let read: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE is_read = 1").fetch_one(&pool).await.unwrap();
		assert_eq!(read, 0, "preview must not write");

		let (status, body) = post_json(&app, &path, serde_json::json!({})).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["marked"], serde_json::json!(2));
		assert_eq!(body["chapterIds"], serde_json::json!([ids[0], ids[1]]));
		let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT chapter_id, is_read FROM chapter_user WHERE user_id = 1 ORDER BY chapter_id")
			.fetch_all(&pool)
			.await
			.unwrap();
		assert_eq!(rows, vec![(ids[0], 1), (ids[1], 1)]);
		// a bare is_read:true does not stamp last_read_at (batch mark semantics)
		let stamped: i64 =
			sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE last_read_at IS NOT NULL").fetch_one(&pool).await.unwrap();
		assert_eq!(stamped, 0);

		// local caught up, so the pull is forward-only and becomes a no-op
		let (status, body) = post_json(&app, &path, serde_json::json!({})).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["marked"], serde_json::json!(0));
		assert_eq!(body["progress"], serde_json::json!(0.0));
	}

	/// Without a binding there is nothing to sync from, and an unknown provider
	/// name is rejected before any remote call.
	#[tokio::test]
	async fn sync_pull_requires_a_binding_and_a_known_provider() {
		let (app, pool, _tmp) = test_app_with_tracker(RemoteState::default()).await;
		let (manga_id, _) = seed_manga_with_chapters(&pool, &[Some(1.0)]).await;
		let (status, _) = post_json(&app, &format!("/api/v1/trackers/links/{manga_id}/anilist/sync"), serde_json::json!({})).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
		let (status, _) = post_json(&app, &format!("/api/v1/trackers/links/{manga_id}/unknown/sync"), serde_json::json!({})).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
	}

	/// A tracker with no readable progress plans nothing.
	#[tokio::test]
	async fn sync_pull_without_remote_progress_is_a_noop() {
		let (app, pool, _tmp) = test_app_with_tracker(RemoteState::default()).await;
		let (manga_id, _) = seed_manga_with_chapters(&pool, &[Some(1.0), Some(2.0)]).await;
		dao::upsert_link(
			&pool,
			&dao::NewTrackerLink {
				manga_id,
				user_id: 1,
				provider: "anilist",
				track_id: "42",
				title: None,
				track_mode: TrackMode::Auto,
				chapter_offset: 0,
			},
		)
		.await
		.unwrap();
		let (status, body) =
			post_json(&app, &format!("/api/v1/trackers/links/{manga_id}/anilist/sync"), serde_json::json!({})).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["marked"], serde_json::json!(0));
		let read: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter_user WHERE is_read = 1").fetch_one(&pool).await.unwrap();
		assert_eq!(read, 0);
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
