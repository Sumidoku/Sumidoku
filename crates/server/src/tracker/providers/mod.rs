pub mod anilist;
pub mod bangumi;
pub mod mal;
pub mod mangabaka;

#[cfg(test)]
pub(super) mod test_support {
	use crate::tracker::oauth::{OAuthApp, OAuthManager};
	use crate::tracker::{Provider, dao};
	use axum::Router;
	use axum::extract::Request;
	use axum::http::{HeaderMap, Method};
	use sqlx::SqlitePool;
	use std::collections::HashMap;
	use std::sync::Arc;

	#[derive(Debug, Clone)]
	pub(super) struct CapturedRequest {
		pub(super) method: Method,
		pub(super) path: String,
		query: Vec<(String, String)>,
		pub(super) authorization: Option<String>,
		pub(super) content_type: Option<String>,
		pub(super) body: Vec<u8>,
	}

	impl CapturedRequest {
		pub(super) fn query(&self, name: &str) -> Option<&str> {
			self.query.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
		}

		pub(super) fn json(&self) -> serde_json::Value {
			serde_json::from_slice(&self.body).unwrap()
		}

		pub(super) fn form(&self) -> HashMap<String, String> {
			url::form_urlencoded::parse(&self.body).into_owned().collect()
		}
	}

	pub(super) async fn capture(request: Request) -> CapturedRequest {
		let (parts, body) = request.into_parts();
		let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
		CapturedRequest {
			method: parts.method,
			path: parts.uri.path().to_string(),
			query: url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes()).into_owned().collect(),
			authorization: header(&parts.headers, reqwest::header::AUTHORIZATION),
			content_type: header(&parts.headers, reqwest::header::CONTENT_TYPE),
			body: bytes.to_vec(),
		}
	}

	fn header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
		headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
	}

	pub(super) async fn serve(router: Router) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		format!("http://{address}")
	}

	pub(super) async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		(tmp, pool)
	}

	pub(super) async fn test_oauth(provider: Provider, base_url: &str) -> (Arc<OAuthManager>, SqlitePool, tempfile::TempDir) {
		let (tmp, pool) = test_pool().await;
		let mut app = OAuthApp::for_provider(provider);
		app.client_id = Some(format!("{}-test-id", provider.as_str()));
		app.client_secret = Some(format!("{}-test-secret", provider.as_str()));
		app.authorize_url = format!("{base_url}/authorize");
		app.token_url = format!("{base_url}/token");
		app.userinfo_url = format!("{base_url}/user");
		let apps = HashMap::from([(provider, app)]);
		let manager = OAuthManager::with_apps(pool.clone(), reqwest::Client::new(), crate::tracker::oauth::OAUTH_RELAY_ROOT.to_string(), apps);
		(manager, pool, tmp)
	}

	pub(super) async fn seed_token(pool: &SqlitePool, provider: Provider, user_id: i64) {
		dao::upsert_token(
			pool,
			provider,
			user_id,
			&dao::TrackerOAuthToken {
				access_token: "old-token".into(),
				refresh_token: Some("refresh-old".into()),
				token_type: Some("Bearer".into()),
				expires_at: None,
				username: Some("test-user".into()),
			},
		)
		.await
		.unwrap();
	}
}
