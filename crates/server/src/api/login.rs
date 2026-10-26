//! Web login for sources (Login setting, method: "web") and source-level logout.
//!
//! The server has no webview, so the login-state cookies are pasted by the user from a browser (a Cookie header or
//! an exported JSON). The pipeline (`run_web_login_pipeline`: store keys/values first, stamp the marker only after
//! success, call the source's `handle_web_login` export, persist cookies into `source_cookie`, then restart the instance).
//! All persistence goes through synchronous SQL: the sink channel is an async batch writer, and the restart right after would read stale values from the DB.

use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, load_defaults};
use aidoku_wasm::DefaultValue;
use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebLoginRequest {
	pub key: String,
	pub cookies: HashMap<String, String>,
	#[serde(default)]
	pub local_storage: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoutRequest {
	pub key: String,
	#[serde(default)]
	pub clear_cookies: bool,
}

/// The fields of a Login setting the server cares about (normalized nested shape; the url tri-state resolves the login host)
pub struct LoginSettingInfo {
	pub method: String,
	pub url: Option<String>,
	pub url_key: Option<String>,
	pub pkce: bool,
	pub token_url: Option<String>,
	/// The Aidoku app layer uses it to intercept the web-auth callback; the server is a manual-paste flow, so it is kept only as a setting portrait
	pub callback_scheme: Option<String>,
	/// Setting-level notification: delivered after the OAuth callback-string branch stores its value (same as Aidoku)
	pub notification: Option<String>,
	pub clear_cookies_on_log_out: bool,
}

/// Recurses group/page containers for the login setting matching key; a key colliding with a non-login setting counts as absent
pub(crate) fn find_login_setting(items: &[Value], key: &str) -> Option<LoginSettingInfo> {
	for item in items {
		let Some(value) = item.get("value") else {
			continue;
		};
		if let Some(container) = value.get("Group").or_else(|| value.get("Page")) {
			if let Some(children) = container.get("items").and_then(Value::as_array)
				&& let Some(found) = find_login_setting(children, key)
			{
				return Some(found);
			}
			continue;
		}
		if item.get("key").and_then(Value::as_str) != Some(key) {
			continue;
		}
		let login = value.get("Login")?;
		return Some(LoginSettingInfo {
			method: login
				.get("method")
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string(),
			url: login.get("url").and_then(Value::as_str).map(str::to_string),
			url_key: login
				.get("url_key")
				.and_then(Value::as_str)
				.map(str::to_string),
			pkce: login.get("pkce").and_then(Value::as_bool).unwrap_or(false),
			token_url: login
				.get("token_url")
				.and_then(Value::as_str)
				.map(str::to_string),
			callback_scheme: login
				.get("callback_scheme")
				.and_then(Value::as_str)
				.map(str::to_string),
			notification: item
				.get("notification")
				.and_then(Value::as_str)
				.map(str::to_string),
			clear_cookies_on_log_out: login
				.get("clear_cookies_on_log_out")
				.and_then(Value::as_bool)
				.unwrap_or(false),
		});
	}
	None
}

/// Three-level resolution of the login URL: the login setting's url → the defaults value url_key points to → the source base_url
pub(crate) async fn resolve_login_url(
	state: &AppState,
	id: i64,
	setting: &LoginSettingInfo,
	base_url: Option<&str>,
) -> ApiResult<String> {
	let mut url = setting.url.clone();
	if url.is_none()
		&& let Some(url_key) = &setting.url_key
	{
		let defaults = load_defaults(&state.pool, id).await?;
		if let Some(DefaultValue::String(value)) = defaults.get(url_key) {
			url = Some(value.clone());
		}
	}
	if url.is_none() {
		url = base_url.map(str::to_string);
	}
	url.ok_or_else(|| ApiError::bad_request("login setting has no url to resolve the login host"))
}

/// The login host comes from the login URL; an invalid URL gets the same 400 convention as #112
fn host_of(url: &str) -> ApiResult<String> {
	url::Url::parse(url)
		.ok()
		.and_then(|parsed| parsed.host_str().map(str::to_string))
		.ok_or_else(|| ApiError::bad_request(format!("invalid login url: {url}")))
}

async fn resolve_login_host(
	state: &AppState,
	id: i64,
	setting: &LoginSettingInfo,
	base_url: Option<&str>,
) -> ApiResult<String> {
	let url = resolve_login_url(state, id, setting, base_url).await?;
	host_of(&url)
}

/// The jar splices name=value straight into the Cookie header, so separators and control characters must be stopped at the entrance
fn validate_cookies(cookies: &HashMap<String, String>) -> ApiResult<()> {
	for (name, value) in cookies {
		if name.is_empty()
			|| name
				.chars()
				.any(|c| c == ';' || c == '=' || c.is_control())
			|| value.chars().any(|c| c == ';' || c.is_control())
		{
			return Err(ApiError::bad_request("invalid cookie name or value"));
		}
	}
	Ok(())
}

pub(crate) async fn upsert_setting(pool: &SqlitePool, source_id: i64, key: &str, value: DefaultValue) -> ApiResult<()> {
	let encoded = serde_json::to_string(&value)?;
	sqlx::query(
		"INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)
		 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
	)
	.bind(source_id)
	.bind(key)
	.bind(encoded)
	.execute(pool)
	.await?;
	Ok(())
}

pub async fn web_login(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<WebLoginRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	validate_cookies(&body.cookies)?;
	let row = super::source_row(&state, id).await?;
	let handle = state.get_handle(id).await?;
	let items = super::load_settings_items(&state, id, &handle).await?;
	let Some(setting) = find_login_setting(&items, &body.key) else {
		return Err(ApiError::bad_request("login setting not found for key"));
	};
	if setting.method != "web" {
		return Err(ApiError::bad_request("login setting is not web login"));
	}
	let host = resolve_login_host(&state, id, &setting, row.base_url.as_deref()).await?;
	run_web_login_pipeline(&state, id, &body.key, &host, &handle, &body.cookies, &body.local_storage).await?;
	Ok(Json(json!({ "ok": true })))
}

/// The #112 pipeline core: keys/values (sorted by cookie name, the same order the host layer pre-fills) and
/// localStorage land in the DB first, then the export is called (Aidoku order; a source veto does not roll back the
/// stored part, and no export means skipping validation), then the marker is stamped + source_cookie written (empty
/// cookies mean logout semantics), and finally the instance restarts.
/// All persistence goes through synchronous SQL: the sink channel is an async batch writer, and the restart right after would read stale values from the DB
pub(crate) async fn run_web_login_pipeline(
	state: &Arc<AppState>,
	id: i64,
	key: &str,
	host: &str,
	handle: &aidoku_wasm::SourceHandle,
	cookies: &HashMap<String, String>,
	local_storage: &HashMap<String, String>,
) -> ApiResult<()> {
	let mut names: Vec<&String> = cookies.keys().collect();
	names.sort();
	let keys: Vec<String> = names.iter().map(|n| n.to_string()).collect();
	let values: Vec<String> = names.iter().map(|n| cookies[*n].clone()).collect();
	upsert_setting(&state.pool, id, &format!("{key}.keys"), DefaultValue::StringArray(keys)).await?;
	upsert_setting(&state.pool, id, &format!("{key}.values"), DefaultValue::StringArray(values)).await?;
	for (ls_key, ls_value) in local_storage {
		upsert_setting(&state.pool, id, &format!("{key}.ls.{ls_key}"), DefaultValue::String(ls_value.clone())).await?;
	}

	// When the source does not export handle_web_login, skip validation and store directly (same as Aidoku)
	if handle.features().web_login
		&& !handle.handle_web_login(key.to_string(), cookies.clone()).await?
	{
		return Err(ApiError::bad_gateway("web login failed"));
	}

	if cookies.is_empty() {
		// Empty cookies mean logout semantics: the marker and that host's cookies are cleared together
		sqlx::query("DELETE FROM source_setting WHERE source_id = ? AND key = ?")
			.bind(id)
			.bind(key)
			.execute(&state.pool)
			.await?;
		sqlx::query("DELETE FROM source_cookie WHERE source_id = ? AND domain = ?")
			.bind(id)
			.bind(host)
			.execute(&state.pool)
			.await?;
	} else {
		upsert_setting(&state.pool, id, key, DefaultValue::String("logged_in".into())).await?;
		for (name, value) in cookies {
			sqlx::query(
				"INSERT INTO source_cookie (source_id, domain, name, value) VALUES (?, ?, ?, ?)
				 ON CONFLICT (source_id, domain, name) DO UPDATE SET value = excluded.value",
			)
			.bind(id)
			.bind(host)
			.bind(name)
			.bind(value)
			.execute(&state.pool)
			.await?;
		}
	}
	super::restart_if_loaded(state, id).await?;
	Ok(())
}

pub async fn logout(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<LogoutRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let row = super::source_row(&state, id).await?;
	// One prefix delete covers every child key like .keys/.values/.ls.*/.username/.password;
	// the instr prefix match avoids LIKE wildcard escaping
	sqlx::query("DELETE FROM source_setting WHERE source_id = ? AND (key = ? OR instr(key, ? || '.') = 1)")
		.bind(id)
		.bind(&body.key)
		.bind(&body.key)
		.execute(&state.pool)
		.await?;

	// The setting lookup is best-effort: logout must work even when the instance cannot start (e.g. the source is disabled); the setting state was already cleared above
	let setting = match state.get_handle(id).await {
		Ok(handle) => super::load_settings_items(&state, id, &handle)
			.await
			.ok()
			.and_then(|items| find_login_setting(&items, &body.key)),
		Err(_) => None,
	};
	let clear = body.clear_cookies
		|| setting
			.as_ref()
			.map(|s| s.clear_cookies_on_log_out)
			.unwrap_or(false);
	if clear {
		// When the host cannot be resolved, clear the whole source jar (an explicit confirm is the user's intent, same convention as cache/clear)
		let host = match &setting {
			Some(s) => resolve_login_host(&state, id, s, row.base_url.as_deref()).await.ok(),
			None => None,
		};
		match host {
			Some(host) => {
				sqlx::query("DELETE FROM source_cookie WHERE source_id = ? AND domain = ?")
					.bind(id)
					.bind(&host)
					.execute(&state.pool)
					.await?;
			}
			None => {
				sqlx::query("DELETE FROM source_cookie WHERE source_id = ?")
					.bind(id)
					.execute(&state.pool)
					.await?;
			}
		}
	}
	super::restart_if_loaded(&state, id).await?;
	Ok(Json(json!({ "ok": true })))
}
