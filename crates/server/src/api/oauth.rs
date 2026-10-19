//! 源的 OAuth 登录（Login setting，method: "oauth"）。
//!
//! Aidoku 里 OAuth 由 app 层完成（SettingView.swift）：源只声明授权地址与 PKCE/
//! token_url，宿主构建授权 URL、跑 web-auth 会话、拿回调里的 code 换令牌，结果串
//! 存在设置键下。浏览器 SPA 拦不到自定义 scheme 的重定向（callback_scheme 通常
//! 是私有 scheme，ASWebAuthenticationSession 的等价物不存在），所以回调这一步
//! 由用户手动粘贴完成：start 给出授权 URL（新标签打开），complete 解析粘贴的
//! 回调串，两条分支与 Aidoku 对齐——
//! - pkce 且声明 token_url：POST grant_type=authorization_code&code&code_verifier
//!   [&redirect_uri][&client_id]（参数不 percent-encode，redirect_uri/client_id
//!   捕获自授权地址的 query）到 token_url，原始响应串存进 <key>；
//! - 否则：回调串存进 <key>，并送达设置的 notification。

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

/// 令牌响应的读取上限：正常是几百字节的 JSON，超限说明 token_url 指错了地方，
/// 无上限会把异常响应写进设置库
const TOKEN_BODY_MAX: usize = 1024 * 1024;
/// 交换卡在用户交互路径上（「完成」按钮在等），不能沿默认的无限等待
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
	/// 用户粘贴的回调 URL（授权完成后浏览器地址栏里的重定向串）
	pub url: String,
}

/// verifier：Aidoku generateCodeVerifier 同构，128 字符取自 unreserved 集
fn generate_code_verifier() -> String {
	const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
	let mut verifier = String::with_capacity(128);
	let mut buf = [0u8; 128];
	while verifier.len() < 128 {
		OsRng.fill_bytes(&mut buf);
		// 198 是不超 256 的 66 的最大整数倍，之外的丢弃，消取模偏差
		for &b in &buf {
			if b < 198 && verifier.len() < 128 {
				verifier.push(CHARSET[(b % 66) as usize] as char);
			}
		}
	}
	verifier
}

/// challenge：非填充 base64url(SHA-256(verifier))，Aidoku generateCodeChallenge 同构
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

/// 构建授权 URL：pkce 时先把 verifier 落库（complete 换令牌要用，跨重启也在），
/// 再在既有 query 后追加 code_challenge 三件套（Aidoku 的 URLComponents 语义）
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

/// 交换分支的请求体：参数逐值拼接不 percent-encode（Aidoku 的
/// `parameters.map { "k=v" }.joined("&")` 同构）；redirect_uri/client_id
/// 捕获自授权地址的 query（与 start 同一套三级解析，缺哪个省哪个）
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
	// Aidoku 不看 HTTP 状态码：响应体原文落库（错误页也照存），保持同口径
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
		// 回调串即登录态（源自己解析里面的 token/code），存规范化后的绝对串（Aidoku 的
		// callback.absoluteString）；notification 失败只记日志，与 Aidoku 的 do/catch 同口径
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
