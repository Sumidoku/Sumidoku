//! 源的网页登录（Login setting，method: "web"）与源级登出。
//!
//! 服务端没有 webview，登录态 cookie 由用户从浏览器粘贴（Cookie 头或导出
//! 的 JSON），这里负责按 Aidoku 的顺序落库（keys/values 先存、marker 成功后
//! 才打）、调源的 `handle_web_login` 导出、把 cookie 持久化进 `source_cookie`
//! 并重启实例。持久化全部走同步 SQL：sink channel 是异步批量写，紧随其后的
//! 重启会从库里读到旧值。

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

/// Login setting 里服务端关心的字段（归一化后的嵌套形状，url 三态用于解析登录 host）
pub struct LoginSettingInfo {
	pub method: String,
	pub url: Option<String>,
	pub url_key: Option<String>,
	pub clear_cookies_on_log_out: bool,
}

/// 递归 group/page 容器找 key 对应的 login 设置；key 撞在非 login 设置上视同不存在
fn find_login_setting(items: &[Value], key: &str) -> Option<LoginSettingInfo> {
	for item in items {
		let value = item.get("value")?;
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
			clear_cookies_on_log_out: login
				.get("clear_cookies_on_log_out")
				.and_then(Value::as_bool)
				.unwrap_or(false),
		});
	}
	None
}

/// 登录 host 的三级解析：login 设置的 url → url_key 指向的 defaults 值 → 源 base_url
async fn resolve_login_host(
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
	let url = url.ok_or_else(|| ApiError::bad_request("login setting has no url to resolve the login host"))?;
	url::Url::parse(&url)
		.ok()
		.and_then(|parsed| parsed.host_str().map(str::to_string))
		.ok_or_else(|| ApiError::bad_request(format!("invalid login url: {url}")))
}

/// jar 会把 name=value 直接拼进 Cookie 头，分隔符与控制字符必须拦在入口
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

async fn upsert_setting(pool: &SqlitePool, source_id: i64, key: &str, value: DefaultValue) -> ApiResult<()> {
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

	// Aidoku 顺序：keys/values（按 cookie 名排序，与宿主层预置同序）与
	// localStorage 先落库，再调导出，源否决不回滚这部分
	let mut names: Vec<&String> = body.cookies.keys().collect();
	names.sort();
	let keys: Vec<String> = names.iter().map(|n| (*n).clone()).collect();
	let values: Vec<String> = names.iter().map(|n| body.cookies[*n].clone()).collect();
	upsert_setting(&state.pool, id, &format!("{}.keys", body.key), DefaultValue::StringArray(keys)).await?;
	upsert_setting(
		&state.pool,
		id,
		&format!("{}.values", body.key),
		DefaultValue::StringArray(values),
	)
	.await?;
	for (ls_key, ls_value) in &body.local_storage {
		upsert_setting(
			&state.pool,
			id,
			&format!("{}.ls.{ls_key}", body.key),
			DefaultValue::String(ls_value.clone()),
		)
		.await?;
	}

	// 源不导出 handle_web_login 时跳过校验直接存（Aidoku 同款）
	if handle.features().web_login
		&& !handle
			.handle_web_login(body.key.clone(), body.cookies.clone())
			.await?
	{
		return Err(ApiError::bad_gateway("web login failed"));
	}

	if body.cookies.is_empty() {
		// 空 cookies 即登出语义：marker 与该 host 的 cookie 一起清
		sqlx::query("DELETE FROM source_setting WHERE source_id = ? AND key = ?")
			.bind(id)
			.bind(&body.key)
			.execute(&state.pool)
			.await?;
		sqlx::query("DELETE FROM source_cookie WHERE source_id = ? AND domain = ?")
			.bind(id)
			.bind(&host)
			.execute(&state.pool)
			.await?;
	} else {
		upsert_setting(&state.pool, id, &body.key, DefaultValue::String("logged_in".into())).await?;
		for (name, value) in &body.cookies {
			sqlx::query(
				"INSERT INTO source_cookie (source_id, domain, name, value) VALUES (?, ?, ?, ?)
				 ON CONFLICT (source_id, domain, name) DO UPDATE SET value = excluded.value",
			)
			.bind(id)
			.bind(&host)
			.bind(name)
			.bind(value)
			.execute(&state.pool)
			.await?;
		}
	}
	super::restart_if_loaded(&state, id).await?;
	Ok(Json(json!({ "ok": true })))
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
	// 前缀删除一次覆盖 .keys/.values/.ls.*/.username/.password 等全部子键；
	// instr 前缀匹配避开 LIKE 通配符转义
	sqlx::query("DELETE FROM source_setting WHERE source_id = ? AND (key = ? OR instr(key, ? || '.') = 1)")
		.bind(id)
		.bind(&body.key)
		.bind(&body.key)
		.execute(&state.pool)
		.await?;

	// 设置查找尽力而为：实例起不来（如源被禁用）也要能登出，设置态上面已清
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
		// host 解析不出时清整源 jar（显式 confirm 即用户授意，口径同 cache/clear）
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
