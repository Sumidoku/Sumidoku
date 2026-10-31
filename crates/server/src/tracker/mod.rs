//! Tracker integration (#35): provider contracts, the service registry and the
//! per-user OAuth manager. Decision rules live in `core` (pure, unit-tested);
//! provider HTTP contracts live in `providers`; `sync` pushes reading progress
//! as it changes; `dao` owns the SQLite rows.

pub mod core;
pub mod dao;
pub mod metadata;
pub mod oauth;
pub mod providers;
pub mod sync;

use core::{RemoteState, TrackSearchItem, TrackUpdate};
use metadata::TrackerMetadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
	Anilist,
	Mal,
	Bangumi,
	Mangabaka,
}

impl Provider {
	pub const ALL: [Self; 4] = [Self::Anilist, Self::Mal, Self::Bangumi, Self::Mangabaka];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Anilist => "anilist",
			Self::Mal => "mal",
			Self::Bangumi => "bangumi",
			Self::Mangabaka => "mangabaka",
		}
	}

	pub fn parse(name: &str) -> Option<Self> {
		match name {
			"anilist" => Some(Self::Anilist),
			"mal" => Some(Self::Mal),
			"bangumi" => Some(Self::Bangumi),
			"mangabaka" => Some(Self::Mangabaka),
			_ => None,
		}
	}
}

impl std::fmt::Display for Provider {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.as_str())
	}
}

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
	#[error("{provider} tracker: not logged in")]
	NotLoggedIn { provider: Provider },
	#[error("{provider} tracker: {message}")]
	Provider { provider: Provider, message: String },
	#[error("{provider} OAuth endpoint returned {status}: {message}")]
	Upstream { provider: Provider, status: reqwest::StatusCode, message: String },
	#[error(transparent)]
	Database(#[from] sqlx::Error),
	#[error("{provider} OAuth client id is not configured at build time ({variable})")]
	MissingClientId { provider: Provider, variable: &'static str },
	#[error("{provider} OAuth client secret is not configured at build time ({variable})")]
	MissingClientSecret { provider: Provider, variable: &'static str },
	#[error("invalid OAuth state")]
	InvalidState,
	#[error("OAuth authorization session is missing or expired")]
	PendingExpired,
}

#[async_trait::async_trait]
pub trait TrackerService: Send + Sync {
	fn provider(&self) -> Provider;
	async fn is_logged_in(&self, user_id: i64) -> Result<bool, TrackerError>;

	async fn search(&self, user_id: i64, query: &str, nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError>;

	async fn get_state(&self, user_id: i64, track_id: &str) -> Result<RemoteState, TrackerError>;

	async fn update(&self, user_id: i64, track_id: &str, update: &TrackUpdate) -> Result<(), TrackerError>;

	/// Full bibliographic metadata of one entry, for writing library metadata
	/// from a tracker. Providers without this capability keep the default.
	async fn metadata(&self, _user_id: i64, _track_id: &str) -> Result<TrackerMetadata, TrackerError> {
		Err(TrackerError::Provider { provider: self.provider(), message: "metadata is not supported by this tracker".into() })
	}

	/// True when `metadata` reads public data, so a user without this tracker's
	/// login can still preview a match. Providers whose metadata call is
	/// authenticated override this.
	fn metadata_is_public(&self) -> bool {
		false
	}
}

#[derive(Clone)]
pub struct TrackerServices {
	oauth: std::sync::Arc<oauth::OAuthManager>,
	services: std::sync::Arc<std::collections::HashMap<Provider, std::sync::Arc<dyn TrackerService>>>,
}

impl TrackerServices {
	pub fn new(
		oauth: std::sync::Arc<oauth::OAuthManager>,
		services: Vec<std::sync::Arc<dyn TrackerService>>,
	) -> Self {
		let services = services.into_iter().map(|service| (service.provider(), service)).collect();
		Self { oauth, services: std::sync::Arc::new(services) }
	}

	pub fn from_db(pool: sqlx::SqlitePool) -> Self {
		let http = reqwest::Client::builder()
			.user_agent(concat!("sumidoku/", env!("CARGO_PKG_VERSION")))
			.timeout(std::time::Duration::from_secs(30))
			.build()
			.expect("tracker HTTP client");
		let oauth = oauth::OAuthManager::new(pool, http.clone());
		let services: Vec<std::sync::Arc<dyn TrackerService>> = vec![
			std::sync::Arc::new(providers::anilist::AniListTracker::new(http.clone(), oauth.clone())),
			std::sync::Arc::new(providers::mal::MalTracker::new(http.clone(), oauth.clone())),
			std::sync::Arc::new(providers::bangumi::BangumiTracker::new(http.clone(), oauth.clone())),
			std::sync::Arc::new(providers::mangabaka::MangaBakaTracker::new(http, oauth.clone())),
		];
		Self::new(oauth, services)
	}

	/// A registry without providers, for tests that only exercise OAuth state.
	pub fn empty(pool: sqlx::SqlitePool) -> Self {
		Self::new(oauth::OAuthManager::new(pool, reqwest::Client::new()), Vec::new())
	}

	pub fn oauth(&self) -> &std::sync::Arc<oauth::OAuthManager> {
		&self.oauth
	}

	pub fn get(&self, provider: Provider) -> Option<std::sync::Arc<dyn TrackerService>> {
		self.services.get(&provider).cloned()
	}

	pub fn all(&self) -> Vec<std::sync::Arc<dyn TrackerService>> {
		Provider::ALL.into_iter().filter_map(|provider| self.get(provider)).collect()
	}

	pub fn is_empty(&self) -> bool {
		self.services.is_empty()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	struct StubTracker;

	#[async_trait::async_trait]
	impl TrackerService for StubTracker {
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
			Ok(RemoteState::default())
		}

		async fn update(&self, _user_id: i64, _track_id: &str, _update: &TrackUpdate) -> Result<(), TrackerError> {
			Ok(())
		}
	}

	#[test]
	fn provider_names_match_existing_routes() {
		for provider in Provider::ALL {
			assert_eq!(Provider::parse(provider.as_str()), Some(provider));
			assert_eq!(serde_json::to_value(provider).unwrap(), serde_json::Value::String(provider.as_str().into()));
		}
		for alias in ["myanimelist", "bgm", "mb", "AniList", "unknown"] {
			assert_eq!(Provider::parse(alias), None);
		}
		let error = TrackerError::NotLoggedIn { provider: Provider::Anilist };
		assert_eq!(error.to_string(), "anilist tracker: not logged in");
	}

	#[tokio::test]
	async fn registry_is_clonable_and_dispatches_object_safely() {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let empty = TrackerServices::empty(pool.clone());
		assert!(empty.is_empty());
		assert!(empty.get(Provider::Anilist).is_none());

		let oauth = oauth::OAuthManager::new(pool, reqwest::Client::new());
		let services = TrackerServices::new(oauth.clone(), vec![std::sync::Arc::new(StubTracker)]);
		assert!(std::sync::Arc::ptr_eq(services.oauth(), &oauth));
		let cloned = services.clone();
		let tracker = cloned.get(Provider::Anilist).unwrap();
		assert!(tracker.is_logged_in(1).await.unwrap());
		assert!(tracker.search(1, "title", true).await.unwrap().is_empty());
		assert_eq!(tracker.get_state(1, "42").await.unwrap(), RemoteState::default());
		tracker.update(1, "42", &TrackUpdate::default()).await.unwrap();
		assert_eq!(services.all().len(), 1);
		assert!(services.get(Provider::Mal).is_none());
	}
}
