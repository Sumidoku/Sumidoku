//! OAuth login for sources (Login setting, method: "oauth").
//!
//! In Aidoku, OAuth is completed at the app layer (SettingView.swift): the source only declares the authorization
//! address and PKCE/token_url, and the host builds the authorization URL, runs the web-auth session, exchanges the
//! code from the callback for a token, and stores the result string under the setting key. A browser SPA cannot
//! intercept a custom-scheme redirect (callback_scheme is usually a private scheme, and there is no
//! ASWebAuthenticationSession equivalent), so the callback step is done by the user pasting it manually: start
//! returns the authorization URL (opened in a new tab), complete parses the pasted callback string — two branches,
//! aligned with Aidoku:
//! - pkce with a declared token_url: POST grant_type=authorization_code&code&code_verifier
//!   [&redirect_uri][&client_id] (parameters are not percent-encoded; redirect_uri/client_id are captured from the
//!   authorization address's query) to token_url, and the raw response string is stored under <key>;
//! - otherwise: the callback string is stored under <key> and the setting's notification is delivered.

use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, load_defaults};
use aidoku_wasm::DefaultValue;
use axum::Json;
use axum::extract::{Path, State};
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::Arc;

use super::login::{LoginSettingInfo, find_login_setting, resolve_login_url, upsert_setting};

/// Read cap for the token response: normally a few hundred bytes of JSON, so exceeding it means token_url points
/// at the wrong place; without a cap a pathological response would be written into the settings DB
const TOKEN_BODY_MAX: usize = 1024 * 1024;
/// The exchange sits on the user-interaction path (the "done" button is waiting), so it cannot inherit the default unbounded wait
const TOKEN_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartRequest {
	pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCompleteRequest {
	pub key: String,
	/// The callback URL pasted by the user (the redirect string in the browser's address bar after authorization completes)
	pub url: String,
}

/// verifier: isomorphic with Aidoku's generateCodeVerifier, 128 characters from the unreserved set
fn generate_code_verifier() -> String {
	const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
	let mut verifier = String::with_capacity(128);
	let mut buf = [0u8; 128];
	while verifier.len() < 128 {
		OsRng.fill_bytes(&mut buf);
		// 198 is the largest multiple of 66 not exceeding 256; values beyond it are discarded, removing modulo bias
		for &b in &buf {
			if b < 198 && verifier.len() < 128 {
				verifier.push(CHARSET[(b % 66) as usize] as char);
			}
		}
	}
	verifier
}

/// challenge: unpadded base64url(SHA-256(verifier)), isomorphic with Aidoku's generateCodeChallenge
fn code_challenge(verifier: &str) -> String {
	base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

async fn resolve_oauth_setting(
	state: &Arc<AppState>,
	id: i64,
	key: &str,
) -> ApiResult<(LoginSettingInfo, Arc<aidoku_wasm::SourceHandle>)> {
	let handle = state.get_handle(id).await?;
	let items = super::load_settings_items(state, id, &handle).await?;
	let Some(setting) = find_login_setting(&items, key) else {
		return Err(ApiError::bad_request("login setting not found for key"));
	};
	if setting.method != "oauth" {
		return Err(ApiError::bad_request("login setting is not oauth login"));
	}
	Ok((setting, handle))
}

/// Builds the authorization URL: with pkce, first persist the verifier (complete needs it for the token exchange,
/// and it survives restarts), then append the code_challenge trio after any existing query (Aidoku's URLComponents semantics)
pub async fn oauth_start(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<OAuthStartRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let row = super::source_row(&state, id).await?;
	let (setting, _handle) = resolve_oauth_setting(&state, id, &body.key).await?;
	let url = resolve_login_url(&state, id, &setting, row.base_url.as_deref()).await?;
	let mut url = url::Url::parse(&url).map_err(|_| ApiError::bad_request(format!("invalid login url: {url}")))?;
	if setting.pkce {
		let verifier = generate_code_verifier();
		upsert_setting(
			&state.pool,
			id,
			&format!("{}.codeVerifier", body.key),
			DefaultValue::String(verifier.clone()),
		)
		.await?;
		url.query_pairs_mut()
			.append_pair("code_challenge", &code_challenge(&verifier))
			.append_pair("code_challenge_method", "S256")
			.append_pair("response_type", "code");
	}
	Ok(Json(json!({ "url": url.to_string() })))
}

/// The exchange branch's request body: parameters concatenated value by value without percent-encoding (isomorphic
/// with Aidoku's `parameters.map { "k=v" }.joined("&")`); redirect_uri/client_id are captured from the
/// authorization address's query (same three-level resolution as start; whichever is missing is omitted)
async fn exchange_code(
	state: &Arc<AppState>,
	id: i64,
	setting: &LoginSettingInfo,
	base_url: Option<&str>,
	code: &str,
	verifier: &str,
) -> ApiResult<String> {
	let token_url = setting
		.token_url
		.as_deref()
		.expect("exchange branch requires token_url");
	let auth_url = resolve_login_url(state, id, setting, base_url).await?;
	let auth_url = url::Url::parse(&auth_url).map_err(|_| ApiError::bad_request(format!("invalid login url: {auth_url}")))?;
	let (mut redirect_uri, mut client_id) = (None, None);
	for (name, value) in auth_url.query_pairs() {
		match name.as_ref() {
			"redirect_uri" => redirect_uri = Some(value.into_owned()),
			"client_id" => client_id = Some(value.into_owned()),
			_ => {}
		}
	}
	let mut body = format!("grant_type=authorization_code&code={code}&code_verifier={verifier}");
	if let Some(redirect_uri) = redirect_uri {
		body.push_str("&redirect_uri=");
		body.push_str(&redirect_uri);
	}
	if let Some(client_id) = client_id {
		body.push_str("&client_id=");
		body.push_str(&client_id);
	}
	let token_url = url::Url::parse(token_url).map_err(|_| ApiError::bad_request(format!("invalid token url: {token_url}")))?;
	let request = state
		.http
		.request(reqwest::Method::POST, &token_url)
		.map_err(|e| ApiError::bad_gateway(format!("oauth token request failed: {e}")))?
		.header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
		.body(body);
	let fetch = async {
		let mut response = request
			.send()
			.await
			.map_err(|e| ApiError::bad_gateway(format!("oauth token request failed: {e}")))?;
		let mut out = Vec::new();
		while let Some(chunk) = response
			.chunk()
			.await
			.map_err(|e| ApiError::bad_gateway(format!("oauth token request failed: {e}")))?
		{
			if out.len() + chunk.len() > TOKEN_BODY_MAX {
				return Err(ApiError::bad_gateway("oauth token response too large"));
			}
			out.extend_from_slice(&chunk);
		}
		Ok(out)
	};
	let bytes = match tokio::time::timeout(TOKEN_FETCH_TIMEOUT, fetch).await {
		Ok(result) => result?,
		Err(_) => return Err(ApiError::bad_gateway("oauth token request timed out")),
	};
	// Aidoku ignores the HTTP status: the response body is stored verbatim (even an error page), keeping the same convention
	Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub async fn oauth_complete(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<OAuthCompleteRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let pasted = body.url.trim();
	if pasted.is_empty() {
		return Err(ApiError::bad_request("url must not be empty"));
	}
	let row = super::source_row(&state, id).await?;
	let (setting, handle) = resolve_oauth_setting(&state, id, &body.key).await?;
	let callback = url::Url::parse(pasted).map_err(|_| ApiError::bad_request(format!("invalid callback url: {pasted}")))?;

	if setting.pkce && setting.token_url.is_some() {
		let defaults = load_defaults(&state.pool, id).await?;
		let verifier_key = format!("{}.codeVerifier", body.key);
		let Some(DefaultValue::String(verifier)) = defaults.get(&verifier_key) else {
			return Err(ApiError::bad_request("oauth start has not been called for key"));
		};
		let code = callback
			.query_pairs()
			.find(|(name, _)| name == "code")
			.map(|(_, value)| value.into_owned())
			.ok_or_else(|| ApiError::bad_request("callback url has no code parameter"))?;
		let result = exchange_code(&state, id, &setting, row.base_url.as_deref(), &code, verifier).await?;
		upsert_setting(&state.pool, id, &body.key, DefaultValue::String(result)).await?;
		super::restart_if_loaded(&state, id).await?;
	} else {
		// The callback string is the login state (the source parses the token/code out of it); store the normalized
		// absolute string (Aidoku's callback.absoluteString); a notification failure is only logged, same as Aidoku's do/catch
		upsert_setting(
			&state.pool,
			id,
			&body.key,
			DefaultValue::String(callback.to_string()),
		)
		.await?;
		super::restart_if_loaded(&state, id).await?;
		if let Some(notification) = &setting.notification
			&& handle.features().notification
			&& let Err(e) = handle.handle_notification(notification.clone()).await
		{
			tracing::warn!(source_id = id, "oauth login notification failed: {e}");
		}
	}
	let values = super::current_values(&state, id).await?;
	Ok(Json(json!({ "values": values })))
}
