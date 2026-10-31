//! MyAnimeList provider over the v2 REST API. Search requests the same
//! fields as a detail fetch, list status included, so one bounded request
//! answers it; rereading has no MAL status and degrades to reading on write.

use crate::tracker::core::{RemoteState, TrackMediaType, TrackSearchItem, TrackStatus, TrackUpdate};
use crate::tracker::metadata::{TitleAlternative, TitleVariants, TrackerMetadata, dedupe};
use crate::tracker::oauth::OAuthManager;
use crate::tracker::{Provider, TrackerError, TrackerService};
use serde_json::Value;
use std::sync::Arc;

const BASE_URL: &str = "https://api.myanimelist.net/v2";
const SEARCH_FIELDS: &str = "id,title,synopsis,num_chapters,main_picture,status,media_type,my_list_status,start_date,authors{first_name,last_name}";
const METADATA_FIELDS: &str = "id,title,alternative_titles{en,ja,synonyms},synopsis,main_picture,status,genres{id,name},media_type,authors{first_name,last_name},start_date";
const SEARCH_LIMIT: usize = 20;

pub struct MalTracker {
	http: reqwest::Client,
	oauth: Arc<OAuthManager>,
	base_url: String,
}

impl MalTracker {
	pub fn new(http: reqwest::Client, oauth: Arc<OAuthManager>) -> Self {
		Self { http, oauth, base_url: BASE_URL.into() }
	}

	#[cfg(test)]
	pub(crate) fn with_base_url(http: reqwest::Client, oauth: Arc<OAuthManager>, base_url: impl Into<String>) -> Self {
		let mut tracker = Self::new(http, oauth);
		tracker.base_url = base_url.into().trim_end_matches('/').to_string();
		tracker
	}

	fn error(message: impl Into<String>) -> TrackerError {
		TrackerError::Provider { provider: Provider::Mal, message: message.into() }
	}

	async fn access_token(&self, user_id: i64) -> Result<String, TrackerError> {
		self.oauth.access_token(Provider::Mal, user_id).await?.ok_or(TrackerError::NotLoggedIn { provider: Provider::Mal })
	}

	async fn read_response(response: reqwest::Response) -> Result<(reqwest::StatusCode, String), TrackerError> {
		let status = response.status();
		let body = response.text().await.map_err(|error| Self::error(format!("response could not be read: {error}")))?;
		Ok((status, body))
	}

	fn response_error(action: &str, status: reqwest::StatusCode, raw: &str) -> TrackerError {
		let message = serde_json::from_str::<Value>(raw)
			.ok()
			.and_then(|body| body.get("message").or_else(|| body.get("error")).and_then(Value::as_str).map(str::to_string))
			.filter(|message| !message.is_empty())
			.unwrap_or_else(|| raw.to_string());
		Self::error(format!("{action} returned {status}: {message}"))
	}

	async fn get_json(&self, user_id: i64, url: &str) -> Result<Value, TrackerError> {
		let mut token = self.access_token(user_id).await?;
		let send = |token: &str| self.http.get(url).bearer_auth(token);
		let response = send(&token).send().await.map_err(|error| Self::error(format!("request failed: {error}")))?;
		let (mut status, mut raw) = Self::read_response(response).await?;

		if status == reqwest::StatusCode::UNAUTHORIZED {
			if let Some(fresh) = self.oauth.refresh_now(Provider::Mal, user_id).await? {
				token = fresh;
				let response = send(&token).send().await.map_err(|error| Self::error(format!("request failed: {error}")))?;
				(status, raw) = Self::read_response(response).await?;
			}
			if status == reqwest::StatusCode::UNAUTHORIZED {
				// the token is dead even after a refresh attempt: drop it so the
				// account stops reporting logged in — but only if the row still
				// holds the token that failed, or a racing re-login would lose
				// its fresh credentials
				self.oauth.logout_if_current(Provider::Mal, user_id, &token).await?;
				return Err(TrackerError::NotLoggedIn { provider: Provider::Mal });
			}
		}

		if !status.is_success() {
			return Err(Self::response_error("request", status, &raw));
		}
		serde_json::from_str(&raw).map_err(|error| Self::error(format!("response was not valid JSON: {error}")))
	}

	fn manga_id(track_id: &str) -> Result<i64, TrackerError> {
		track_id.parse().map_err(|_| Self::error(format!("invalid manga id: {track_id}")))
	}

	fn manga_url_id(query: &str) -> Option<i64> {
		let url = super::parse_tracker_url(query)?;
		let host = url.host_str()?.to_ascii_lowercase();
		if host != "myanimelist.net" && !host.ends_with(".myanimelist.net") {
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
			TrackStatus::Reading | TrackStatus::Rereading => "reading",
			TrackStatus::Planning => "plan_to_read",
			TrackStatus::Completed => "completed",
			TrackStatus::Paused => "on_hold",
			TrackStatus::Dropped => "dropped",
		}
	}

	fn status_from_wire(status: &str) -> Option<TrackStatus> {
		match status {
			"reading" => Some(TrackStatus::Reading),
			"plan_to_read" => Some(TrackStatus::Planning),
			"completed" => Some(TrackStatus::Completed),
			"on_hold" => Some(TrackStatus::Paused),
			"dropped" => Some(TrackStatus::Dropped),
			_ => None,
		}
	}

	fn media_type(value: Option<&str>) -> Option<TrackMediaType> {
		match value {
			Some("novel" | "light_novel") => Some(TrackMediaType::Novel),
			Some("manga" | "one_shot" | "doujinshi" | "manhua" | "manhwa" | "oel") => Some(TrackMediaType::Manga),
			_ => None,
		}
	}

	fn search_item(id: i64, manga: &Value) -> TrackSearchItem {
		let detail = |name: &str| manga.get(name);
		TrackSearchItem {
			id: id.to_string(),
			title: detail("title").and_then(Value::as_str).unwrap_or("Unknown Title").to_string(),
			cover_url: detail("main_picture").and_then(|picture| picture.get("large")).and_then(Value::as_str).map(str::to_string),
			info: super::search_info(
				detail("start_date").and_then(Value::as_str).and_then(|date| date.get(..4)).and_then(|year| year.parse().ok()),
				detail("authors").and_then(Value::as_array).into_iter().flatten().filter_map(|author| author.get("node")).map(|person| {
					let name = |key| person.get(key).and_then(Value::as_str).unwrap_or("");
					format!("{} {}", name("first_name"), name("last_name"))
				}),
			),
			description: detail("synopsis").and_then(Value::as_str).map(str::to_string),
			tracked: detail("my_list_status").is_some_and(|status| !status.is_null()),
			media_type: Self::media_type(detail("media_type").and_then(Value::as_str)),
			series: false,
			url: Some(format!("https://myanimelist.net/manga/{id}")),
		}
	}

	fn metadata_from_manga(id: i64, manga: &Value) -> TrackerMetadata {
		let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string).filter(|value| !value.is_empty());
		let default = string(manga.get("title")).unwrap_or_default();
		let alternatives = [
			("English", string(manga.pointer("/alternative_titles/en"))),
			("Japanese", string(manga.pointer("/alternative_titles/ja"))),
		]
		.into_iter()
		.filter_map(|(label, value)| value.map(|value| TitleAlternative { label: label.into(), value }))
		.chain(
			manga
				.pointer("/alternative_titles/synonyms")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.filter_map(Value::as_str)
				.filter(|synonym| !synonym.is_empty())
				.map(|synonym| TitleAlternative { label: "Synonym".into(), value: synonym.to_string() }),
		)
		.collect();
		let authors = dedupe(
			manga
				.get("authors")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.filter_map(|author| author.get("node"))
				.map(|person| {
					let name = |key| person.get(key).and_then(Value::as_str).unwrap_or("");
					format!("{} {}", name("first_name"), name("last_name")).trim().to_string()
				})
				.filter(|name| !name.is_empty()),
		);
		let tags = dedupe(
			manga
				.get("genres")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.filter_map(|genre| genre.get("name").and_then(Value::as_str))
				.map(str::to_string),
		);
		TrackerMetadata {
			id: id.to_string(),
			title: TitleVariants { default, alternatives },
			info: super::search_info(
				manga.get("start_date").and_then(Value::as_str).and_then(|date| date.get(..4)).and_then(|year| year.parse().ok()),
				authors.iter().cloned(),
			),
			authors,
			artists: Vec::new(),
			description: string(manga.get("synopsis")),
			tags,
			status: manga.get("status").and_then(Value::as_str).and_then(crate::tracker::metadata::mal_status),
			content_rating: None,
			cover_url: string(manga.pointer("/main_picture/large")),
			url: Some(format!("https://myanimelist.net/manga/{id}")),
		}
	}

	fn date_update(date: &str) -> String {
		if matches!(date, "1969-12-31" | "1970-01-01") { String::new() } else { date.to_string() }
	}
}

#[async_trait::async_trait]
impl TrackerService for MalTracker {
	fn provider(&self) -> Provider {
		Provider::Mal
	}

	async fn is_logged_in(&self, user_id: i64) -> Result<bool, TrackerError> {
		Ok(self.oauth.status(Provider::Mal, user_id).await?.logged_in)
	}

	async fn search(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
		if let Some(id) = Self::manga_url_id(query) {
			let details = self.get_json(user_id, &format!("{}/manga/{id}?fields={SEARCH_FIELDS}", self.base_url)).await?;
			return Ok(vec![Self::search_item(id, &details)]);
		}

		let query = query.chars().take(64).collect::<String>();
		let query = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
		let body = self
			.get_json(user_id, &format!("{}/manga?q={query}&nsfw={nsfw}&limit={SEARCH_LIMIT}&fields={SEARCH_FIELDS}", self.base_url))
			.await?;
		Ok(body
			.get("data")
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.filter_map(|entry| {
				let manga = entry.get("node")?;
				let id = manga.get("id")?.as_i64()?;
				Some(Self::search_item(id, manga))
			})
			.collect())
	}

	async fn get_state(&self, user_id: i64, track_id: &str) -> Result<RemoteState, TrackerError> {
		let id = Self::manga_id(track_id)?;
		let body = self.get_json(user_id, &format!("{}/manga/{id}?fields=num_volumes,num_chapters,my_list_status", self.base_url)).await?;
		let Some(status) = body.get("my_list_status").filter(|status| !status.is_null()) else {
			return Ok(RemoteState {
				total_chapters: body.get("num_chapters").and_then(Value::as_i64).map(|total| total as i32),
				total_volumes: body.get("num_volumes").and_then(Value::as_i64).map(|total| total as i32),
				..RemoteState::default()
			});
		};
		Ok(RemoteState {
			score: status.get("score").and_then(Value::as_i64).map(|score| score as i32),
			status: status.get("status").and_then(Value::as_str).and_then(Self::status_from_wire),
			last_read_chapter: status.get("num_chapters_read").and_then(Value::as_i64).map(|progress| progress as f32),
			last_read_volume: status.get("num_volumes_read").and_then(Value::as_i64).map(|progress| progress as i32),
			total_chapters: body.get("num_chapters").and_then(Value::as_i64).map(|total| total as i32),
			total_volumes: body.get("num_volumes").and_then(Value::as_i64).map(|total| total as i32),
			start_read_date: status.get("start_date").and_then(Value::as_str).map(str::to_string),
			finish_read_date: status.get("finish_date").and_then(Value::as_str).map(str::to_string),
		})
	}

	async fn update(&self, user_id: i64, track_id: &str, update: &TrackUpdate) -> Result<(), TrackerError> {
		let id = Self::manga_id(track_id)?;
		let mut form: Vec<(&'static str, String)> = Vec::new();
		if let Some(status) = update.status {
			form.push(("status", Self::status_to_wire(status).into()));
		}
		if let Some(score) = update.score {
			form.push(("score", score.to_string()));
		}
		if let Some(chapter) = update.last_read_chapter {
			form.push(("num_chapters_read", chapter.floor().to_string()));
		}
		if let Some(volume) = update.last_read_volume {
			form.push(("num_volumes_read", volume.to_string()));
		}
		if let Some(date) = update.start_read_date.as_deref() {
			form.push(("start_date", Self::date_update(date)));
		}
		if let Some(date) = update.finish_read_date.as_deref() {
			form.push(("finish_date", Self::date_update(date)));
		}

		let url = format!("{}/manga/{id}/my_list_status", self.base_url);
		let mut token = self.access_token(user_id).await?;
		let send = |token: &str| self.http.patch(&url).bearer_auth(token).form(&form);
		let response = send(&token).send().await.map_err(|error| Self::error(format!("update failed: {error}")))?;
		let (mut status, mut raw) = Self::read_response(response).await?;
		if status == reqwest::StatusCode::UNAUTHORIZED {
			if let Some(fresh) = self.oauth.refresh_now(Provider::Mal, user_id).await? {
				token = fresh;
				let response = send(&token).send().await.map_err(|error| Self::error(format!("update failed: {error}")))?;
				(status, raw) = Self::read_response(response).await?;
			}
			if status == reqwest::StatusCode::UNAUTHORIZED {
				self.oauth.logout_if_current(Provider::Mal, user_id, &token).await?;
				return Err(TrackerError::NotLoggedIn { provider: Provider::Mal });
			}
		}
		if !status.is_success() {
			return Err(Self::response_error("update", status, &raw));
		}
		Ok(())
	}

	async fn metadata(&self, user_id: i64, track_id: &str) -> Result<TrackerMetadata, TrackerError> {
		let id = Self::manga_id(track_id)?;
		let manga = self.get_json(user_id, &format!("{}/manga/{id}?fields={METADATA_FIELDS}", self.base_url)).await?;
		Ok(Self::metadata_from_manga(id, &manga))
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
	use serde_json::json;
	use sqlx::SqlitePool;
	use std::collections::HashMap;
	use std::sync::Mutex;
	use std::sync::atomic::{AtomicBool, Ordering};

	#[derive(Clone)]
	struct MockMal {
		requests: Arc<Mutex<Vec<CapturedRequest>>>,
		search_results: Value,
		details: Arc<HashMap<i64, Value>>,
		unauthorized_once: Arc<AtomicBool>,
	}

	impl MockMal {
		fn new() -> Self {
			let details = HashMap::from([
				(
					1,
					json!({
						"id": 1,
						"title": "Detail One",
						"synopsis": "First synopsis",
						"start_date": "1989-08-25",
						"authors": [{"node": {"id": 1868, "first_name": "Kentarou", "last_name": "Miura"}, "role": "Story & Art"}],
						"main_picture": {"large": "https://img.example/one.jpg"},
						"media_type": "light_novel",
						"my_list_status": {"status": "reading"}
					}),
				),
				(
					2,
					json!({
						"id": 2,
						"title": "Detail Two",
						"main_picture": {"large": "https://img.example/two.jpg"},
						"media_type": "manga",
						"my_list_status": null
					}),
				),
				(
					42,
					json!({
						"id": 42,
						"title": "Direct Manga",
						"synopsis": "Direct synopsis",
						"main_picture": {"large": "https://img.example/direct.jpg"},
						"media_type": "manga",
						"num_chapters": 100,
						"num_volumes": 10,
						"my_list_status": {
							"status": "on_hold",
							"score": 8,
							"num_chapters_read": 12,
							"num_volumes_read": 3,
							"start_date": "2024-03-02",
							"finish_date": "2026-10-25"
						}
					}),
				),
			]);
			Self {
				requests: Arc::new(Mutex::new(Vec::new())),
				search_results: json!([
					{"node": details[&1]},
					{"node": details[&2]},
					{"node": {"id": 3}}
				]),
				details: Arc::new(details),
				unauthorized_once: Arc::new(AtomicBool::new(false)),
			}
		}

		fn requests(&self) -> Vec<CapturedRequest> {
			self.requests.lock().unwrap().clone()
		}
	}

	async fn endpoint(State(mock): State<MockMal>, request: Request) -> Response {
		let captured = capture(request).await;
		let path = captured.path.clone();
		let method = captured.method.clone();
		mock.requests.lock().unwrap().push(captured);

		if path == "/token" {
			return Json(json!({
				"access_token": "new-token",
				"expires_in": 3600
			}))
			.into_response();
		}
		if mock.unauthorized_once.swap(false, Ordering::SeqCst) {
			return (StatusCode::UNAUTHORIZED, Json(json!({"message": "Unauthorized"}))).into_response();
		}
		if path == "/v2/manga" && method == Method::GET {
			return Json(json!({"data": mock.search_results})).into_response();
		}
		if path.ends_with("/my_list_status") && method == Method::PATCH {
			return Json(json!({})).into_response();
		}
		let id = path.strip_prefix("/v2/manga/").and_then(|id| id.parse::<i64>().ok()).unwrap();
		Json(mock.details.get(&id).cloned().unwrap_or(Value::Null)).into_response()
	}

	async fn serve_mock(mock: MockMal) -> String {
		serve(Router::new().fallback(endpoint).with_state(mock)).await
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
		let (oauth, pool, tmp) = test_oauth(Provider::Mal, base_url).await;
		let user = seed_user(&pool, "alice").await;
		seed_token(&pool, Provider::Mal, user).await;
		(oauth, user, tmp)
	}

	#[tokio::test]
	async fn search_builds_items_from_one_bounded_request() {
		let mock = MockMal::new();
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = MalTracker::with_base_url(reqwest::Client::new(), oauth, format!("{base_url}/v2"));
		assert_eq!(tracker.provider(), Provider::Mal);
		assert!(tracker.is_logged_in(alice).await.unwrap());

		let query = "界".repeat(70);
		let items = tracker.search(alice, &query, true).await.unwrap();
		assert_eq!(items.len(), 3);
		assert_eq!(items[0].id, "1");
		assert_eq!(items[0].title, "Detail One");
		assert_eq!(items[0].description.as_deref(), Some("First synopsis"));
		assert_eq!(items[0].info.as_deref(), Some("1989 / Kentarou Miura"));
		assert_eq!(items[0].cover_url.as_deref(), Some("https://img.example/one.jpg"));
		assert_eq!(items[0].media_type, Some(TrackMediaType::Novel));
		assert_eq!(items[0].url.as_deref(), Some("https://myanimelist.net/manga/1"));
		assert!(items[0].tracked);
		assert_eq!(items[1].title, "Detail Two");
		assert_eq!(items[1].media_type, Some(TrackMediaType::Manga));
		assert!(!items[1].tracked);
		assert_eq!(items[2].title, "Unknown Title");
		assert!(!items[2].tracked);
		assert!(matches!(tracker.search(alice + 1, "Test", true).await, Err(TrackerError::NotLoggedIn { provider: Provider::Mal })));

		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		let search = &requests[0];
		assert_eq!(search.path, "/v2/manga");
		let expected_query = query.chars().take(64).collect::<String>();
		assert_eq!(search.query("q"), Some(expected_query.as_str()));
		assert_eq!(search.query("nsfw"), Some("true"));
		assert_eq!(search.query("limit"), Some("20"));
		assert_eq!(search.query("fields"), Some(SEARCH_FIELDS));
		assert_eq!(search.authorization.as_deref(), Some("Bearer old-token"));
	}

	#[tokio::test]
	async fn direct_url_search_and_state_use_my_list_status() {
		let mock = MockMal::new();
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = MalTracker::with_base_url(reqwest::Client::new(), oauth, format!("{base_url}/v2"));

		let items = tracker.search(alice, "myanimelist.net/manga/42/Direct_Manga", false).await.unwrap();
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].id, "42");
		assert_eq!(items[0].title, "Direct Manga");
		assert!(items[0].tracked);
		let state = tracker.get_state(alice, "42").await.unwrap();
		assert_eq!(
			state,
			RemoteState {
				score: Some(8),
				status: Some(TrackStatus::Paused),
				last_read_chapter: Some(12.0),
				last_read_volume: Some(3),
				total_chapters: Some(100),
				total_volumes: Some(10),
				start_read_date: Some("2024-03-02".into()),
				finish_read_date: Some("2026-10-25".into()),
			}
		);

		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert_eq!(requests[0].path, "/v2/manga/42");
		assert_eq!(requests[0].query("fields"), Some(SEARCH_FIELDS));
		assert_eq!(requests[1].path, "/v2/manga/42");
		assert_eq!(requests[1].query("fields"), Some("num_volumes,num_chapters,my_list_status"));
	}

	#[tokio::test]
	async fn state_preserves_totals_when_media_is_not_in_user_list() {
		let mut mock = MockMal::new();
		Arc::get_mut(&mut mock.details).unwrap().get_mut(&42).unwrap()["my_list_status"] = Value::Null;
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = MalTracker::with_base_url(reqwest::Client::new(), oauth, format!("{base_url}/v2"));

		let state = tracker.get_state(alice, "42").await.unwrap();
		assert_eq!(state.total_chapters, Some(100));
		assert_eq!(state.total_volumes, Some(10));
		assert_eq!(state.status, None);
		assert_eq!(state.last_read_chapter, None);
		let chapters = [ChapterTrackInput { number: Some(100.0), volume: None, read: true }];
		let update = decide(TrackMode::Chapter, 0, &state, &chapters, chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
		assert_eq!(update.status, Some(TrackStatus::Completed));
	}

	#[tokio::test]
	async fn update_sends_form_and_degrades_rereading_to_reading() {
		let mock = MockMal::new();
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = MalTracker::with_base_url(reqwest::Client::new(), oauth, format!("{base_url}/v2"));
		let update = TrackUpdate {
			score: Some(9),
			status: Some(TrackStatus::Rereading),
			last_read_chapter: Some(12.9),
			last_read_volume: Some(4),
			start_read_date: Some("1969-12-31".into()),
			finish_read_date: Some("2026-10-25".into()),
		};
		tracker.update(alice, "42", &update).await.unwrap();

		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		let request = &requests[0];
		assert_eq!(request.method, Method::PATCH);
		assert_eq!(request.path, "/v2/manga/42/my_list_status");
		assert_eq!(request.authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(request.content_type.as_deref(), Some("application/x-www-form-urlencoded"));
		let form = request.form();
		assert_eq!(form.len(), 6);
		assert_eq!(form["status"], "reading");
		assert_eq!(form["score"], "9");
		assert_eq!(form["num_chapters_read"], "12");
		assert_eq!(form["num_volumes_read"], "4");
		assert_eq!(form["start_date"], "");
		assert_eq!(form["finish_date"], "2026-10-25");
	}

	#[tokio::test]
	async fn unauthorized_update_refreshes_and_replays_form() {
		let mut mock = MockMal::new();
		mock.unauthorized_once = Arc::new(AtomicBool::new(true));
		let base_url = serve_mock(mock.clone()).await;
		let (oauth, alice, _tmp) = seeded_oauth(&base_url).await;
		let tracker = MalTracker::with_base_url(reqwest::Client::new(), oauth, format!("{base_url}/v2"));
		let update = TrackUpdate { status: Some(TrackStatus::Completed), last_read_chapter: Some(100.0), ..TrackUpdate::default() };
		tracker.update(alice, "42", &update).await.unwrap();

		let requests = mock.requests();
		assert_eq!(requests.len(), 3);
		assert_eq!(requests[0].path, "/v2/manga/42/my_list_status");
		assert_eq!(requests[1].path, "/token");
		assert_eq!(requests[2].path, "/v2/manga/42/my_list_status");
		assert_eq!(requests[0].authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(requests[2].authorization.as_deref(), Some("Bearer new-token"));
		assert_eq!(requests[0].body, requests[2].body);
		assert_eq!(requests[0].form()["status"], "completed");
		assert_eq!(requests[1].form()["grant_type"], "refresh_token");
		assert_eq!(requests[1].form()["refresh_token"], "refresh-old");
	}

	#[test]
	fn every_tracker_status_has_a_mal_write_mapping() {
		let cases = [
			(TrackStatus::Reading, "reading"),
			(TrackStatus::Planning, "plan_to_read"),
			(TrackStatus::Completed, "completed"),
			(TrackStatus::Paused, "on_hold"),
			(TrackStatus::Dropped, "dropped"),
			(TrackStatus::Rereading, "reading"),
		];
		for (status, wire) in cases {
			assert_eq!(MalTracker::status_to_wire(status), wire);
		}
		for (status, wire) in cases.into_iter().take(5) {
			assert_eq!(MalTracker::status_from_wire(wire), Some(status));
		}
		assert_eq!(MalTracker::status_from_wire("rereading"), None);
	}
	#[test]
	fn metadata_maps_alternative_titles_authors_and_genres() {
		let manga = json!({
			"id": 30002,
			"title": "Berserk",
			"alternative_titles": {"en": "Berserk", "ja": "ベルセルク", "synonyms": ["Berserk Manga"]},
			"synopsis": "A swordsman's tale.",
			"main_picture": {"large": "https://img.test/large.jpg"},
			"status": "currently_publishing",
			"genres": [{"id": 1, "name": "Action"}, {"id": 2, "name": "Adventure"}],
			"media_type": "manga",
			"start_date": "1989-01-01",
			"num_volumes": 41,
			"authors": [{"node": {"first_name": "Kentarou", "last_name": "Miura"}}]
		});
		let metadata = MalTracker::metadata_from_manga(30002, &manga);
		assert_eq!(metadata.id, "30002");
		assert_eq!(metadata.title_default(), Some("Berserk"));
		assert_eq!(metadata.authors, ["Kentarou Miura"]);
		assert_eq!(metadata.tags, ["Action", "Adventure"]);
		assert_eq!(metadata.status, Some(aidoku::MangaStatus::Ongoing));
		assert!(metadata.content_rating.is_none());
		assert_eq!(metadata.cover_url.as_deref(), Some("https://img.test/large.jpg"));
		assert_eq!(metadata.url.as_deref(), Some("https://myanimelist.net/manga/30002"));
		assert!(metadata.title.alternatives.iter().any(|alternative| alternative.label == "Japanese" && alternative.value == "ベルセルク"));
		assert!(metadata.title.alternatives.iter().any(|alternative| alternative.label == "Synonym" && alternative.value == "Berserk Manga"));
	}
}
