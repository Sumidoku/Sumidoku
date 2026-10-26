//! Placeholder until the provider is ported.

use crate::tracker::core::{RemoteState, TrackSearchItem, TrackUpdate};
use crate::tracker::oauth::OAuthManager;
use crate::tracker::{Provider, TrackerError, TrackerService};
use std::sync::Arc;

pub struct MangaBakaTracker {
	#[allow(dead_code)]
	http: reqwest::Client,
	#[allow(dead_code)]
	oauth: Arc<OAuthManager>,
}

impl MangaBakaTracker {
	pub fn new(http: reqwest::Client, oauth: Arc<OAuthManager>) -> Self {
		Self { http, oauth }
	}
}

#[async_trait::async_trait]
impl TrackerService for MangaBakaTracker {
	fn provider(&self) -> Provider {
		Provider::Mangabaka
	}

	fn is_logged_in(&self, _user_id: i64) -> Result<bool, TrackerError> {
		Ok(false)
	}

	async fn search(&self, _user_id: i64, _query: &str, _nsfw: bool) -> Result<Vec<TrackSearchItem>, TrackerError> {
		Err(TrackerError::Provider { provider: Provider::Mangabaka, message: "provider not implemented".into() })
	}

	async fn get_state(&self, _user_id: i64, _track_id: &str) -> Result<RemoteState, TrackerError> {
		Err(TrackerError::Provider { provider: Provider::Mangabaka, message: "provider not implemented".into() })
	}

	async fn update(&self, _user_id: i64, _track_id: &str, _update: &TrackUpdate) -> Result<(), TrackerError> {
		Err(TrackerError::Provider { provider: Provider::Mangabaka, message: "provider not implemented".into() })
	}
}
