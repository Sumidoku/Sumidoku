use crate::tracker::core::{RemoteState, TrackMediaType, TrackSearchItem, TrackStatus, TrackUpdate};
use crate::tracker::oauth::OAuthManager;
use crate::tracker::{Provider, TrackerError, TrackerService};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

const API_ORIGIN: &str = "https://api.mangabaka.org";
const SITE_ORIGIN: &str = "https://mangabaka.org";
const DEFAULT_TITLE_LANGUAGE: &str = "en";
const USER_AGENT: &str = concat!("sumidoku/", env!("CARGO_PKG_VERSION"));

pub struct MangaBakaTracker {
	http: reqwest::Client,
	oauth: Arc<OAuthManager>,
	api_origin: String,
	title_language: String,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
	data: T,
}

#[derive(Debug, Default, Deserialize)]
struct Series {
	id: i64,
	#[serde(default)]
	title: String,
	#[serde(default)]
	titles: Vec<SeriesTitle>,
	#[serde(default)]
	cover: Cover,
	#[serde(default)]
	description: Option<String>,
	#[serde(default)]
	status: String,
	#[serde(default, rename = "type")]
	series_type: Option<String>,
	#[serde(default)]
	total_chapters: Option<String>,
	#[serde(default)]
	final_volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SeriesTitle {
	#[serde(default)]
	language: String,
	#[serde(default)]
	title: String,
	#[serde(default)]
	traits: Vec<String>,
	#[serde(default)]
	is_primary: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct Cover {
	#[serde(default)]
	raw: Option<CoverRaw>,
	#[serde(default)]
	x350: Option<CoverDpi>,
}

#[derive(Debug, Deserialize)]
struct CoverRaw {
	url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverDpi {
	x1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LibraryBatchEntry {
	series_id: i64,
}

#[derive(Debug, Default, Deserialize)]
struct LibraryEntry {
	#[serde(default)]
	rating: Option<f64>,
	#[serde(default)]
	state: Option<String>,
	#[serde(default)]
	progress_chapter: Option<i64>,
	#[serde(default)]
	progress_volume: Option<i32>,
	#[serde(default)]
	start_date: Option<String>,
	#[serde(default)]
	finish_date: Option<String>,
}

fn provider_error(message: impl Into<String>) -> TrackerError {
	TrackerError::Provider { provider: Provider::Mangabaka, message: message.into() }
}

fn not_logged_in() -> TrackerError {
	TrackerError::NotLoggedIn { provider: Provider::Mangabaka }
}

fn track_id(value: &str) -> Result<i64, TrackerError> {
	value.parse().map_err(|_| provider_error(format!("invalid MangaBaka series id '{value}'")))
}

fn library_status(status: &str) -> Option<TrackStatus> {
	match status {
		"reading" => Some(TrackStatus::Reading),
		"considering" | "plan_to_read" => Some(TrackStatus::Planning),
		"completed" => Some(TrackStatus::Completed),
		"paused" => Some(TrackStatus::Paused),
		"dropped" => Some(TrackStatus::Dropped),
		"rereading" => Some(TrackStatus::Rereading),
		_ => None,
	}
}

fn library_status_name(status: TrackStatus) -> &'static str {
	match status {
		TrackStatus::Reading => "reading",
		TrackStatus::Planning => "plan_to_read",
		TrackStatus::Completed => "completed",
		TrackStatus::Paused => "paused",
		TrackStatus::Dropped => "dropped",
		TrackStatus::Rereading => "rereading",
	}
}

fn normalized_language(language: &str) -> String {
	language.replace("-Latn", "-ro")
}

fn primary_title(titles: &[SeriesTitle], preference: &str) -> String {
	let preference = normalized_language(preference);
	if !preference.is_empty() {
		if let Some(title) = titles.iter().find(|title| normalized_language(&title.language) == preference) {
			return title.title.clone();
		}
		if let Some(title) =
			titles.iter().find(|title| normalized_language(&title.language).starts_with(&preference) && !title.language.ends_with("-Latn"))
		{
			return title.title.clone();
		}
	}
	if let Some(title) = titles.iter().find(|title| title.traits.iter().any(|trait_name| trait_name == "native")) {
		return title.title.clone();
	}
	if let Some(title) = titles.iter().find(|title| title.language == "en" && title.is_primary == Some(true)) {
		return title.title.clone();
	}
	titles.first().map(|title| title.title.clone()).unwrap_or_default()
}

fn supported_date(date: &str) -> bool {
	date != "1969-12-31" && date != "1970-01-01"
}

impl Series {
	fn totals(&self) -> (Option<i32>, Option<i32>) {
		if self.status == "releasing" {
			return (None, None);
		}
		(self.total_chapters.as_deref().and_then(|value| value.parse().ok()), self.final_volume.as_deref().and_then(|value| value.parse().ok()))
	}
}

impl MangaBakaTracker {
	pub fn new(http: reqwest::Client, oauth: Arc<OAuthManager>) -> Self {
		Self::with_api_origin(http, oauth, API_ORIGIN)
	}

	pub fn with_api_origin(http: reqwest::Client, oauth: Arc<OAuthManager>, api_origin: impl Into<String>) -> Self {
		Self { http, oauth, api_origin: api_origin.into().trim_end_matches('/').to_string(), title_language: DEFAULT_TITLE_LANGUAGE.into() }
	}

	#[cfg(test)]
	pub(crate) fn with_title_language(mut self, title_language: impl Into<String>) -> Self {
		self.title_language = title_language.into();
		self
	}

	fn api_url(&self, segments: &[&str]) -> Result<Url, TrackerError> {
		let mut url = Url::parse(&format!("{}/", self.api_origin)).map_err(|error| provider_error(format!("invalid API origin: {error}")))?;
		url.path_segments_mut().map_err(|_| provider_error("API origin cannot contain path segments"))?.extend(segments);
		Ok(url)
	}

	async fn access_token(&self, user_id: i64) -> Result<String, TrackerError> {
		self.oauth.access_token(Provider::Mangabaka, user_id).await?.ok_or_else(not_logged_in)
	}

	async fn send_public(&self, url: Url, action: &str) -> Result<reqwest::Response, TrackerError> {
		self.http
			.get(url)
			.header(reqwest::header::USER_AGENT, USER_AGENT)
			.send()
			.await
			.map_err(|error| provider_error(format!("{action} request failed: {error}")))
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
		let mut response =
			build(&token).send().await.map_err(|error| provider_error(format!("{action} request failed: {error}")))?;
		if response.status() == StatusCode::UNAUTHORIZED {
			let token = match self.oauth.refresh_now(Provider::Mangabaka, user_id).await? {
				Some(token) => token,
				None => {
					self.oauth.logout_if_current(Provider::Mangabaka, user_id, &token).await?;
					return Err(not_logged_in());
				}
			};
			response = build(&token).send().await.map_err(|error| provider_error(format!("{action} retry failed: {error}")))?;
			if response.status() == StatusCode::UNAUTHORIZED {
				self.oauth.logout_if_current(Provider::Mangabaka, user_id, &token).await?;
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

	async fn get_authed_json<T: DeserializeOwned>(&self, user_id: i64, url: Url, action: &str) -> Result<T, TrackerError> {
		let response = self.send_authed(user_id, Method::GET, url, None, action).await?;
		Self::decode_json(response, action).await
	}

	async fn get_public_json<T: DeserializeOwned>(&self, url: Url, action: &str) -> Result<T, TrackerError> {
		let response = self.send_public(url, action).await?;
		Self::decode_json(response, action).await
	}

	fn series_url_id(query: &str) -> Option<i64> {
		let url = super::parse_tracker_url(query)?;
		let host = url.host_str()?.to_ascii_lowercase();
		if host != "mangabaka.org" && !host.ends_with(".mangabaka.org") {
			return None;
		}
		url.path_segments()?.next()?.parse().ok()
	}

	async fn detail(&self, id: i64) -> Result<Series, TrackerError> {
		let envelope: Envelope<Series> =
			self.get_public_json(self.api_url(&["v1", "series", &id.to_string()])?, "MangaBaka series detail").await?;
		Ok(envelope.data)
	}

	fn search_item(&self, series: &Series, tracked: bool) -> TrackSearchItem {
		let title = if series.titles.is_empty() { series.title.clone() } else { primary_title(&series.titles, &self.title_language) };
		let media_type = match series.series_type.as_deref() {
			Some("novel") => Some(TrackMediaType::Novel),
			Some("manga" | "manhwa" | "manhua" | "oel") => Some(TrackMediaType::Manga),
			_ => None,
		};
		TrackSearchItem {
			id: series.id.to_string(),
			title,
			cover_url: series.cover.x350.as_ref().and_then(|cover| cover.x1.clone()).or_else(|| {
				series.cover.raw.as_ref().and_then(|cover| cover.url.clone())
			}),
			description: series.description.clone(),
			tracked,
			media_type,
			url: Some(format!("{SITE_ORIGIN}/{}", series.id)),
		}
	}

	async fn tracked_ids(&self, user_id: i64, ids: &[i64]) -> Result<HashSet<i64>, TrackerError> {
		if ids.is_empty() {
			return Ok(HashSet::new());
		}
		let mut url = self.api_url(&["v1", "my", "library", "batch"])?;
		{
			let mut query = url.query_pairs_mut();
			for id in ids {
				query.append_pair("series_id", &id.to_string());
			}
		}
		let envelope: Envelope<Vec<LibraryBatchEntry>> = self.get_authed_json(user_id, url, "MangaBaka library batch").await?;
		Ok(envelope.data.into_iter().map(|entry| entry.series_id).collect())
	}

	async fn library_entry(&self, user_id: i64, id: i64) -> Result<Option<LibraryEntry>, TrackerError> {
		let response = self
			.send_authed(user_id, Method::GET, self.api_url(&["v1", "my", "library", &id.to_string()])?, None, "MangaBaka library entry")
			.await?;
		if response.status() == StatusCode::NOT_FOUND {
			return Ok(None);
		}
		let envelope: Envelope<Option<LibraryEntry>> = Self::decode_json(response, "MangaBaka library entry").await?;
		Ok(envelope.data)
	}
}

#[async_trait::async_trait]
impl TrackerService for MangaBakaTracker {
	fn provider(&self) -> Provider {
		Provider::Mangabaka
	}

	async fn is_logged_in(&self, user_id: i64) -> Result<bool, TrackerError> {
		Ok(self.oauth.status(Provider::Mangabaka, user_id).await?.logged_in)
	}

	async fn search(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
		let series = if let Some(id) = Self::series_url_id(query) {
			vec![self.detail(id).await?]
		} else {
			let mut url = self.api_url(&["v1", "series", "search"])?;
			{
				let mut pairs = url.query_pairs_mut();
				pairs.append_pair("q", query);
				if !nsfw {
					pairs.append_pair("not_content_rating", "erotica").append_pair("not_content_rating", "pornographic");
				}
			}
			let envelope: Envelope<Vec<Series>> = self.get_public_json(url, "MangaBaka series search").await?;
			envelope.data
		};
		let ids = series.iter().map(|series| series.id).collect::<Vec<_>>();
		let tracked = self.tracked_ids(user_id, &ids).await?;
		Ok(series.iter().map(|series| self.search_item(series, tracked.contains(&series.id))).collect())
	}

	async fn get_state(&self, user_id: i64, id: &str) -> Result<RemoteState, TrackerError> {
		let id = track_id(id)?;
		let (total_chapters, total_volumes) = self.detail(id).await?.totals();
		let Some(entry) = self.library_entry(user_id, id).await? else {
			return Ok(RemoteState { total_chapters, total_volumes, ..RemoteState::default() });
		};
		Ok(RemoteState {
			score: entry.rating.map(|rating| rating.round() as i32),
			status: entry.state.as_deref().and_then(library_status),
			last_read_chapter: entry.progress_chapter.map(|chapter| chapter as f32),
			last_read_volume: entry.progress_volume,
			total_chapters,
			total_volumes,
			start_read_date: entry.start_date,
			finish_read_date: entry.finish_date,
		})
	}

	async fn update(&self, user_id: i64, id: &str, update: &TrackUpdate) -> Result<(), TrackerError> {
		let id = track_id(id)?;
		let mut body = serde_json::Map::new();
		if let Some(status) = update.status {
			body.insert("state".into(), json!(library_status_name(status)));
		}
		if let Some(score) = update.score {
			body.insert("rating".into(), json!(score));
		}
		if let Some(chapter) = update.last_read_chapter {
			body.insert("progress_chapter".into(), json!(chapter.floor() as i64));
		}
		if let Some(volume) = update.last_read_volume {
			body.insert("progress_volume".into(), json!(volume));
		}
		if let Some(date) = update.start_read_date.as_deref().filter(|date| supported_date(date)) {
			body.insert("start_date".into(), json!(date));
		}
		if let Some(date) = update.finish_read_date.as_deref().filter(|date| supported_date(date)) {
			body.insert("finish_date".into(), json!(date));
		}
		if body.is_empty() {
			return Ok(());
		}

		let exists = self.library_entry(user_id, id).await?.is_some();
		let method = if exists { Method::PATCH } else { Method::POST };
		let response = self
			.send_authed(
				user_id,
				method,
				self.api_url(&["v1", "my", "library", &id.to_string()])?,
				Some(&Value::Object(body)),
				"MangaBaka library update",
			)
			.await?;
		Self::ensure_success(response, "MangaBaka library update").await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tracker::providers::test_support::{self, CapturedRequest};
	use axum::extract::{Request, State};
	use axum::http::{Method, StatusCode};
	use axum::response::{IntoResponse, Response};
	use axum::{Json, Router};
	use std::sync::Mutex;

	#[derive(Debug, Clone)]
	struct Recorded {
		request: CapturedRequest,
		query: String,
	}

	type Responder = dyn Fn(&CapturedRequest) -> (StatusCode, Value) + Send + Sync;

	#[derive(Clone)]
	struct Mock {
		requests: Arc<Mutex<Vec<Recorded>>>,
		responder: Arc<Responder>,
	}

	impl Mock {
		fn requests(&self) -> Vec<Recorded> {
			self.requests.lock().unwrap().clone()
		}
	}

	async fn handle(State(mock): State<Mock>, request: Request) -> Response {
		let query = request.uri().query().unwrap_or_default().to_string();
		let captured = test_support::capture(request).await;
		let (status, body) = (mock.responder)(&captured);
		mock.requests.lock().unwrap().push(Recorded { request: captured, query });
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

	fn query_values(recorded: &Recorded, key: &str) -> Vec<String> {
		url::form_urlencoded::parse(recorded.query.as_bytes())
			.filter(|(name, _)| name == key)
			.map(|(_, value)| value.into_owned())
			.collect()
	}

	async fn test_service(origin: &str) -> (MangaBakaTracker, i64, tempfile::TempDir) {
		let (oauth, pool, tmp) = test_support::test_oauth(Provider::Mangabaka, origin).await;
		let user_id = sqlx::query_scalar::<_, i64>("INSERT INTO user (username, password_hash) VALUES ('reader', 'x') RETURNING id")
			.fetch_one(&pool)
			.await
			.unwrap();
		test_support::seed_token(&pool, Provider::Mangabaka, user_id).await;
		(MangaBakaTracker::with_api_origin(reqwest::Client::new(), oauth, origin), user_id, tmp)
	}

	fn series(id: i64) -> Value {
		json!({
			"id": id,
			"title": "Fallback",
			"titles": [
				{"language": "en", "title": format!("English {id}"), "is_primary": true},
				{"language": "ja", "title": format!("日本語 {id}"), "traits": ["native"]},
				{"language": "ja-Latn", "title": format!("Romaji {id}")}
			],
			"cover": {
				"raw": {"url": format!("https://img.test/{id}-raw.jpg")},
				"x350": {"x1": format!("https://img.test/{id}.jpg")}
			},
			"description": format!("Description {id}"),
			"status": "completed",
			"type": if id == 8 { "novel" } else { "manga" },
			"total_chapters": "20",
			"final_volume": "2"
		})
	}

	#[tokio::test]
	async fn public_search_uses_batch_tracking_and_title_language() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/v1/series/search" => (StatusCode::OK, json!({"data": [series(7), series(8)]})),
			"/v1/my/library/batch" => (StatusCode::OK, json!({"data": [{"series_id": 7}]})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;
		let service = service.with_title_language("ja");
		assert_eq!(service.provider(), Provider::Mangabaka);
		assert!(service.is_logged_in(user).await.unwrap());

		let results = service.search(user, "A title / 検索", false).await.unwrap();
		assert_eq!(results.len(), 2);
		assert_eq!(results[0].title, "日本語 7");
		assert!(results[0].tracked);
		assert_eq!(results[0].cover_url.as_deref(), Some("https://img.test/7.jpg"));
		assert_eq!(results[0].media_type, Some(TrackMediaType::Manga));
		assert_eq!(results[0].url.as_deref(), Some("https://mangabaka.org/7"));
		assert!(!results[1].tracked);
		assert_eq!(results[1].media_type, Some(TrackMediaType::Novel));
		let requests = mock.requests();
		let search = requests.iter().find(|recorded| recorded.request.path == "/v1/series/search").unwrap();
		assert_eq!(search.request.method, Method::GET);
		assert_eq!(search.request.authorization, None);
		assert_eq!(query_values(search, "q"), vec!["A title / 検索"]);
		assert_eq!(query_values(search, "not_content_rating"), vec!["erotica", "pornographic"]);
		let batch = requests.iter().find(|recorded| recorded.request.path == "/v1/my/library/batch").unwrap();
		assert_eq!(batch.request.authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(query_values(batch, "series_id"), vec!["7", "8"]);

		let default_title: Series = serde_json::from_value(series(9)).unwrap();
		let (plain, _, _keep) = test_service(&origin).await;
		assert_eq!(plain.search_item(&default_title, false).title, "English 9");
		let romaji = plain.with_title_language("ja-ro");
		assert_eq!(romaji.search_item(&default_title, false).title, "Romaji 9");
	}

	#[tokio::test]
	async fn url_search_fetches_public_detail_without_search_request() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/v1/series/77" => (StatusCode::OK, json!({"data": series(77)})),
			"/v1/my/library/batch" => (StatusCode::OK, json!({"data": []})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;

		let results = service.search(user, "mangabaka.org/77", true).await.unwrap();
		assert_eq!(results.len(), 1);
		assert_eq!(results[0].id, "77");
		let requests = mock.requests();
		let detail = requests.iter().find(|recorded| recorded.request.path == "/v1/series/77").unwrap();
		assert_eq!(detail.request.authorization, None);
		assert!(!requests.iter().any(|recorded| recorded.request.path == "/v1/series/search"));
	}

	#[tokio::test]
	async fn state_maps_library_dates_progress_and_releasing_totals() {
		let (origin, _) = serve(|request| match request.path.as_str() {
			"/v1/series/42" => (StatusCode::OK, json!({"data": series(42)})),
			"/v1/my/library/42" => (
				StatusCode::OK,
				json!({"data": {
					"rating": 86.6,
					"state": "considering",
					"progress_chapter": 14,
					"progress_volume": 2,
					"start_date": "2026-10-01",
					"finish_date": "2026-10-20"
				}}),
			),
			"/v1/series/43" => (
				StatusCode::OK,
				json!({"data": {
					"id": 43,
					"title": "Releasing",
					"status": "releasing",
					"total_chapters": "999",
					"final_volume": "99"
				}}),
			),
			"/v1/my/library/43" => (StatusCode::OK, json!({"data": {"state": "reading", "progress_chapter": 1}})),
			"/v1/series/44" => (StatusCode::OK, json!({"data": series(44)})),
			"/v1/my/library/44" => (StatusCode::NOT_FOUND, json!({"message": "not in library", "data": null})),
			"/v1/series/45" => (StatusCode::OK, json!({"data": series(45)})),
			"/v1/my/library/45" => (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "library unavailable"})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;

		let state = service.get_state(user, "42").await.unwrap();
		assert_eq!(state.score, Some(87));
		assert_eq!(state.status, Some(TrackStatus::Planning));
		assert_eq!(state.last_read_chapter, Some(14.0));
		assert_eq!(state.last_read_volume, Some(2));
		assert_eq!(state.total_chapters, Some(20));
		assert_eq!(state.total_volumes, Some(2));
		assert_eq!(state.start_read_date.as_deref(), Some("2026-10-01"));
		assert_eq!(state.finish_read_date.as_deref(), Some("2026-10-20"));

		let releasing = service.get_state(user, "43").await.unwrap();
		assert_eq!(releasing.total_chapters, None);
		assert_eq!(releasing.total_volumes, None);
		assert_eq!(releasing.status, Some(TrackStatus::Reading));
		let uncollected = service.get_state(user, "44").await.unwrap();
		assert_eq!(uncollected.total_chapters, Some(20));
		assert_eq!(uncollected.total_volumes, Some(2));
		assert_eq!(uncollected.status, None);
		assert!(matches!(
			service.get_state(user, "45").await,
			Err(TrackerError::Provider { provider: Provider::Mangabaka, .. })
		));
	}

	#[tokio::test]
	async fn search_propagates_library_batch_failure() {
		let (origin, _) = serve(|request| match request.path.as_str() {
			"/v1/series/search" => (StatusCode::OK, json!({"data": [series(7)]})),
			"/v1/my/library/batch" => (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "library unavailable"})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;

		assert!(matches!(
			service.search(user, "Book", false).await,
			Err(TrackerError::Provider { provider: Provider::Mangabaka, .. })
		));
	}

	#[tokio::test]
	async fn state_propagates_series_detail_failure() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/v1/series/42" => (StatusCode::INTERNAL_SERVER_ERROR, json!({"message": "series unavailable"})),
			"/v1/my/library/42" => (StatusCode::OK, json!({"data": {"state": "reading", "progress_chapter": 1}})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;

		assert!(matches!(
			service.get_state(user, "42").await,
			Err(TrackerError::Provider { provider: Provider::Mangabaka, .. })
		));
		assert!(!mock.requests().iter().any(|recorded| recorded.request.path == "/v1/my/library/42"));
	}

	#[tokio::test]
	async fn update_creates_missing_entry_and_maps_all_fields() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/v1/my/library/42" if request.method == Method::GET => {
				(StatusCode::NOT_FOUND, json!({"message": "not in library", "data": null}))
			}
			"/v1/my/library/42" => (StatusCode::OK, json!({"data": {}})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;
		let update = TrackUpdate {
			score: Some(75),
			status: Some(TrackStatus::Planning),
			last_read_chapter: Some(10.9),
			last_read_volume: Some(3),
			start_read_date: Some("2026-10-01".into()),
			finish_read_date: Some("2026-10-20".into()),
		};

		service.update(user, "42", &update).await.unwrap();
		let requests = mock.requests();
		let post = requests.iter().find(|recorded| recorded.request.method == Method::POST).unwrap();
		assert_eq!(post.request.path, "/v1/my/library/42");
		assert_eq!(post.request.authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(
			post.request.json(),
			json!({
				"state": "plan_to_read",
				"rating": 75,
				"progress_chapter": 10,
				"progress_volume": 3,
				"start_date": "2026-10-01",
				"finish_date": "2026-10-20"
			})
		);
	}

	#[tokio::test]
	async fn final_unauthorized_logs_out() {
		let (origin, _) = serve(|request| match request.path.as_str() {
			"/token" => (StatusCode::OK, json!({"access_token": "fresh", "expires_in": 3600})),
			_ => (StatusCode::UNAUTHORIZED, json!({"message": "expired"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;
		let update = TrackUpdate { last_read_chapter: Some(1.0), ..TrackUpdate::default() };
		assert!(matches!(service.update(user, "42", &update).await, Err(TrackerError::NotLoggedIn { .. })));
		assert!(!service.oauth.status(Provider::Mangabaka, user).await.unwrap().logged_in);
	}

	#[tokio::test]
	async fn update_patches_existing_entry_and_refreshes_once() {
		let (origin, mock) = serve(|request| match request.path.as_str() {
			"/token" => (
				StatusCode::OK,
				json!({"access_token": "fresh", "refresh_token": "refresh-new", "expires_in": 3600}),
			),
			"/v1/my/library/42" if request.method == Method::GET => (StatusCode::OK, json!({"data": {"state": "reading"}})),
			"/v1/my/library/42" if request.authorization.as_deref() == Some("Bearer old-token") => {
				(StatusCode::UNAUTHORIZED, json!({"message": "expired"}))
			}
			"/v1/my/library/42" => (StatusCode::OK, json!({"data": {}})),
			_ => (StatusCode::NOT_FOUND, json!({"message": "unexpected"})),
		})
		.await;
		let (service, user, _tmp) = test_service(&origin).await;
		let update = TrackUpdate {
			score: Some(91),
			status: Some(TrackStatus::Rereading),
			last_read_chapter: Some(5.9),
			start_read_date: Some("1970-01-01".into()),
			finish_read_date: Some("2026-10-20".into()),
			..TrackUpdate::default()
		};

		service.update(user, "42", &update).await.unwrap();
		let requests = mock.requests();
		let patches = requests.iter().filter(|recorded| recorded.request.method == Method::PATCH).collect::<Vec<_>>();
		assert_eq!(patches.len(), 2);
		assert_eq!(patches[0].request.authorization.as_deref(), Some("Bearer old-token"));
		assert_eq!(patches[1].request.authorization.as_deref(), Some("Bearer fresh"));
		for recorded in patches {
			assert_eq!(recorded.request.path, "/v1/my/library/42");
			assert_eq!(
				recorded.request.json(),
				json!({
					"state": "rereading",
					"rating": 91,
					"progress_chapter": 5,
					"finish_date": "2026-10-20"
				})
			);
		}
		assert!(!requests.iter().any(|recorded| recorded.request.method == Method::POST && recorded.request.path == "/v1/my/library/42"));
		let token = requests.iter().find(|recorded| recorded.request.path == "/token").unwrap();
		let form = token.request.form();
		assert_eq!(form["grant_type"], "refresh_token");
		assert_eq!(form["refresh_token"], "refresh-old");
	}
}
