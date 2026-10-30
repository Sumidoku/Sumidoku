//! AniList provider over the GraphQL API. Scores travel in the viewer's own
//! scale: reads request POINT_100 and both reads and writes convert through
//! the viewer's configured score format, cached per user.

use crate::tracker::core::{RemoteState, TrackMediaType, TrackSearchItem, TrackStatus, TrackUpdate};
use crate::tracker::oauth::OAuthManager;
use crate::tracker::{Provider, TrackerError, TrackerService};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

const GRAPHQL_URL: &str = "https://graphql.anilist.co";

// AniList descriptions carry <br>, <i> and <a> markup plus HTML entities, while
// the other providers return plain text
fn plain_description(html: &str) -> String {
	let fragment = scraper::Html::parse_fragment(html);
	let mut text = String::with_capacity(html.len());
	for node in fragment.root_element().descendants() {
		match node.value() {
			scraper::Node::Text(chunk) => text.push_str(chunk),
			scraper::Node::Element(element) if element.name() == "br" => text.push('\n'),
			_ => {}
		}
	}
	// <br> usually sits next to a literal newline, so blank-line runs collapse
	// to one paragraph break
	let mut plain = String::with_capacity(text.len());
	let mut blank = false;
	for line in text.lines().map(str::trim) {
		if line.is_empty() {
			blank = !plain.is_empty();
			continue;
		}
		if !plain.is_empty() {
			plain.push_str(if blank { "\n\n" } else { "\n" });
		}
		plain.push_str(line);
		blank = false;
	}
	plain
}

const SEARCH_SFW: &str = r#"
query ($search: String) {
  Page(perPage: 20) {
    media(search: $search, type: MANGA, isAdult: false) {
      id
      title { userPreferred }
      description
      startDate { year }
      staff(perPage: 2, sort: RELEVANCE) { nodes { name { full } } }
      format
      coverImage { medium }
      mediaListEntry { id }
    }
  }
}
"#;

const SEARCH_ALL: &str = r#"
query ($search: String) {
  Page(perPage: 20) {
    media(search: $search, type: MANGA) {
      id
      title { userPreferred }
      description
      startDate { year }
      staff(perPage: 2, sort: RELEVANCE) { nodes { name { full } } }
      format
      coverImage { medium }
      mediaListEntry { id }
    }
  }
}
"#;

const MEDIA_BY_ID: &str = r#"
query ($id: Int) {
  Media(id: $id) {
    id
    title { userPreferred }
    description
    startDate { year }
    staff(perPage: 2, sort: RELEVANCE) { nodes { name { full } } }
    format
    coverImage { medium }
    mediaListEntry { id }
  }
}
"#;

const MEDIA_STATE: &str = r#"
query ($id: Int) {
  Media(id: $id) {
    chapters
    volumes
    mediaListEntry {
      status
      score(format: POINT_100)
      progress
      progressVolumes
      startedAt { year month day }
      completedAt { year month day }
    }
  }
}
"#;

const VIEWER_SCORE_FORMAT: &str = r#"
query {
  Viewer { mediaListOptions { scoreFormat } }
}
"#;

const SAVE_MEDIA_LIST_ENTRY: &str = r#"
mutation (
  $id: Int, $status: MediaListStatus, $progress: Int, $volumes: Int,
  $score: Int, $startedAt: FuzzyDateInput, $completedAt: FuzzyDateInput
) {
  SaveMediaListEntry(
    mediaId: $id, status: $status, progress: $progress, progressVolumes: $volumes,
    scoreRaw: $score, startedAt: $startedAt, completedAt: $completedAt
  ) { id }
}
"#;

pub struct AniListTracker {
	http: reqwest::Client,
	oauth: Arc<OAuthManager>,
	graphql_url: String,
	score_formats: RwLock<HashMap<i64, String>>,
}

impl AniListTracker {
	pub fn new(http: reqwest::Client, oauth: Arc<OAuthManager>) -> Self {
		Self { http, oauth, graphql_url: GRAPHQL_URL.into(), score_formats: RwLock::new(HashMap::new()) }
	}

	#[cfg(test)]
	pub(crate) fn with_graphql_url(http: reqwest::Client, oauth: Arc<OAuthManager>, graphql_url: impl Into<String>) -> Self {
		let mut tracker = Self::new(http, oauth);
		tracker.graphql_url = graphql_url.into();
		tracker
	}

	fn error(message: impl Into<String>) -> TrackerError {
		TrackerError::Provider { provider: Provider::Anilist, message: message.into() }
	}

	async fn access_token(&self, user_id: i64) -> Result<String, TrackerError> {
		self.oauth.access_token(Provider::Anilist, user_id).await?.ok_or(TrackerError::NotLoggedIn { provider: Provider::Anilist })
	}

	async fn read_response(response: reqwest::Response) -> Result<(reqwest::StatusCode, String), TrackerError> {
		let status = response.status();
		let body = response.text().await.map_err(|error| Self::error(format!("response could not be read: {error}")))?;
		Ok((status, body))
	}

	fn is_unauthorized(status: reqwest::StatusCode, body: Option<&Value>) -> bool {
		status == reqwest::StatusCode::UNAUTHORIZED
			|| body
				.and_then(|body| body.get("errors"))
				.and_then(Value::as_array)
				.is_some_and(|errors| {
					errors.iter().filter_map(|error| error.get("message")).any(|message| {
						message.as_str().is_some_and(|message| message.contains("Invalid token") || message.contains("Invalid Authentication"))
					})
				})
	}

	async fn graphql(&self, user_id: i64, query: &str, variables: Value) -> Result<Value, TrackerError> {
		let mut token = self.access_token(user_id).await?;
		let send = |token: &str| {
			self.http.post(&self.graphql_url).bearer_auth(token).json(&json!({
				"query": query,
				"variables": variables.clone(),
			}))
		};

		let response = send(&token).send().await.map_err(|error| Self::error(format!("request failed: {error}")))?;
		let (mut status, mut raw) = Self::read_response(response).await?;
		let mut body = serde_json::from_str::<Value>(&raw).ok();

		if Self::is_unauthorized(status, body.as_ref()) {
			if let Some(fresh) = self.oauth.refresh_now(Provider::Anilist, user_id).await? {
				token = fresh;
				let response = send(&token).send().await.map_err(|error| Self::error(format!("request failed: {error}")))?;
				(status, raw) = Self::read_response(response).await?;
				body = serde_json::from_str::<Value>(&raw).ok();
			}
			if Self::is_unauthorized(status, body.as_ref()) {
				// the token is dead even after a refresh attempt: drop it so
				// the account stops reporting logged in — but only if the row
				// still holds the token that failed, or a racing re-login
				// would lose its fresh credentials
				self.oauth.logout_if_current(Provider::Anilist, user_id, &token).await?;
				return Err(TrackerError::NotLoggedIn { provider: Provider::Anilist });
			}
		}

		if !status.is_success() {
			return Err(Self::error(format!("request returned {status}: {raw}")));
		}
		let body = body.ok_or_else(|| Self::error("response was not valid JSON"))?;
		if let Some(errors) = body.get("errors").and_then(Value::as_array) {
			let message = errors
				.iter()
				.filter_map(|error| error.get("message"))
				.filter_map(Value::as_str)
				.collect::<Vec<_>>()
				.join("; ");
			return Err(Self::error(if message.is_empty() { "GraphQL request failed".into() } else { message }));
		}
		body.get("data").cloned().ok_or_else(|| Self::error("GraphQL response has no data"))
	}

	async fn score_format(&self, user_id: i64) -> Result<String, TrackerError> {
		if let Some(format) = self.score_formats.read().unwrap_or_else(|error| error.into_inner()).get(&user_id).cloned() {
			return Ok(format);
		}
		let data = self.graphql(user_id, VIEWER_SCORE_FORMAT, json!({})).await?;
		let format = data
			.pointer("/Viewer/mediaListOptions/scoreFormat")
			.and_then(Value::as_str)
			.map(str::to_string)
			.ok_or_else(|| Self::error("Viewer response has no score format"))?;
		self.score_formats.write().unwrap_or_else(|error| error.into_inner()).insert(user_id, format.clone());
		Ok(format)
	}

	fn manga_id(track_id: &str) -> Result<i64, TrackerError> {
		track_id.parse().map_err(|_| Self::error(format!("invalid media id: {track_id}")))
	}

	fn manga_url_id(query: &str) -> Option<i64> {
		let url = super::parse_tracker_url(query)?;
		let host = url.host_str()?.to_ascii_lowercase();
		if host != "anilist.co" && !host.ends_with(".anilist.co") {
			return None;
		}
		let mut segments = url.path_segments()?;
		if segments.next()? != "manga" {
			return None;
		}
		segments.next()?.parse().ok()
	}

	fn status_to_wire(status: TrackStatus) -> &'static str {
		match status {
			TrackStatus::Reading => "CURRENT",
			TrackStatus::Planning => "PLANNING",
			TrackStatus::Completed => "COMPLETED",
			TrackStatus::Paused => "PAUSED",
			TrackStatus::Dropped => "DROPPED",
			TrackStatus::Rereading => "REPEATING",
		}
	}

	fn status_from_wire(status: &str) -> Option<TrackStatus> {
		match status {
			"CURRENT" => Some(TrackStatus::Reading),
			"PLANNING" => Some(TrackStatus::Planning),
			"COMPLETED" => Some(TrackStatus::Completed),
			"PAUSED" => Some(TrackStatus::Paused),
			"DROPPED" => Some(TrackStatus::Dropped),
			"REPEATING" => Some(TrackStatus::Rereading),
			_ => None,
		}
	}

	fn date_input(date: Option<&str>) -> Option<Value> {
		let mut parts = date?.split('-');
		let year = parts.next()?.parse::<i64>().ok()?;
		let month = parts.next()?.parse::<i64>().ok()?;
		let day = parts.next()?.parse::<i64>().ok()?;
		Some(json!({ "year": year, "month": month, "day": day }))
	}

	fn date_output(date: Option<&Value>) -> Option<String> {
		let date = date?;
		let year = date.get("year")?.as_i64()?;
		if year == 0 {
			return None;
		}
		let month = date.get("month")?.as_i64()?;
		let day = date.get("day")?.as_i64()?;
		Some(format!("{year:04}-{month:02}-{day:02}"))
	}

	fn search_item(media: &Value) -> Option<TrackSearchItem> {
		let id = media.get("id")?.as_i64()?;
		let media_type = match media.get("format").and_then(Value::as_str) {
			Some("NOVEL") => Some(TrackMediaType::Novel),
			Some(_) => Some(TrackMediaType::Manga),
			None => None,
		};
		Some(TrackSearchItem {
			id: id.to_string(),
			title: media.pointer("/title/userPreferred").and_then(Value::as_str).unwrap_or("Unknown Title").to_string(),
			cover_url: media.pointer("/coverImage/medium").and_then(Value::as_str).map(str::to_string),
			info: super::search_info(
				media.pointer("/startDate/year").and_then(Value::as_i64),
				media
					.pointer("/staff/nodes")
					.and_then(Value::as_array)
					.into_iter()
					.flatten()
					.filter_map(|staff| staff.pointer("/name/full").and_then(Value::as_str))
					.map(str::to_string),
			),
			description: media.get("description").and_then(Value::as_str).map(plain_description),
			tracked: media.get("mediaListEntry").is_some_and(|entry| !entry.is_null()),
			media_type,
			series: false,
			url: Some(format!("https://anilist.co/manga/{id}")),
		})
	}
}

#[async_trait::async_trait]
impl TrackerService for AniListTracker {
	fn provider(&self) -> Provider {
		Provider::Anilist
	}

	async fn is_logged_in(&self, user_id: i64) -> Result<bool, TrackerError> {
		Ok(self.oauth.status(Provider::Anilist, user_id).await?.logged_in)
	}

	async fn search(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
		if let Some(id) = Self::manga_url_id(query) {
			let data = self.graphql(user_id, MEDIA_BY_ID, json!({ "id": id })).await?;
			return Ok(data.get("Media").and_then(Self::search_item).into_iter().collect());
		}

		let data = self.graphql(user_id, if nsfw { SEARCH_ALL } else { SEARCH_SFW }, json!({ "search": query })).await?;
		Ok(data.pointer("/Page/media").and_then(Value::as_array).into_iter().flatten().filter_map(Self::search_item).collect())
	}

	async fn get_state(&self, user_id: i64, track_id: &str) -> Result<RemoteState, TrackerError> {
		let id = Self::manga_id(track_id)?;
		let data = self.graphql(user_id, MEDIA_STATE, json!({ "id": id })).await?;
		let media = data.get("Media").ok_or_else(|| Self::error("media not found"))?;
		let Some(entry) = media.get("mediaListEntry").filter(|entry| !entry.is_null()) else {
			return Ok(RemoteState {
				total_chapters: media.get("chapters").and_then(Value::as_i64).map(|total| total as i32),
				total_volumes: media.get("volumes").and_then(Value::as_i64).map(|total| total as i32),
				..RemoteState::default()
			});
		};

		let raw_score = entry.get("score").and_then(Value::as_i64);
		let score = if self.score_format(user_id).await? == "POINT_10" { raw_score.map(|score| score / 10) } else { raw_score };
		Ok(RemoteState {
			score: score.map(|score| score as i32),
			status: entry.get("status").and_then(Value::as_str).and_then(Self::status_from_wire),
			last_read_chapter: entry.get("progress").and_then(Value::as_i64).map(|progress| progress as f32),
			last_read_volume: entry.get("progressVolumes").and_then(Value::as_i64).map(|progress| progress as i32),
			total_chapters: media.get("chapters").and_then(Value::as_i64).map(|total| total as i32),
			total_volumes: media.get("volumes").and_then(Value::as_i64).map(|total| total as i32),
			start_read_date: Self::date_output(entry.get("startedAt")),
			finish_read_date: Self::date_output(entry.get("completedAt")),
		})
	}

	async fn update(&self, user_id: i64, track_id: &str, update: &TrackUpdate) -> Result<(), TrackerError> {
		let id = Self::manga_id(track_id)?;
		let score = match update.score {
			Some(score) if self.score_format(user_id).await? == "POINT_10" => Some(score * 10),
			score => score,
		};
		let mut variables = serde_json::Map::new();
		variables.insert("id".into(), json!(id));
		if let Some(status) = update.status {
			variables.insert("status".into(), json!(Self::status_to_wire(status)));
		}
		if let Some(chapter) = update.last_read_chapter {
			variables.insert("progress".into(), json!(chapter.floor() as i64));
		}
		if let Some(volume) = update.last_read_volume {
			variables.insert("volumes".into(), json!(volume));
		}
		if let Some(score) = score {
			variables.insert("score".into(), json!(score));
		}
		if let Some(date) = Self::date_input(update.start_read_date.as_deref()) {
			variables.insert("startedAt".into(), date);
		}
		if let Some(date) = Self::date_input(update.finish_read_date.as_deref()) {
			variables.insert("completedAt".into(), date);
		}
		self.graphql(user_id, SAVE_MEDIA_LIST_ENTRY, Value::Object(variables)).await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::super::test_support::{CapturedRequest, capture, seed_token, serve, test_oauth};
	use super::*;
	use crate::tracker::core::{ChapterTrackInput, TrackMode, decide};
	use axum::extract::{Request, State};
	use axum::http::{Method, StatusCode};
	use axum::response::{IntoResponse, Response};
	use axum::{Json, Router};
	use sqlx::SqlitePool;
	use std::sync::Mutex;
	use std::sync::atomic::{AtomicBool, Ordering};

	#[derive(Clone)]
	struct MockAniList {
		requests: Arc<Mutex<Vec<CapturedRequest>>>,
		media: Value,
		score_format: Value,
		fail_score_format: Arc<AtomicBool>,
		unauthorized_once: Arc<AtomicBool>,
	}

	impl MockAniList {
		fn new(media: Value, score_format: &str) -> Self {
			Self {
				requests: Arc::new(Mutex::new(Vec::new())),
				media,
				score_format: score_format.into(),
				fail_score_format: Arc::new(AtomicBool::new(false)),
				unauthorized_once: Arc::new(AtomicBool::new(false)),
			}
		}

		fn requests(&self) -> Vec<CapturedRequest> {
			self.requests.lock().unwrap().clone()
		}
	}

	async fn endpoint(State(mock): State<MockAniList>, request: Request) -> Response {
		let captured = capture(request).await;
		let path = captured.path.clone();
		let request_json = (path == "/graphql").then(|| captured.json());
		mock.requests.lock().unwrap().push(captured);

		if path == "/token" {
			return Json(json!({
				"access_token": "new-token",
				"expires_in": 3600
			}))
			.into_response();
		}
		if mock.unauthorized_once.swap(false, Ordering::SeqCst) {
			return (StatusCode::UNAUTHORIZED, Json(json!({"errors": [{"message": "Invalid token"}]}))).into_response();
		}

		let query = request_json.unwrap()["query"].as_str().unwrap().to_string();
		if query.contains("Viewer") && mock.fail_score_format.load(Ordering::SeqCst) {
			return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"message": "score format unavailable"}))).into_response();
		}
		let data = if query.contains("SaveMediaListEntry") {
			json!({"SaveMediaListEntry": {"id": 1}})
		} else if query.contains("Viewer") {
			json!({"Viewer": {"mediaListOptions": {"scoreFormat": mock.score_format}}})
		} else if query.contains("Page") {
			json!({"Page": {"media": [mock.media]}})
		} else {
			json!({"Media": mock.media})
		};
		Json(json!({"data": data})).into_response()
	}

	async fn serve_mock(mock: MockAniList) -> String {
		serve(Router::new().fallback(endpoint).with_state(mock)).await
	}

	fn media() -> Value {
		json!({
			"id": 42,
			"title": {"userPreferred": "Test Manga"},
			"description": "Guts&rsquo; <i>story</i>.<br>\n<br>\n(Source: Dark Horse)",
			"startDate": {"year": 1989},
			"staff": {"nodes": [{"name": {"full": "Kentarou Miura"}}, {"name": {"full": "Studio Gaga"}}]},
			"format": "NOVEL",
			"coverImage": {"medium": "https://img.example/cover.jpg"},
			"chapters": 120,
			"volumes": 12,
			"mediaListEntry": {
				"id": 7,
				"status": "REPEATING",
				"score": 85,
				"progress": 12,
				"progressVolumes": 2,
				"startedAt": {"year": 2024, "month": 3, "day": 2},
				"completedAt": {"year": 0, "month": 0, "day": 0}
			}
		})
	}

	// token rows reference user(id) with foreign keys enforced, so the user
	// must exist before seed_token
	async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
		sqlx::query_scalar::<_, i64>("INSERT INTO user (username, password_hash) VALUES (?, 'x') RETURNING id")
			.bind(username)
			.fetch_one(pool)
			.await
			.unwrap()
	}

	async fn seeded_oauth(base_url: &str) -> (Arc<OAuthManager>, i64, tempfile::TempDir) {
		let (oauth, pool, tmp) = test_oauth(Provider::Anilist, base_url).await;
		let user = seed_user(&pool, "alice").await;
		seed_token(&pool, Provider::Anilist, user).await;
		(oauth, user, tmp)
	}

	async fn score_format_tracker(mock: MockAniList) -> (AniListTracker, i64, tempfile::TempDir) {
		let base_url = serve_mock(mock).await;
		let (oauth, user, tmp) = seeded_oauth(&base_url).await;
		(AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql")), user, tmp)
	}

	fn request_has_query(request: &CapturedRequest, query: &str) -> bool {
		request.json()["query"].as_str().is_some_and(|candidate| candidate.contains(query))
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
	async fn search_uses_graphql_filters_and_direct_media_lookup() {
		let mock = MockAniList::new(media(), "POINT_100");
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));
		assert_eq!(tracker.provider(), Provider::Anilist);
		assert!(tracker.is_logged_in(alice).await.unwrap());

		let items = tracker.search(alice, "Test", false).await.unwrap();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].id, "42");
		assert_eq!(items[0].title, "Test Manga");
		assert_eq!(items[0].info.as_deref(), Some("1989 / Kentarou Miura / Studio Gaga"));
		assert_eq!(items[0].description.as_deref(), Some("Guts\u{2019} story.\n\n(Source: Dark Horse)"));
		assert_eq!(items[0].media_type, Some(TrackMediaType::Novel));
		assert!(items[0].tracked);
		assert_eq!(items[0].url.as_deref(), Some("https://anilist.co/manga/42"));

		tracker.search(alice, "Test", true).await.unwrap();
		tracker.search(alice, "anilist.co/manga/42/Test_Manga", true).await.unwrap();
		assert!(matches!(
			tracker.search(alice + 1, "Test", true).await,
			Err(TrackerError::NotLoggedIn { provider: Provider::Anilist })
		));

		let requests = mock.requests();
		assert_eq!(requests.len(), 3);
		let first = &requests[0];
		assert_eq!(first.method, Method::POST);
		assert_eq!(first.path, "/graphql");
		assert_eq!(first.authorization.as_deref(), Some("Bearer old-token"));
		assert!(first.json()["query"].as_str().unwrap().contains("isAdult: false"));
		assert_eq!(first.json()["variables"], json!({"search": "Test"}));
		assert!(!requests[1].json()["query"].as_str().unwrap().contains("isAdult: false"));
		let direct = &requests[2];
		assert!(direct.json()["query"].as_str().unwrap().contains("Media(id: $id)"));
		assert_eq!(direct.json()["variables"], json!({"id": 42}));
	}

	#[tokio::test]
	async fn state_maps_score_status_progress_totals_and_fuzzy_dates() {
		let mock = MockAniList::new(media(), "POINT_10");
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));

		let state = tracker.get_state(alice, "42").await.unwrap();
		assert_eq!(
			state,
			RemoteState {
				score: Some(8),
				status: Some(TrackStatus::Rereading),
				last_read_chapter: Some(12.0),
				last_read_volume: Some(2),
				total_chapters: Some(120),
				total_volumes: Some(12),
				start_read_date: Some("2024-03-02".into()),
				finish_read_date: None,
			}
		);
		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert!(requests[0].json()["query"].as_str().unwrap().contains("score(format: POINT_100)"));
		assert!(requests[1].json()["query"].as_str().unwrap().contains("scoreFormat"));
	}

	#[tokio::test]
	async fn state_propagates_score_format_request_failure_without_caching_default() {
		let mock = MockAniList::new(media(), "POINT_10");
		mock.fail_score_format.store(true, Ordering::SeqCst);
		let (tracker, alice, _tmp) = score_format_tracker(mock.clone()).await;

		assert!(matches!(tracker.get_state(alice, "42").await, Err(TrackerError::Provider { provider: Provider::Anilist, .. })));
		mock.fail_score_format.store(false, Ordering::SeqCst);
		let state = tracker.get_state(alice, "42").await.unwrap();
		assert_eq!(state.score, Some(8));
		let requests = mock.requests();
		assert_eq!(requests.iter().filter(|request| request_has_query(request, "scoreFormat")).count(), 2);
	}

	#[tokio::test]
	async fn state_rejects_missing_score_format() {
		let mut mock = MockAniList::new(media(), "POINT_100");
		mock.score_format = Value::Null;
		let (tracker, alice, _tmp) = score_format_tracker(mock).await;

		assert!(matches!(tracker.get_state(alice, "42").await, Err(TrackerError::Provider { provider: Provider::Anilist, .. })));
	}

	#[tokio::test]
	async fn update_propagates_score_format_request_failure_without_saving_or_caching_default() {
		let mock = MockAniList::new(media(), "POINT_10");
		mock.fail_score_format.store(true, Ordering::SeqCst);
		let (tracker, alice, _tmp) = score_format_tracker(mock.clone()).await;
		let update = TrackUpdate { score: Some(9), ..TrackUpdate::default() };

		assert!(matches!(tracker.update(alice, "42", &update).await, Err(TrackerError::Provider { provider: Provider::Anilist, .. })));
		assert!(!mock.requests().iter().any(|request| request_has_query(request, "SaveMediaListEntry")));

		mock.fail_score_format.store(false, Ordering::SeqCst);
		tracker.update(alice, "42", &update).await.unwrap();
		let requests = mock.requests();
		assert_eq!(requests.iter().filter(|request| request_has_query(request, "scoreFormat")).count(), 2);
		let mutations = requests.iter().filter(|request| request_has_query(request, "SaveMediaListEntry")).collect::<Vec<_>>();
		assert_eq!(mutations.len(), 1);
		assert_eq!(mutations[0].json()["variables"]["score"], json!(90));
	}

	#[tokio::test]
	async fn update_rejects_missing_score_format_without_saving() {
		let mut mock = MockAniList::new(media(), "POINT_100");
		mock.score_format = Value::Null;
		let (tracker, alice, _tmp) = score_format_tracker(mock.clone()).await;
		let update = TrackUpdate { score: Some(9), ..TrackUpdate::default() };

		assert!(matches!(tracker.update(alice, "42", &update).await, Err(TrackerError::Provider { provider: Provider::Anilist, .. })));
		assert!(!mock.requests().iter().any(|request| request_has_query(request, "SaveMediaListEntry")));
	}

	#[tokio::test]
	async fn state_preserves_totals_when_media_is_not_in_user_list() {
		let mut media = media();
		media["mediaListEntry"] = Value::Null;
		let mock = MockAniList::new(media, "POINT_10");
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));

		let state = tracker.get_state(alice, "42").await.unwrap();
		assert_eq!(state.total_chapters, Some(120));
		assert_eq!(state.total_volumes, Some(12));
		assert_eq!(state.status, None);
		assert_eq!(state.last_read_chapter, None);
		let chapters = [ChapterTrackInput { number: Some(120.0), volume: None, read: true }];
		let update = decide(TrackMode::Chapter, 0, &state, &chapters, chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
		assert_eq!(update.status, Some(TrackStatus::Completed));
	}

	#[tokio::test]
	async fn update_uses_score_raw_and_floors_chapter_progress() {
		let mock = MockAniList::new(media(), "POINT_10");
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));
		let update = TrackUpdate {
			score: Some(9),
			status: Some(TrackStatus::Paused),
			last_read_chapter: Some(12.9),
			last_read_volume: Some(3),
			start_read_date: Some("2024-03-02".into()),
			finish_read_date: Some("2026-10-25".into()),
		};
		tracker.update(alice, "42", &update).await.unwrap();

		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		let mutation = &requests[1];
		assert!(mutation.json()["query"].as_str().unwrap().contains("scoreRaw: $score"));
		assert_eq!(
			mutation.json()["variables"],
			json!({
				"id": 42,
				"status": "PAUSED",
				"progress": 12,
				"volumes": 3,
				"score": 90,
				"startedAt": {"year": 2024, "month": 3, "day": 2},
				"completedAt": {"year": 2026, "month": 10, "day": 25}
			})
		);
	}

	#[tokio::test]
	async fn update_omits_unset_fields() {
		let mock = MockAniList::new(media(), "POINT_100");
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));
		let update = TrackUpdate { last_read_chapter: Some(12.9), ..TrackUpdate::default() };
		tracker.update(alice, "42", &update).await.unwrap();

		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		assert_eq!(requests[0].json()["variables"], json!({"id": 42, "progress": 12}));
	}

	#[tokio::test]
	async fn unauthorized_response_refreshes_and_retries_same_request() {
		let mut mock = MockAniList::new(media(), "POINT_100");
		mock.unauthorized_once = Arc::new(AtomicBool::new(true));
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = AniListTracker::with_graphql_url(reqwest::Client::new(), oauth, format!("{base_url}/graphql"));
		tracker.search(alice, "Test", false).await.unwrap();

		let requests = mock.requests();
		assert_eq!(requests.len(), 3);
		assert_eq!(requests[0].path, "/graphql");
		assert_eq!(requests[1].path, "/token");
		assert_eq!(requests[2].path, "/graphql");
		assert_eq!(requests[0].authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(requests[2].authorization.as_deref(), Some("Bearer new-token"));
		assert_eq!(requests[0].body, requests[2].body);
		assert_eq!(requests[1].content_type.as_deref(), Some("application/x-www-form-urlencoded"));
		assert!(requests[1].query("grant_type").is_none());
		let refresh = requests[1].form();
		assert_eq!(refresh["grant_type"], "refresh_token");
		assert_eq!(refresh["refresh_token"], "refresh-old");
		assert_eq!(refresh["client_id"], "anilist-test-id");
		assert_eq!(refresh["client_secret"], "anilist-test-secret");
	}

	#[test]
	fn every_tracker_status_has_an_anilist_mapping() {
		let cases = [
			(TrackStatus::Reading, "CURRENT"),
			(TrackStatus::Planning, "PLANNING"),
			(TrackStatus::Completed, "COMPLETED"),
			(TrackStatus::Paused, "PAUSED"),
			(TrackStatus::Dropped, "DROPPED"),
			(TrackStatus::Rereading, "REPEATING"),
		];
		for (status, wire) in cases {
			assert_eq!(AniListTracker::status_to_wire(status), wire);
			assert_eq!(AniListTracker::status_from_wire(wire), Some(status));
		}
		assert_eq!(AniListTracker::status_from_wire("UNKNOWN"), None);
	}
}
