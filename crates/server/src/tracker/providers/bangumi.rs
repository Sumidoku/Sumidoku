//! Bangumi provider (bgm.tv Next `/p1` API). Search and subject detail embed
//! the caller's collection (`interest`), so neither needs a separate
//! collection lookup. Collections have no rereading type and carry no
//! start/finish dates, so rereading pushes as reading and the TrackUpdate date
//! fields are dropped on update.

use crate::tracker::core::{RemoteState, TrackMediaType, TrackSearchItem, TrackStatus, TrackUpdate};
use crate::tracker::oauth::OAuthManager;
use crate::tracker::{Provider, TrackerError, TrackerService};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

const API_ORIGIN: &str = "https://next.bgm.tv";
const SITE_ORIGIN: &str = "https://bgm.tv";
const USER_AGENT: &str = concat!("sumidoku/", env!("CARGO_PKG_VERSION"));

pub struct BangumiTracker {
	http: reqwest::Client,
	oauth: Arc<OAuthManager>,
	api_origin: String,
}

#[derive(Debug, Default, Deserialize)]
struct Subject {
	id: i64,
	#[serde(default)]
	name: String,
	#[serde(default, rename = "nameCN")]
	name_cn: Option<String>,
	#[serde(default)]
	info: Option<String>,
	#[serde(default)]
	summary: Option<String>,
	#[serde(default)]
	images: Option<Images>,
	#[serde(default)]
	platform: Option<SubjectPlatform>,
	#[serde(default)]
	series: bool,
	#[serde(default)]
	eps: Option<i32>,
	#[serde(default)]
	volumes: Option<i32>,
	#[serde(default)]
	interest: Option<SubjectInterest>,
}

#[derive(Debug, Deserialize)]
struct SubjectPlatform {
	#[serde(default, rename = "typeCN")]
	type_cn: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SubjectInterest {
	#[serde(default, rename = "type")]
	collection_type: Option<i64>,
	#[serde(default)]
	rate: Option<i32>,
	#[serde(default, rename = "epStatus")]
	ep_status: Option<i64>,
	#[serde(default, rename = "volStatus")]
	vol_status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Images {
	#[serde(default)]
	large: Option<String>,
	#[serde(default)]
	common: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SearchPage {
	#[serde(default)]
	data: Vec<Subject>,
}

fn provider_error(message: impl Into<String>) -> TrackerError {
	TrackerError::Provider { provider: Provider::Bangumi, message: message.into() }
}

fn not_logged_in() -> TrackerError {
	TrackerError::NotLoggedIn { provider: Provider::Bangumi }
}

fn subject_id(value: &str) -> Result<i64, TrackerError> {
	value.parse().map_err(|_| provider_error(format!("invalid Bangumi subject id '{value}'")))
}

fn collection_status(collection_type: Option<i64>) -> Option<TrackStatus> {
	match collection_type {
		Some(1) => Some(TrackStatus::Planning),
		Some(2) => Some(TrackStatus::Completed),
		Some(3) => Some(TrackStatus::Reading),
		Some(4) => Some(TrackStatus::Paused),
		Some(5) => Some(TrackStatus::Dropped),
		_ => None,
	}
}

fn collection_type(status: TrackStatus) -> i64 {
	match status {
		TrackStatus::Reading | TrackStatus::Rereading => 3,
		TrackStatus::Planning => 1,
		TrackStatus::Completed => 2,
		TrackStatus::Paused => 4,
		TrackStatus::Dropped => 5,
	}
}

impl BangumiTracker {
	pub fn new(http: reqwest::Client, oauth: Arc<OAuthManager>) -> Self {
		Self::with_api_origin(http, oauth, API_ORIGIN)
	}

	pub fn with_api_origin(http: reqwest::Client, oauth: Arc<OAuthManager>, api_origin: impl Into<String>) -> Self {
		Self { http, oauth, api_origin: api_origin.into().trim_end_matches('/').to_string() }
	}

	fn api_url(&self, segments: &[&str]) -> Result<Url, TrackerError> {
		let mut url = Url::parse(&format!("{}/", self.api_origin)).map_err(|error| provider_error(format!("invalid API origin: {error}")))?;
		url.path_segments_mut().map_err(|_| provider_error("API origin cannot contain path segments"))?.extend(segments);
		Ok(url)
	}

	async fn access_token(&self, user_id: i64) -> Result<String, TrackerError> {
		self.oauth.access_token(Provider::Bangumi, user_id).await?.ok_or_else(not_logged_in)
	}

	async fn send_authed(
		&self,
		user_id: i64,
		method: Method,
		url: Url,
		body: Option<&Value>,
		action: &str,
	) -> Result<reqwest::Response, TrackerError> {
		let token = self.access_token(user_id).await?;
		let build = |token: &str| {
			let mut request = self
				.http
				.request(method.clone(), url.clone())
				.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
				.header(reqwest::header::USER_AGENT, USER_AGENT);
			if let Some(body) = body {
				request = request.json(body);
			}
			request
		};
		let mut response = build(&token).send().await.map_err(|error| provider_error(format!("{action} request failed: {error}")))?;
		if response.status() == StatusCode::UNAUTHORIZED {
			let token = match self.oauth.refresh_now(Provider::Bangumi, user_id).await? {
				Some(token) => token,
				None => {
					self.oauth.logout_if_current(Provider::Bangumi, user_id, &token).await?;
					return Err(not_logged_in());
				}
			};
			response = build(&token).send().await.map_err(|error| provider_error(format!("{action} retry failed: {error}")))?;
			if response.status() == StatusCode::UNAUTHORIZED {
				self.oauth.logout_if_current(Provider::Bangumi, user_id, &token).await?;
				return Err(not_logged_in());
			}
		}
		Ok(response)
	}

	async fn decode_json<T: DeserializeOwned>(response: reqwest::Response, action: &str) -> Result<T, TrackerError> {
		let status = response.status();
		let body = response.text().await.map_err(|error| provider_error(format!("{action} response could not be read: {error}")))?;
		if !status.is_success() {
			return Err(provider_error(format!("{action} returned {status}: {body}")));
		}
		serde_json::from_str(&body).map_err(|error| provider_error(format!("{action} response is invalid: {error}")))
	}

	async fn ensure_success(response: reqwest::Response, action: &str) -> Result<(), TrackerError> {
		if response.status().is_success() {
			return Ok(());
		}
		let status = response.status();
		let body = response.text().await.unwrap_or_default();
		Err(provider_error(format!("{action} returned {status}: {body}")))
	}

	async fn send_ok(&self, user_id: i64, method: Method, url: Url, body: &Value, action: &str) -> Result<(), TrackerError> {
		let response = self.send_authed(user_id, method, url, Some(body), action).await?;
		Self::ensure_success(response, action).await
	}

	fn subject_url_id(query: &str) -> Option<i64> {
		let url = super::parse_tracker_url(query)?;
		let host = url.host_str()?.to_ascii_lowercase();
		if host != "bgm.tv" && host != "bangumi.tv" && !host.ends_with(".bgm.tv") {
			return None;
		}
		let mut segments = url.path_segments()?;
		if segments.next()? != "subject" {
			return None;
		}
		segments.next()?.parse().ok()
	}

	fn search_item(subject: Subject) -> TrackSearchItem {
		let title = subject.name_cn.as_deref().filter(|title| !title.is_empty()).unwrap_or(&subject.name);
		let media_type = match subject.platform.as_ref().and_then(|platform| platform.type_cn.as_deref()) {
			Some(platform) if platform.contains("小说") => Some(TrackMediaType::Novel),
			Some(platform) if platform.contains("漫画") => Some(TrackMediaType::Manga),
			_ => None,
		};
		let cover_url = subject.images.as_ref().and_then(|images| images.large.clone().or_else(|| images.common.clone()));
		TrackSearchItem {
			id: subject.id.to_string(),
			title: if title.is_empty() { "Unknown Title" } else { title }.into(),
			cover_url,
			info: subject.info.filter(|info| !info.is_empty()),
			// search results carry the infobox line only; subject detail carries the summary
			description: subject.summary.filter(|summary| !summary.is_empty()),
			tracked: subject.interest.is_some(),
			media_type,
			series: subject.series,
			url: Some(format!("{SITE_ORIGIN}/subject/{}", subject.id)),
		}
	}

	async fn search_subjects(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<Subject>, TrackerError> {
		let mut url = self.api_url(&["p1", "search", "subjects"])?;
		url.query_pairs_mut().append_pair("limit", "20");
		let body = json!({
			"keyword": query,
			"sort": "match",
			"filter": { "type": [1], "nsfw": nsfw }
		});
		let response = self.send_authed(user_id, Method::POST, url, Some(&body), "Bangumi search").await?;
		let page: SearchPage = Self::decode_json(response, "Bangumi search").await?;
		Ok(page.data)
	}

	async fn subject(&self, user_id: i64, id: i64) -> Result<Subject, TrackerError> {
		let url = self.api_url(&["p1", "subjects", &id.to_string()])?;
		let response = self.send_authed(user_id, Method::GET, url, None, "Bangumi subject detail").await?;
		Self::decode_json(response, "Bangumi subject detail").await
	}
}

#[async_trait::async_trait]
impl TrackerService for BangumiTracker {
	fn provider(&self) -> Provider {
		Provider::Bangumi
	}

	async fn is_logged_in(&self, user_id: i64) -> Result<bool, TrackerError> {
		Ok(self.oauth.status(Provider::Bangumi, user_id).await?.logged_in)
	}

	async fn search(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
		if let Some(id) = Self::subject_url_id(query) {
			return Ok(vec![Self::search_item(self.subject(user_id, id).await?)]);
		}
		let subjects = self.search_subjects(user_id, query, nsfw).await?;
		Ok(subjects.into_iter().map(Self::search_item).collect())
	}

	async fn get_state(&self, user_id: i64, track_id: &str) -> Result<RemoteState, TrackerError> {
		let subject = self.subject(user_id, subject_id(track_id)?).await?;
		let interest = subject.interest.unwrap_or_default();
		Ok(RemoteState {
			score: interest.rate,
			status: collection_status(interest.collection_type),
			last_read_chapter: interest.ep_status.map(|value| value as f32),
			last_read_volume: interest.vol_status,
			total_chapters: subject.eps,
			total_volumes: subject.volumes,
			start_read_date: None,
			finish_read_date: None,
		})
	}

	async fn update(&self, user_id: i64, track_id: &str, update: &TrackUpdate) -> Result<(), TrackerError> {
		let id = subject_id(track_id)?;
		let mut progress = serde_json::Map::new();
		if let Some(chapter) = update.last_read_chapter {
			progress.insert("epStatus".into(), json!(chapter.floor() as i64));
		}
		if let Some(volume) = update.last_read_volume {
			progress.insert("volStatus".into(), json!(volume));
		}
		let has_collection = update.status.is_some() || update.score.is_some();
		if !has_collection && progress.is_empty() {
			return Ok(());
		}
		let url = self.api_url(&["p1", "collections", "subjects", &id.to_string()])?;
		if has_collection {
			// PUT replaces both type and rate, so a partial update re-sends the other's current value
			let current = match (update.status, update.score) {
				(Some(_), Some(_)) => SubjectInterest::default(),
				_ => self.subject(user_id, id).await?.interest.unwrap_or_default(),
			};
			let collection_type = update.status.map(collection_type).or(current.collection_type).unwrap_or(3);
			let rate = update.score.or(current.rate).unwrap_or(0);
			self.send_ok(user_id, Method::PUT, url.clone(), &json!({"type": collection_type, "rate": rate}), "Bangumi collection update").await?;
		}
		if progress.is_empty() {
			return Ok(());
		}
		let progress = Value::Object(progress);
		let response = self.send_authed(user_id, Method::PATCH, url.clone(), Some(&progress), "Bangumi progress update").await?;
		if response.status() == StatusCode::NOT_FOUND && !has_collection {
			// progress can only be patched onto an existing collection: create one as reading, then retry
			self.send_ok(user_id, Method::PUT, url.clone(), &json!({"type": 3, "rate": 0}), "Bangumi collection update").await?;
			return self.send_ok(user_id, Method::PATCH, url, &progress, "Bangumi progress update").await;
		}
		Self::ensure_success(response, "Bangumi progress update").await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tracker::core::{ChapterTrackInput, TrackMode};
	use crate::tracker::providers::test_support::{self, CapturedRequest};
	use axum::extract::{Request, State};
	use axum::response::{IntoResponse, Response};
	use axum::{Json, Router};
	use chrono::NaiveDate;
	use sqlx::SqlitePool;
	use std::sync::Mutex;

	// the built-in admin row migration 0001 creates satisfies the token table's user FK
	const USER_ID: i64 = 1;

	type Responder = dyn Fn(&CapturedRequest) -> (StatusCode, Value) + Send + Sync;

	#[derive(Clone)]
	struct Mock {
		requests: Arc<Mutex<Vec<CapturedRequest>>>,
		responder: Arc<Responder>,
	}

	impl Mock {
		fn requests(&self) -> Vec<CapturedRequest> {
			self.requests.lock().unwrap().clone()
		}
	}

	async fn handle(State(mock): State<Mock>, request: Request) -> Response {
		let captured = test_support::capture(request).await;
		let (status, body) = (mock.responder)(&captured);
		mock.requests.lock().unwrap().push(captured);
		(status, Json(body)).into_response()
	}

	async fn serve<F>(responder: F) -> (String, Mock)
	where
		F: Fn(&CapturedRequest) -> (StatusCode, Value) + Send + Sync + 'static,
	{
		let mock = Mock { requests: Arc::new(Mutex::new(Vec::new())), responder: Arc::new(responder) };
		let router = Router::new().fallback(axum::routing::any(handle)).with_state(mock.clone());
		let origin = test_support::serve(router).await;
		(origin, mock)
	}

	async fn test_service(origin: &str) -> (BangumiTracker, SqlitePool, tempfile::TempDir) {
		let (oauth, pool, tmp) = test_support::test_oauth(Provider::Bangumi, origin).await;
		test_support::seed_token(&pool, Provider::Bangumi, USER_ID).await;
		(BangumiTracker::with_api_origin(reqwest::Client::new(), oauth, origin), pool, tmp)
	}

	fn subject(id: i64, name: &str) -> Value {
		json!({
			"id": id,
			"name": name,
			"nameCN": "",
			"images": {"large": format!("https://img.test/{id}.jpg")},
			"info": format!("Info {id}"),
			"summary": format!("Summary {id}"),
			"platform": {"typeCN": "漫画"},
			"eps": 30,
			"volumes": 6,
			"interest": null
		})
	}

	// search results carry no summary, eps or volumes
	fn slim_subject(id: i64, name: &str) -> Value {
		json!({
			"id": id,
			"name": name,
			"nameCN": "",
			"images": {"large": format!("https://img.test/{id}.jpg")},
			"info": format!("Info {id}"),
			"type": 1,
			"platform": {"typeCN": "漫画"},
			"interest": null
		})
	}

	#[tokio::test]
	async fn search_reads_platform_series_and_interest_from_one_request() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/search/subjects" => {
				let mut manga = slim_subject(101, "Book");
				manga["nameCN"] = json!("书籍");
				manga["interest"] = json!({"type": 3, "rate": 8});
				manga["series"] = json!(true);
				let mut novel = slim_subject(102, "Novel");
				novel["platform"]["typeCN"] = json!("轻小说");
				(StatusCode::OK, json!({"data": [manga, novel], "total": 2}))
			}
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		assert_eq!(service.provider(), Provider::Bangumi);
		assert!(service.is_logged_in(USER_ID).await.unwrap());

		let results = service.search(USER_ID, "Book", false).await.unwrap();
		assert_eq!(results.len(), 2);
		assert_eq!(results[0].id, "101");
		assert_eq!(results[0].title, "书籍");
		assert!(results[0].tracked);
		assert!(results[0].series);
		assert_eq!(results[0].media_type, Some(TrackMediaType::Manga));
		assert_eq!(results[0].info.as_deref(), Some("Info 101"));
		assert_eq!(results[0].description, None);
		assert_eq!(results[1].id, "102");
		assert!(!results[1].tracked);
		assert!(!results[1].series);
		assert_eq!(results[1].media_type, Some(TrackMediaType::Novel));
		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		let search = &requests[0];
		assert_eq!(search.path, "/p1/search/subjects");
		assert_eq!(search.method, Method::POST);
		assert_eq!(search.query("limit"), Some("20"));
		assert_eq!(search.authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(search.json(), json!({"keyword": "Book", "sort": "match", "filter": {"type": [1], "nsfw": false}}));
	}

	#[tokio::test]
	async fn failed_search_returns_error_without_detail_requests() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/search/subjects" => (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "search unavailable"})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;

		assert!(matches!(
			service.search(USER_ID, "标题 / A", true).await,
			Err(TrackerError::Provider { provider: Provider::Bangumi, .. })
		));
		let requests = mock.requests();
		let search = requests.iter().find(|request| request.path == "/p1/search/subjects").unwrap();
		assert_eq!(search.json()["filter"]["nsfw"], true);
		assert!(!requests.iter().any(|request| request.path.starts_with("/p1/subjects/")));
	}

	#[tokio::test]
	async fn state_propagates_subject_detail_failure() {
		let (origin, _mock) = serve(|request| match request.path.as_str() {
			"/p1/subjects/42" => (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "subject unavailable"})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		assert!(matches!(service.get_state(USER_ID, "42").await, Err(TrackerError::Provider { provider: Provider::Bangumi, .. })));
	}

	#[tokio::test]
	async fn state_reads_interest_and_totals_from_one_subject_request() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/subjects/42" => {
				let mut value = subject(42, "Book");
				value["interest"] = json!({"type": 3, "rate": 8, "epStatus": 12, "volStatus": 2});
				(StatusCode::OK, value)
			}
			"/p1/subjects/43" => (StatusCode::OK, subject(43, "Book 43")),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;

		let state = service.get_state(USER_ID, "42").await.unwrap();
		assert_eq!(state.score, Some(8));
		assert_eq!(state.status, Some(TrackStatus::Reading));
		assert_eq!(state.last_read_chapter, Some(12.0));
		assert_eq!(state.last_read_volume, Some(2));
		assert_eq!(state.total_chapters, Some(30));
		assert_eq!(state.total_volumes, Some(6));
		assert_eq!(state.start_read_date, None);
		assert_eq!(state.finish_read_date, None);

		let uncollected = service.get_state(USER_ID, "43").await.unwrap();
		assert_eq!(uncollected.score, None);
		assert_eq!(uncollected.status, None);
		assert_eq!(uncollected.last_read_chapter, None);
		assert_eq!(uncollected.total_chapters, Some(30));
		assert_eq!(uncollected.total_volumes, Some(6));
		let chapters = [ChapterTrackInput { number: Some(30.0), volume: None, read: true }];
		let update = crate::tracker::core::decide(TrackMode::Chapter, 0, &uncollected, &chapters, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
		assert_eq!(update.status, Some(TrackStatus::Completed));

		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert!(requests.iter().all(|request| request.path.starts_with("/p1/subjects/")));
	}

	#[tokio::test]
	async fn final_unauthorized_logs_out() {
		let (origin, _mock) = serve(|request| match request.path.as_str() {
			"/token" => (StatusCode::OK, json!({"access_token": "fresh", "expires_in": 3600})),
			_ => (StatusCode::UNAUTHORIZED, json!({"message": "expired"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { last_read_chapter: Some(1.0), ..TrackUpdate::default() };
		assert!(matches!(service.update(USER_ID, "42", &update).await, Err(TrackerError::NotLoggedIn { .. })));
		assert!(!service.oauth.status(Provider::Bangumi, USER_ID).await.unwrap().logged_in);
	}

	#[tokio::test]
	async fn update_splits_collection_and_progress_and_refreshes_once() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/token" => (StatusCode::OK, json!({"access_token": "fresh", "refresh_token": "refresh-new", "expires_in": 3600})),
			"/p1/collections/subjects/42" if request.authorization.as_deref() == Some("Bearer old-token") => {
				(StatusCode::UNAUTHORIZED, json!({"message": "expired"}))
			}
			"/p1/collections/subjects/42" => (StatusCode::OK, json!({})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate {
			score: Some(8),
			status: Some(TrackStatus::Rereading),
			last_read_chapter: Some(12.9),
			last_read_volume: Some(3),
			start_read_date: Some("2026-10-01".into()),
			finish_read_date: Some("2026-10-20".into()),
		};

		service.update(USER_ID, "42", &update).await.unwrap();
		let requests = mock.requests();
		let updates = requests.iter().filter(|request| request.path == "/p1/collections/subjects/42").collect::<Vec<_>>();
		assert_eq!(updates.len(), 3);
		assert_eq!(updates[0].method, Method::PUT);
		assert_eq!(updates[1].method, Method::PUT);
		assert_eq!(updates[2].method, Method::PATCH);
		assert_eq!(updates[0].authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(updates[1].authorization.as_deref(), Some("Bearer fresh"));
		assert_eq!(updates[2].authorization.as_deref(), Some("Bearer fresh"));
		for request in &updates[..2] {
			assert_eq!(request.json(), json!({"type": 3, "rate": 8}));
		}
		assert_eq!(updates[2].json(), json!({"epStatus": 12, "volStatus": 3}));
		let tokens = requests.iter().filter(|request| request.path == "/token").collect::<Vec<_>>();
		assert_eq!(tokens.len(), 1);
		assert_eq!(tokens[0].form()["grant_type"], "refresh_token");
		assert_eq!(tokens[0].form()["refresh_token"], "refresh-old");
		assert!(!requests.iter().any(|request| request.path == "/p1/subjects/42"));
	}

	#[tokio::test]
	async fn status_only_update_preserves_existing_rating() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/subjects/42" => {
				let mut value = subject(42, "Book");
				value["interest"] = json!({"type": 1, "rate": 8, "epStatus": 4, "volStatus": 1});
				(StatusCode::OK, value)
			}
			"/p1/collections/subjects/42" => (StatusCode::OK, json!({})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { status: Some(TrackStatus::Reading), ..TrackUpdate::default() };

		service.update(USER_ID, "42", &update).await.unwrap();
		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert_eq!(requests[0].path, "/p1/subjects/42");
		assert_eq!(requests[1].method, Method::PUT);
		assert_eq!(requests[1].json(), json!({"type": 3, "rate": 8}));
	}

	#[tokio::test]
	async fn score_only_update_creates_reading_collection_when_missing() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/subjects/42" => (StatusCode::OK, subject(42, "Book")),
			"/p1/collections/subjects/42" => (StatusCode::OK, json!({})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { score: Some(8), ..TrackUpdate::default() };

		service.update(USER_ID, "42", &update).await.unwrap();
		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert_eq!(requests[1].method, Method::PUT);
		assert_eq!(requests[1].json(), json!({"type": 3, "rate": 8}));
	}

	#[tokio::test]
	async fn progress_only_update_sends_one_patch() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/collections/subjects/42" => (StatusCode::OK, json!({})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { last_read_chapter: Some(12.9), ..TrackUpdate::default() };

		service.update(USER_ID, "42", &update).await.unwrap();
		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		assert_eq!(requests[0].method, Method::PATCH);
		assert_eq!(requests[0].json(), json!({"epStatus": 12}));
	}

	#[tokio::test]
	async fn progress_only_update_creates_missing_collection_and_retries() {
		let collected = Arc::new(Mutex::new(false));
		let collected_by_put = collected.clone();
		let (origin, mock) = serve(move |request| match request.path.as_str() {
			"/p1/collections/subjects/42" if request.method == Method::PUT => {
				*collected_by_put.lock().unwrap() = true;
				(StatusCode::OK, json!({}))
			}
			"/p1/collections/subjects/42" if !*collected.lock().unwrap() => (StatusCode::NOT_FOUND, json!({"message": "subject not collected"})),
			"/p1/collections/subjects/42" => (StatusCode::OK, json!({})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { last_read_chapter: Some(12.9), last_read_volume: Some(3), ..TrackUpdate::default() };

		service.update(USER_ID, "42", &update).await.unwrap();
		let requests = mock.requests();
		assert_eq!(requests.len(), 3);
		assert_eq!(requests[0].method, Method::PATCH);
		assert_eq!(requests[1].method, Method::PUT);
		assert_eq!(requests[2].method, Method::PATCH);
		assert_eq!(requests[1].json(), json!({"type": 3, "rate": 0}));
		for request in [&requests[0], &requests[2]] {
			assert_eq!(request.json(), json!({"epStatus": 12, "volStatus": 3}));
		}
	}

	#[tokio::test]
	async fn empty_or_date_only_update_skips_requests() {
		let (origin, mock) = serve(|_| (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "unexpected"}))).await;
		let (service, _pool, _tmp) = test_service(&origin).await;
		service.update(USER_ID, "42", &TrackUpdate::default()).await.unwrap();
		let update = TrackUpdate {
			start_read_date: Some("2026-10-01".into()),
			finish_read_date: Some("2026-10-20".into()),
			..TrackUpdate::default()
		};
		service.update(USER_ID, "42", &update).await.unwrap();
		assert!(mock.requests().is_empty());
	}

	#[tokio::test]
	async fn subject_url_uses_detail_and_official_result_url() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/p1/subjects/1902" => {
				let mut value = subject(1902, "Book");
				value["interest"] = json!({"type": 3, "rate": 8});
				(StatusCode::OK, value)
			}
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, _pool, _tmp) = test_service(&origin).await;

		let results = service.search(USER_ID, "bgm.tv/subject/1902", false).await.unwrap();
		assert_eq!(results.len(), 1);
		assert_eq!(results[0].id, "1902");
		assert!(results[0].tracked);
		assert_eq!(results[0].media_type, Some(TrackMediaType::Manga));
		assert_eq!(results[0].description.as_deref(), Some("Summary 1902"));
		assert_eq!(results[0].url.as_deref(), Some("https://bgm.tv/subject/1902"));
		let requests = mock.requests();
		assert_eq!(requests.len(), 1);
		assert_eq!(requests[0].path, "/p1/subjects/1902");
	}

	#[test]
	fn status_mapping_has_no_rereading_collection_type() {
		assert_eq!(collection_status(Some(1)), Some(TrackStatus::Planning));
		assert_eq!(collection_status(Some(2)), Some(TrackStatus::Completed));
		assert_eq!(collection_status(Some(3)), Some(TrackStatus::Reading));
		assert_eq!(collection_status(Some(4)), Some(TrackStatus::Paused));
		assert_eq!(collection_status(Some(5)), Some(TrackStatus::Dropped));
		assert_eq!(collection_type(TrackStatus::Rereading), 3);
	}
}
