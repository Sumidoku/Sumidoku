//! 源的网页登录（Login setting，method: "web"）与源级登出。
//!
//! 服务端没有 webview，登录态 cookie 两条来路：用户从浏览器粘贴（Cookie 头或导出
//! 的 JSON），或配置 `net.browser_ws` 后由 sidecar 交互登录收割（服务端开浏览器
//! context 加载登录页，screencast/输入经 WS 中继给用户，完成后读 context 的
//! cookie）。两条来路汇进同一条管线（`run_web_login_pipeline`：keys/values 先存、
//! marker 成功后才打、调源的 `handle_web_login` 导出、cookie 持久化进
//! `source_cookie` 并重启实例）。持久化全部走同步 SQL：sink channel 是异步批量写，
//! 紧随其后的重启会从库里读到旧值。

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
	pub pkce: bool,
	pub token_url: Option<String>,
	/// Aidoku app 层用它拦截 web-auth 回调；服务端是手动粘贴流，仅留作设置画像
	pub callback_scheme: Option<String>,
	/// 设置级 notification：OAuth 回调串分支存完值后送达（Aidoku 同款）
	pub notification: Option<String>,
	pub clear_cookies_on_log_out: bool,
	/// 设置声明的 localStorage 键；sidecar 收割时按它从页面读取
	pub local_storage_keys: Vec<String>,
}

/// 递归 group/page 容器找 key 对应的 login 设置；key 撞在非 login 设置上视同不存在
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
			local_storage_keys: login
				.get("local_storage_keys")
				.and_then(Value::as_array)
				.map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect())
				.unwrap_or_default(),
		});
	}
	None
}

/// 登录 URL 的三级解析：login 设置的 url → url_key 指向的 defaults 值 → 源 base_url
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

/// 登录 host 取自登录 URL；URL 非法时与 #112 同口径 400
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

/// #112 管线核心，粘贴与 sidecar 收割共用：keys/values（按 cookie 名排序，与宿主层
/// 预置同序）与 localStorage 先落库，再调导出（Aidoku 顺序，源否决不回滚已存部分；
/// 无导出跳过校验），然后打 marker + 写 source_cookie（空 cookies 即登出清态），
/// 最后重启实例。持久化全部走同步 SQL：sink channel 是异步批量写，紧随其后的
/// 重启会从库里读到旧值
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
	let keys: Vec<String> = names.iter().map(|n| (*n).clone()).collect();
	let values: Vec<String> = names.iter().map(|n| cookies[*n].clone()).collect();
	upsert_setting(&state.pool, id, &format!("{key}.keys"), DefaultValue::StringArray(keys)).await?;
	upsert_setting(&state.pool, id, &format!("{key}.values"), DefaultValue::StringArray(values)).await?;
	for (ls_key, ls_value) in local_storage {
		upsert_setting(&state.pool, id, &format!("{key}.ls.{ls_key}"), DefaultValue::String(ls_value.clone())).await?;
	}

	// 源不导出 handle_web_login 时跳过校验直接存（Aidoku 同款）
	if handle.features().web_login
		&& !handle.handle_web_login(key.to_string(), cookies.clone()).await?
	{
		return Err(ApiError::bad_gateway("web login failed"));
	}

	if cookies.is_empty() {
		// 空 cookies 即登出语义：marker 与该 host 的 cookie 一起清
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

// ---- sidecar 交互登录：服务端开浏览器 context，用户经 WS 中继完成登录 ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginStartRequest {
	pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginSessionRequest {
	pub key: String,
	pub session: String,
}

fn browser_manager(state: &AppState) -> ApiResult<&crate::browser_login::BrowserLoginManager> {
	let manager = state
		.browser_logins
		.get()
		.ok_or_else(|| ApiError::unsupported_501("browser sidecar not configured"))?;
	if !manager.configured() {
		return Err(ApiError::unsupported_501("browser sidecar not configured"));
	}
	Ok(manager)
}

/// 开会话：先把 setting/host 验掉再开浏览器 context（白开 context 是对 sidecar 的浪费），
/// 返回句柄与实际导航的登录 URL，用户交互经 stream WS 中继
pub async fn browser_login_start(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<BrowserLoginStartRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let manager = browser_manager(&state)?;
	let row = super::source_row(&state, id).await?;
	let handle = state.get_handle(id).await?;
	let items = super::load_settings_items(&state, id, &handle).await?;
	let Some(setting) = find_login_setting(&items, &body.key) else {
		return Err(ApiError::bad_request("login setting not found for key"));
	};
	if setting.method != "web" {
		return Err(ApiError::bad_request("login setting is not web login"));
	}
	let url = resolve_login_url(&state, id, &setting, row.base_url.as_deref()).await?;
	let host = host_of(&url)?;
	let session = manager.start(id, &body.key, &host, &url).await?;
	let expires_at = crate::model::unix_now() + manager.ttl().as_secs() as i64;
	Ok(Json(json!({ "session": session, "loginUrl": url, "expiresAt": expires_at })))
}

/// 完成：收割 context 里登录 host（含父子域）的 cookie，localStorage 按设置声明键
/// 尽力而为读取，然后进 #112 共享管线。会话无论成败都已终结（一次性语义）：
/// client 的 current_thread runtime 在 async 上下文 drop 会 panic，任何出口都必须
/// 经 discard 在 blocking 线程池里释放
pub async fn browser_login_complete(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Json(body): Json<BrowserLoginSessionRequest>,
) -> ApiResult<Json<Value>> {
	if body.key.is_empty() {
		return Err(ApiError::bad_request("key must not be empty"));
	}
	let manager = browser_manager(&state)?;
	let Some(session) = manager.take(&body.session) else {
		return Err(ApiError::not_found("login session not found"));
	};
	if session.source_id != id || session.key != body.key {
		manager.discard(session, crate::browser_login::SessionEnd::Cancelled).await;
		return Err(ApiError::bad_request("login session does not match the source or key"));
	}
	let result = harvest_and_run_pipeline(&state, id, &body.key, &session).await;
	let end = match &result {
		Ok(()) => crate::browser_login::SessionEnd::Completed,
		Err(_) => crate::browser_login::SessionEnd::Cancelled,
	};
	manager.discard(session, end).await;
	result?;
	Ok(Json(json!({ "ok": true })))
}

async fn harvest_and_run_pipeline(
	state: &Arc<AppState>,
	id: i64,
	key: &str,
	session: &crate::browser_login::SessionRef,
) -> ApiResult<()> {
	let harvested = session.call(|client, context| client.cookies(context)).await?;
	let cookies = harvest_host_cookies(harvested, &session.host);
	if cookies.is_empty() {
		return Err(ApiError::bad_request("no cookies found for the login host"));
	}
	validate_cookies(&cookies)?;

	let handle = state.get_handle(id).await?;
	let items = super::load_settings_items(state, id, &handle).await?;
	let ls_keys = find_login_setting(&items, key)
		.map(|setting| setting.local_storage_keys)
		.unwrap_or_default();
	let mut local_storage = HashMap::new();
	if !ls_keys.is_empty() {
		match session
			.call(move |client, context| client.local_storage_values(context, &ls_keys))
			.await
		{
			Ok(pairs) => local_storage = pairs.into_iter().collect(),
			// 页面已被用户关掉等场景：cookie 才是登录态本体，localStorage 缺失不拖垮登录
			Err(e) => tracing::warn!(source_id = id, "browser login localStorage harvest skipped: {}", e.message()),
		}
	}

	run_web_login_pipeline(state, id, key, &session.host, &handle, &cookies, &local_storage).await
}

/// 取消是清理语义：句柄即授权（不可猜），未知句柄也幂等 200
pub async fn browser_login_cancel(
	State(state): State<Arc<AppState>>,
	Path(_id): Path<i64>,
	Json(body): Json<BrowserLoginSessionRequest>,
) -> ApiResult<Json<Value>> {
	let manager = browser_manager(&state)?;
	if let Some(session) = manager.take(&body.session) {
		manager.discard(session, crate::browser_login::SessionEnd::Cancelled).await;
	}
	Ok(Json(json!({ "ok": true })))
}

/// CDP cookie 的 domain 带点前缀与子域常态，收割窗口放宽到登录 host 的父子域；
/// 跨域 SSO（如 accounts.google.com）不在此列，极端情况仍走粘贴路径
fn harvest_host_cookies(cookies: Vec<aidoku_wasm::browser::BrowserCookie>, host: &str) -> HashMap<String, String> {
	let mut out = HashMap::new();
	for cookie in cookies {
		let domain = cookie.domain.trim_start_matches('.');
		let matched = domain == host || domain.ends_with(&format!(".{host}")) || host.ends_with(&format!(".{domain}"));
		if matched {
			out.insert(cookie.name, cookie.value);
		}
	}
	out
}

// ---- 交互中继：帧出、输入进 ----

use aidoku_wasm::browser::{KeyInput, KeyKind, MouseButton, MouseInput, MouseKind};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MouseMessage {
	event: String,
	x: f64,
	y: f64,
	#[serde(default)]
	button: String,
	#[serde(default)]
	click_count: i64,
	#[serde(default)]
	delta_x: f64,
	#[serde(default)]
	delta_y: f64,
	#[serde(default)]
	modifiers: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyMessage {
	event: String,
	key: String,
	#[serde(default)]
	code: String,
	#[serde(default)]
	windows_virtual_key_code: i64,
	#[serde(default)]
	modifiers: i64,
}

/// 帧中继与输入回灌的 WS 端点；会话不存在或归属不符按 404（句柄不可猜，404 不泄露）
pub async fn browser_login_stream(
	State(state): State<Arc<AppState>>,
	Path((id, session_id)): Path<(i64, String)>,
	ws: WebSocketUpgrade,
) -> Response {
	let reference = state
		.browser_logins
		.get()
		.and_then(|manager| manager.session(&session_id))
		.filter(|reference| reference.source_id == id);
	let Some(reference) = reference else {
		return ApiError::not_found("login session not found").into_response();
	};
	ws.on_upgrade(move |socket| stream_loop(socket, reference))
}

async fn send_frame(socket: &mut WebSocket, frame: &str) -> Result<(), axum::Error> {
	// frame 是 base64，不含 JSON 转义字符，手工拼省一次大字符串克隆
	socket
		.send(Message::Text(format!("{{\"type\":\"frame\",\"data\":\"{frame}\"}}").into()))
		.await
}

async fn send_closed(socket: &mut WebSocket, end: crate::browser_login::SessionEnd) {
	let _ = socket
		.send(Message::Text(
			format!("{{\"type\":\"closed\",\"reason\":\"{}\"}}", end.reason()).into(),
		))
		.await;
}

async fn stream_loop(mut socket: WebSocket, reference: crate::browser_login::SessionRef) {
	let mut frames = reference.frames.clone();
	let mut ends = reference.ends.clone();
	// 补发已有最新帧：attach 晚于开流时第一眼不是空白
	let initial = frames.borrow_and_update().clone();
	if !initial.is_empty() && send_frame(&mut socket, &initial).await.is_err() {
		return;
	}
	// attach 晚于会话终结的边界：立刻告知，不挂死；watch::Ref 非 Send，先拷出再 await
	let ended = *ends.borrow_and_update();
	if let Some(end) = ended {
		send_closed(&mut socket, end).await;
		return;
	}
	loop {
		tokio::select! {
			changed = frames.changed() => {
				if changed.is_err() {
					return;
				}
				let frame = frames.borrow_and_update().clone();
				if !frame.is_empty() && send_frame(&mut socket, &frame).await.is_err() {
					return;
				}
			}
			changed = ends.changed() => {
				if changed.is_err() {
					return;
				}
				let ended = *ends.borrow_and_update();
				if let Some(end) = ended {
					send_closed(&mut socket, end).await;
				}
				return;
			}
			message = socket.recv() => {
				match message {
					Some(Ok(Message::Text(text))) => handle_stream_input(&reference, &text).await,
					Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
					Some(Ok(_)) => {}
				}
			}
		}
	}
}

/// 单条输入的转发；单次失败（页面已关等）只记日志，不断流不杀会话
async fn handle_stream_input(reference: &crate::browser_login::SessionRef, text: &str) {
	let Ok(value) = serde_json::from_str::<Value>(text) else {
		return;
	};
	let result = match value.get("type").and_then(Value::as_str) {
		Some("mouse") => match serde_json::from_value::<MouseMessage>(value) {
			Ok(message) => {
				let kind = match message.event.as_str() {
					"down" => MouseKind::Down,
					"up" => MouseKind::Up,
					"move" => MouseKind::Move,
					"wheel" => MouseKind::Wheel,
					_ => return,
				};
				let button = match message.button.as_str() {
					"left" => MouseButton::Left,
					"middle" => MouseButton::Middle,
					"right" => MouseButton::Right,
					_ => MouseButton::None,
				};
				let input = MouseInput {
					kind,
					x: message.x,
					y: message.y,
					button,
					click_count: message.click_count,
					delta_x: message.delta_x,
					delta_y: message.delta_y,
					modifiers: message.modifiers,
				};
				reference.call(move |client, context| client.dispatch_mouse(context, &input)).await
			}
			Err(_) => return,
		},
		Some("key") => match serde_json::from_value::<KeyMessage>(value) {
			Ok(message) => {
				let kind = match message.event.as_str() {
					"down" => KeyKind::Down,
					"up" => KeyKind::Up,
					_ => return,
				};
				let input = KeyInput {
					kind,
					key: message.key,
					code: message.code,
					windows_virtual_key_code: message.windows_virtual_key_code,
					modifiers: message.modifiers,
				};
				reference.call(move |client, context| client.dispatch_key(context, &input)).await
			}
			Err(_) => return,
		},
		Some("text") => match value.get("text").and_then(Value::as_str) {
			Some(text) if !text.is_empty() => {
				let text = text.to_string();
				reference.call(move |client, context| client.insert_text(context, &text)).await
			}
			_ => return,
		},
		_ => return,
	};
	if let Err(e) = result {
		tracing::debug!("browser login input dropped: {}", e.message());
	}
}
