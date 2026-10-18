use crate::{
	WasmEnv,
	runtime::store::{FFIResult, ImageData, Ptr, Rid, StoreItem},
	traits::NetRoute,
};
use image::ImageReader;
use reqwest::{
	StatusCode,
	header::{ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, SET_COOKIE, USER_AGENT},
};
use crate::traits::CookieSink;
use std::{collections::HashMap, io::Cursor, str::FromStr, sync::Arc, time::Duration};
use url::Url;
use wasmer::FunctionEnvMut;

const FALLBACK_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
/// Aidoku 宿主由 URLSession 按系统语言隐式提供 Accept-Language；reqwest 不会，
/// 源没设时补一个默认值（部分 WAF 对缺失该头的请求直接 403）
const DEFAULT_ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";
const MAX_ATTEMPTS: u32 = 3;

enum Result {
	Success,
	InvalidDescriptor,
	InvalidString,
	InvalidMethod,
	InvalidUrl,
	MissingData,
	MissingResponse,
	RequestError,
	FailedMemoryWrite,
	NotAnImage,
}

impl From<Result> for i32 {
	fn from(result: Result) -> Self {
		match result {
			Result::Success => 0,
			Result::InvalidDescriptor => -1,
			Result::InvalidString => -2,
			Result::InvalidMethod => -3,
			Result::InvalidUrl => -4,
			Result::MissingData => -7,
			Result::MissingResponse => -8,
			Result::RequestError => -10,
			Result::FailedMemoryWrite => -11,
			Result::NotAnImage => -12,
		}
	}
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum HttpMethod {
	Get,
	Post,
	Put,
	Head,
	Delete,
	Patch,
	Options,
	Connect,
	Trace,
}

impl From<HttpMethod> for reqwest::Method {
	fn from(method: HttpMethod) -> Self {
		match method {
			HttpMethod::Get => reqwest::Method::GET,
			HttpMethod::Post => reqwest::Method::POST,
			HttpMethod::Put => reqwest::Method::PUT,
			HttpMethod::Delete => reqwest::Method::DELETE,
			HttpMethod::Head => reqwest::Method::HEAD,
			HttpMethod::Patch => reqwest::Method::PATCH,
			HttpMethod::Options => reqwest::Method::OPTIONS,
			HttpMethod::Connect => reqwest::Method::CONNECT,
			HttpMethod::Trace => reqwest::Method::TRACE,
		}
	}
}

#[derive(Debug)]
pub struct NetResponse {
	pub url: Url,
	pub status: StatusCode,
	pub headers: HeaderMap,
	pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct NetRequest {
	pub method: HttpMethod,
	pub url: Option<Url>,
	pub headers: HeaderMap,
	pub body: Option<Vec<u8>>,
	pub timeout: Option<f64>,
	pub response: Option<NetResponse>,
}

impl NetRequest {
	pub fn new(method: HttpMethod) -> Self {
		Self {
			method,
			url: None,
			headers: HeaderMap::new(),
			body: None,
			timeout: None,
			response: None,
		}
	}
}

#[derive(Default)]
pub struct CookieJar {
	domains: HashMap<String, HashMap<String, String>>,
	sink: Option<Arc<dyn CookieSink>>,
}

impl CookieJar {
	pub fn new(domains: HashMap<String, HashMap<String, String>>, sink: Arc<dyn CookieSink>) -> Self {
		Self {
			domains,
			sink: Some(sink),
		}
	}

	pub fn header_for(&self, url: &Url) -> Option<String> {
		let host = url.host_str()?;
		let mut pairs = Vec::new();
		for (domain, cookies) in &self.domains {
			if host == domain || host.ends_with(&format!(".{domain}")) {
				for (name, value) in cookies {
					pairs.push(format!("{name}={value}"));
				}
			}
		}
		if pairs.is_empty() { None } else { Some(pairs.join("; ")) }
	}

	/// 浏览器回流的 cookie 按 Domain 属性直接落库（无 Set-Cookie 原文可解析）；
	/// 与 store 一样经 sink 持久化，否则登录态只活内存、重启即丢
	pub fn insert(&mut self, domain: &str, name: &str, value: &str) {
		if domain.is_empty() || name.is_empty() {
			return;
		}
		self.domains
			.entry(domain.to_string())
			.or_default()
			.insert(name.to_string(), value.to_string());
		if let Some(sink) = &self.sink {
			sink.set(domain, name, Some(value));
		}
	}

	pub fn store(&mut self, url: &Url, set_cookie: &str) {
		let mut parts = set_cookie.split(';');
		let Some(pair) = parts.next() else { return };
		let Some((name, value)) = pair.split_once('=') else { return };
		let (name, value) = (name.trim(), value.trim());
		if name.is_empty() {
			return;
		}
		let mut domain = url.host_str().unwrap_or_default().to_string();
		let mut expired = false;
		for attr in parts {
			let attr = attr.trim();
			let Some((k, v)) = attr.split_once('=') else { continue };
			match k.trim().to_ascii_lowercase().as_str() {
				"domain" => domain = v.trim().trim_start_matches('.').to_string(),
				"max-age" if v.trim().parse::<i64>().map(|s| s <= 0).unwrap_or(false) => {
					expired = true;
				}
				_ => {}
			}
		}
		let entry = self.domains.entry(domain.clone()).or_default();
		if expired {
			if entry.remove(name).is_some()
				&& let Some(sink) = &self.sink
			{
				sink.set(&domain, name, None);
			}
		} else {
			entry.insert(name.to_string(), value.to_string());
			if let Some(sink) = &self.sink {
				sink.set(&domain, name, Some(value));
			}
		}
	}
}

pub fn init(mut env: FunctionEnvMut<WasmEnv>, method: u8) -> FFIResult {
	let method = match method {
		0 => HttpMethod::Get,
		1 => HttpMethod::Post,
		2 => HttpMethod::Put,
		3 => HttpMethod::Head,
		4 => HttpMethod::Delete,
		5 => HttpMethod::Patch,
		6 => HttpMethod::Options,
		7 => HttpMethod::Connect,
		8 => HttpMethod::Trace,
		_ => return Result::InvalidMethod.into(),
	};
	env.data_mut()
		.store
		.store(StoreItem::Request(Box::new(NetRequest::new(method))))
}

/// perform 的失败面，映射到 send 的 FFI 错误码
#[derive(Debug)]
pub(crate) enum SendFailure {
	InvalidUrl,
	Request,
}

/// 按 policy 路由发出一个请求：429/5xx 退避重试、直连撞 CF 挑战经 trawl
/// 升级一次、响应 Set-Cookie 入 jar。
/// 任何 HTTP 响应（含非 2xx）都写回 request.response；只有网络层失败才报错。
pub(crate) fn perform(
	net: &mut crate::runtime::env::NetState,
	request: &mut NetRequest,
	timeout: Duration,
) -> std::result::Result<(), SendFailure> {
	let Some(url) = request.url.clone() else {
		return Err(SendFailure::InvalidUrl);
	};
	let mut route = net.policy.route_for(&url);
	if net.client_for(&route).is_err() {
		return Err(SendFailure::Request);
	}

	net.rate.acquire();

	// 请求头只组装一次：重试与 CF 升级重发同一份头，放循环里会把 jar
	// cookie 反复拼进 Cookie
	if !request.headers.contains_key(USER_AGENT) {
		let ua = net
			.policy
			.user_agent()
			.unwrap_or_else(|| FALLBACK_USER_AGENT.to_string());
		if let Ok(value) = HeaderValue::from_str(&ua) {
			request.headers.insert(USER_AGENT, value);
		}
	}
	if !request.headers.contains_key(ACCEPT_LANGUAGE) {
		request
			.headers
			.insert(ACCEPT_LANGUAGE, HeaderValue::from_static(DEFAULT_ACCEPT_LANGUAGE));
	}
	// 对齐 Aidoku：jar cookie 拼在源自带 Cookie 前面，而不是源设了就丢掉 jar
	if let Some(jar_header) = net.jar.header_for(&url) {
		let merged = match request.headers.get(COOKIE).and_then(|v| v.to_str().ok()) {
			Some(existing) if !existing.is_empty() => format!("{jar_header}; {existing}"),
			_ => jar_header,
		};
		if let Ok(value) = HeaderValue::from_str(&merged) {
			request.headers.insert(COOKIE, value);
		}
	}

	let mut attempt = 0;
	let mut escalated = false;
	loop {
		attempt += 1;
		let client = net.clients.get(&route).expect("client built above");
		let mut builder = client
			.request(request.method.into(), url.as_str())
			.headers(request.headers.clone())
			.timeout(timeout);
		// 重试也要带 body：take 会让第二次尝试（含 trawl 升级）丢 POST 载荷
		if let Some(body) = request.body.clone() {
			builder = builder.body(body);
		}
		let Ok(response) = builder.send() else {
			return Err(SendFailure::Request);
		};
		let status = response.status();
		// CF 升级先于退避：503 挑战页不该白烧直连的退避次数
		if !escalated
			&& route == NetRoute::Direct
			&& crate::traits::is_cf_challenge(status, response.headers())
			&& let Some(trawl_route) = net.policy.trawl_escalation(&url)
		{
			if net.client_for(&trawl_route).is_err() {
				return Err(SendFailure::Request);
			}
			route = trawl_route;
			escalated = true;
			attempt = 0;
			continue;
		}
		let retry_after = response
			.headers()
			.get(reqwest::header::RETRY_AFTER)
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse::<u64>().ok());
		if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && attempt < MAX_ATTEMPTS {
			let wait = retry_after
				.map(Duration::from_secs)
				.unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1)));
			std::thread::sleep(wait);
			continue;
		}

		let url = response.url().clone();
		let headers = response.headers().clone();
		let Ok(bytes) = response.bytes() else {
			return Err(SendFailure::Request);
		};

		for value in headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()) {
			net.jar.store(&url, value);
		}
		request.response = Some(NetResponse {
			url,
			status,
			headers,
			data: bytes.into(),
		});
		return Ok(());
	}
}

fn common_send(env: &mut FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let data = env.data_mut();
	let Some(request) = data.store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	let timeout = request
		.timeout
		.map(|t| Duration::from_secs_f64(t.max(0.0)))
		.map(|t| t.min(data.limits.net_timeout_max))
		.unwrap_or(data.limits.net_timeout_default);
	match perform(&mut data.net, request, timeout) {
		Ok(()) => Result::Success.into(),
		Err(SendFailure::InvalidUrl) => Result::InvalidUrl.into(),
		Err(SendFailure::Request) => Result::RequestError.into(),
	}
}

pub fn send(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	common_send(&mut env, rid)
}

pub fn send_all(mut env: FunctionEnvMut<WasmEnv>, rid_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(rids) = env.data().read_values::<Rid>(&env, rid_ptr, len) else {
		return Result::InvalidDescriptor.into();
	};
	let mut errors = Vec::new();
	let mut was_error = false;
	for rid in rids {
		let result = common_send(&mut env, rid);
		if result != Into::<i32>::into(Result::Success) {
			was_error = true;
		}
		errors.push(result);
	}
	if env.data().write_values(&env, rid_ptr, errors).is_err() {
		Result::FailedMemoryWrite.into()
	} else if was_error {
		Result::RequestError.into()
	} else {
		Result::Success.into()
	}
}

pub fn set_url(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, ptr: Ptr, len: u32) -> FFIResult {
	let Ok(string) = env.data().read_string(&env, ptr, len) else {
		return Result::InvalidString.into();
	};
	let Ok(url) = Url::parse(&string) else {
		return Result::InvalidUrl.into();
	};
	let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.url = Some(url);
	Result::Success.into()
}

pub fn set_header(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	key_ptr: Ptr,
	key_len: u32,
	val_ptr: Ptr,
	val_len: u32,
) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(val) = env.data().read_string(&env, val_ptr, val_len) else {
		return Result::InvalidString.into();
	};
	let Ok(name) = HeaderName::from_str(&key) else {
		return Result::InvalidString.into();
	};
	let Ok(value) = HeaderValue::from_str(&val) else {
		return Result::InvalidString.into();
	};
	let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.headers.insert(name, value);
	Result::Success.into()
}

pub fn set_body(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, ptr: Ptr, len: u32) -> FFIResult {
	let Ok(body) = env.data().read_bytes(&env, ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.body = Some(body);
	Result::Success.into()
}

pub fn set_timeout(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, value: f64) -> FFIResult {
	let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.timeout = Some(value);
	Result::Success.into()
}

fn with_response<T>(
	env: &mut FunctionEnvMut<WasmEnv>,
	rid: Rid,
	missing: Result,
	f: impl FnOnce(&NetResponse) -> T,
) -> std::result::Result<T, i32> {
	let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Err(Result::InvalidDescriptor.into());
	};
	let Some(response) = request.response.as_ref() else {
		return Err(missing.into());
	};
	Ok(f(response))
}

pub fn data_len(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	match with_response(&mut env, rid, Result::MissingResponse, |r| r.data.len() as i32) {
		Ok(v) => v,
		Err(e) => e,
	}
}

pub fn read_data(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, buffer: Ptr, size: u32) -> FFIResult {
	let data = match with_response(&mut env, rid, Result::MissingResponse, |r| r.data.clone()) {
		Ok(d) => d,
		Err(e) => return e,
	};
	if size as usize > data.len() {
		return Result::FailedMemoryWrite.into();
	}
	if env.data().write_buffer(&env, buffer, &data[..size as usize]).is_err() {
		Result::FailedMemoryWrite.into()
	} else {
		Result::Success.into()
	}
}

pub fn get_image(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let data = match with_response(&mut env, rid, Result::MissingResponse, |r| r.data.clone()) {
		Ok(d) => d,
		Err(e) => return e,
	};
	let cursor = Cursor::new(data);
	let Some(rgba) = ImageReader::new(cursor)
		.with_guessed_format()
		.ok()
		.and_then(|r| r.decode().ok())
		.map(|img| img.to_rgba8())
	else {
		return Result::NotAnImage.into();
	};
	let (width, height) = rgba.dimensions();
	let image = ImageData::from_rgba8(rgba.into_raw(), width, height);
	env.data_mut().store.store(StoreItem::ImageData(image))
}

pub fn get_status_code(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	match with_response(&mut env, rid, Result::MissingResponse, |r| r.status.as_u16() as i32) {
		Ok(v) => v,
		Err(e) => e,
	}
}

pub fn get_url(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let url = match with_response(&mut env, rid, Result::MissingResponse, |r| r.url.as_str().to_string()) {
		Ok(u) => u,
		Err(e) => return e,
	};
	env.data_mut().store.store(StoreItem::String(url))
}

pub fn get_header(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: Ptr, key_len: u32) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(name) = HeaderName::from_str(&key) else {
		return Result::InvalidString.into();
	};
	let value = match with_response(&mut env, rid, Result::MissingResponse, |r| {
		r.headers
			.get_all(&name)
			.iter()
			.filter_map(|v| String::from_utf8(v.as_bytes().to_vec()).ok())
			.collect::<Vec<_>>()
			.join(", ")
	}) {
		Ok(v) => v,
		Err(e) => return e,
	};
	if value.is_empty() {
		return Result::MissingData.into();
	}
	env.data_mut().store.store(StoreItem::String(value))
}

pub fn html(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let (text, url) = match with_response(&mut env, rid, Result::MissingResponse, |r| {
		(decode_body(&r.headers, &r.data), r.url.as_str().to_string())
	}) {
		Ok(p) => p,
		Err(e) => return e,
	};
	let document = crate::html_dom::HtmlDocument::parse(&text, Some(&url));
	env.data_mut().store.store(StoreItem::HtmlDocument(document))
}

/// 按响应体的 charset 解码：BOM → Content-Type → `<meta charset>` → UTF-8 兜底。
/// Aidoku 宿主由 URLSession/SwiftSoup 处理这一步，reqwest 不管，
/// 旧实现直接在字节上跑 from_utf8，非 UTF-8 页面被整页判失败
fn decode_body(headers: &HeaderMap, data: &[u8]) -> String {
	let (text, _, _) = detect_encoding(headers, data).decode(data);
	text.into_owned()
}

fn detect_encoding(headers: &HeaderMap, data: &[u8]) -> &'static encoding_rs::Encoding {
	if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
		return encoding_rs::UTF_8;
	}
	if data.starts_with(&[0xFF, 0xFE]) {
		return encoding_rs::UTF_16LE;
	}
	if data.starts_with(&[0xFE, 0xFF]) {
		return encoding_rs::UTF_16BE;
	}
	if let Some(enc) = headers
		.get(CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.and_then(charset_from_content_type)
		.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
	{
		return enc;
	}
	meta_charset(data).unwrap_or(encoding_rs::UTF_8)
}

fn charset_from_content_type(value: &str) -> Option<&str> {
	for part in value.split(';') {
		let part = part.trim();
		if let Some((key, val)) = part.split_once('=')
			&& key.trim().eq_ignore_ascii_case("charset")
		{
			let val = val.trim().trim_matches(['"', '\'']);
			if !val.is_empty() {
				return Some(val);
			}
		}
	}
	None
}

/// 从文档前 1024 字节里嗅探 `<meta charset>` / `<meta http-equiv=content-type>`
fn meta_charset(data: &[u8]) -> Option<&'static encoding_rs::Encoding> {
	let head: String = data[..data.len().min(1024)].iter().map(|&b| char::from(b)).collect();
	let idx = head.to_ascii_lowercase().find("charset")?;
	let after = head[idx + "charset".len()..].trim_start();
	let after = after.strip_prefix('=')?.trim_start();
	let label = if let Some(rest) = after.strip_prefix('"') {
		rest.split('"').next()?
	} else if let Some(rest) = after.strip_prefix('\'') {
		rest.split('\'').next()?
	} else {
		let end = after
			.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
			.unwrap_or(after.len());
		&after[..end]
	};
	if label.is_empty() {
		return None;
	}
	encoding_rs::Encoding::for_label(label.as_bytes())
}

pub fn set_rate_limit(mut env: FunctionEnvMut<WasmEnv>, permits: i32, period: i32, unit: i32) {
	env.data_mut().net.rate.set_rate(permits, period, unit);
}


#[cfg(test)]
mod tests {
	use super::*;
	use crate::runtime::env::{Defaults, NetState};
	use crate::runtime::limits::Limits;
	use crate::runtime::store::GlobalStore;
	use crate::traits::{DirectPolicy, NetPolicy, NetRoute, NoopDefaultsSink, StdoutLogger};
	use std::io::{Read, Write};
	use std::net::TcpListener;
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::sync::{Arc, Mutex};
	use wasmer::{Function, FunctionEnv, Memory, MemoryType, Pages, Store, Value};

	struct EscalationPolicy {
		trawl: Option<String>,
	}

	impl NetPolicy for EscalationPolicy {
		fn route_for(&self, _url: &Url) -> NetRoute {
			NetRoute::Direct
		}

		fn user_agent(&self) -> Option<String> {
			None
		}

		fn trawl_escalation(&self, _url: &Url) -> Option<NetRoute> {
			self.trawl.as_ref().map(|p| NetRoute::Proxy(p.clone()))
		}
	}

	struct MockServer {
		addr: String,
		hits: Arc<AtomicUsize>,
		heads: Arc<Mutex<Vec<String>>>,
	}

	/// 裸 HTTP/1.1 mock：每连接一个请求（响应带 connection: close），
	/// handler 收到请求头文本、回完整响应字节。源头与 trawl 代理共用。
	fn mock_server(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> MockServer {
		let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
		let port = listener.local_addr().expect("local addr").port();
		let hits = Arc::new(AtomicUsize::new(0));
		let heads = Arc::new(Mutex::new(Vec::new()));
		let (hits2, heads2) = (hits.clone(), heads.clone());
		std::thread::spawn(move || {
			for stream in listener.incoming() {
				let Ok(mut stream) = stream else { break };
				let mut head = Vec::new();
				let mut chunk = [0u8; 4096];
				loop {
					match stream.read(&mut chunk) {
						Ok(0) | Err(_) => break,
						Ok(n) => {
							head.extend_from_slice(&chunk[..n]);
							if head.windows(4).any(|w| w == b"\r\n\r\n") {
								break;
							}
						}
					}
				}
				let head = String::from_utf8_lossy(&head).to_string();
				hits2.fetch_add(1, Ordering::SeqCst);
				heads2.lock().expect("heads lock").push(head.clone());
				if stream.write_all(&handler(&head)).is_err() {
					break;
				}
			}
		});
		MockServer {
			addr: format!("127.0.0.1:{port}"),
			hits,
			heads,
		}
	}

	impl MockServer {
		fn hits(&self) -> usize {
			self.hits.load(Ordering::SeqCst)
		}

		fn heads(&self) -> Vec<String> {
			self.heads.lock().expect("heads lock").clone()
		}
	}

	fn respond(status: &str, extra_headers: &[&str], body: &str) -> Vec<u8> {
		respond_bytes(status, extra_headers, body.as_bytes())
	}

	fn respond_bytes(status: &str, extra_headers: &[&str], body: &[u8]) -> Vec<u8> {
		let mut out = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
		for header in extra_headers {
			out.push_str(header);
			out.push_str("\r\n");
		}
		out.push_str("\r\n");
		let mut out = out.into_bytes();
		out.extend_from_slice(body);
		out
	}

	fn get(net: &mut NetState, url: &Url) -> NetResponse {
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url.clone());
		perform(net, &mut request, Duration::from_secs(10)).expect("perform");
		request.response.expect("response stored")
	}

	#[test]
	fn cf_challenge_escalates_via_trawl_and_reuses_clearance() {
		let origin = mock_server(|head| {
			if head.contains("cf_clearance=ok") {
				respond("200 OK", &[], "direct-with-clearance")
			} else {
				respond(
					"403 Forbidden",
					&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
					"challenged",
				)
			}
		});
		let trawl = mock_server(|_head| {
			respond(
				"200 OK",
				&["set-cookie: cf_clearance=ok; Path=/; HttpOnly", "content-type: text/html"],
				"solved-via-trawl",
			)
		});
		let policy = Arc::new(EscalationPolicy {
			trawl: Some(format!("http://{}", trawl.addr)),
		});
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		let first = get(&mut net, &url);
		assert_eq!(first.status, StatusCode::OK);
		assert_eq!(first.data, b"solved-via-trawl");
		assert_eq!(origin.hits(), 1, "直连一次撞挑战即升级");
		assert_eq!(trawl.hits(), 1, "经 trawl 重试一次");
		assert!(
			trawl.heads()[0].starts_with(&format!("GET http://{}/manga/1", origin.addr)),
			"代理收到 absolute-form 请求: {}",
			trawl.heads()[0].lines().next().unwrap_or_default()
		);

		// clearance 已入 jar：后续请求直连带出，不再惊动 trawl
		let second = get(&mut net, &url);
		assert_eq!(second.status, StatusCode::OK);
		assert_eq!(second.data, b"direct-with-clearance");
		assert_eq!(origin.hits(), 2);
		assert_eq!(trawl.hits(), 1);
	}

	#[test]
	fn no_trawl_returns_challenge_as_is() {
		let origin = mock_server(|_head| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::FORBIDDEN);
		assert_eq!(response.data, b"challenged");
		assert_eq!(origin.hits(), 1, "403 不是重试面，不应有退避重发");
	}

	#[test]
	fn non_cf_403_does_not_escalate() {
		let origin = mock_server(|_head| respond("403 Forbidden", &["server: nginx"], "forbidden"));
		let trawl = mock_server(|_head| respond("200 OK", &[], "unexpected"));
		let policy = Arc::new(EscalationPolicy {
			trawl: Some(format!("http://{}", trawl.addr)),
		});
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/private", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::FORBIDDEN);
		assert_eq!(response.data, b"forbidden");
		assert_eq!(trawl.hits(), 0, "源站自己的 403 不该走 trawl");
	}

	#[test]
	fn jar_sink_fires_on_set_and_expiry() {
		// jar 变更必须同步给 sink（持久化的唯一通道）：写入发 Some，真实删除发 None
		struct RecSink(std::sync::Mutex<Vec<(String, String, Option<String>)>>);
		impl CookieSink for RecSink {
			fn set(&self, domain: &str, name: &str, value: Option<&str>) {
				self.0
					.lock()
					.expect("writes lock")
					.push((domain.to_string(), name.to_string(), value.map(str::to_string)));
			}
		}
		let sink = Arc::new(RecSink(std::sync::Mutex::new(Vec::new())));
		let mut jar = CookieJar::new(HashMap::new(), sink.clone());
		let url = Url::parse("https://example.com/x").unwrap();
		jar.store(&url, "sid=abc; Path=/");
		jar.store(&url, "gone=1; Path=/");
		jar.store(&url, "gone=; Max-Age=0");
		jar.store(&url, "never=; Max-Age=0");
		let writes = sink.0.lock().expect("writes lock").clone();
		assert_eq!(
			writes,
			vec![
				("example.com".to_string(), "sid".to_string(), Some("abc".to_string())),
				("example.com".to_string(), "gone".to_string(), Some("1".to_string())),
				("example.com".to_string(), "gone".to_string(), None),
			],
			"不存在的 cookie 删除不该发 None"
		);
	}

	#[test]
	fn jar_insert_fires_sink_for_browser_reflux() {
		// 浏览器回流（insert）与 store 同一条持久化通道，否则登录态重启即丢
		struct RecSink(std::sync::Mutex<Vec<(String, String, Option<String>)>>);
		impl CookieSink for RecSink {
			fn set(&self, domain: &str, name: &str, value: Option<&str>) {
				self.0
					.lock()
					.expect("writes lock")
					.push((domain.to_string(), name.to_string(), value.map(str::to_string)));
			}
		}
		let sink = Arc::new(RecSink(std::sync::Mutex::new(Vec::new())));
		let mut jar = CookieJar::new(HashMap::new(), sink.clone());
		jar.insert("example.com", "sid", "abc");
		jar.insert("", "ignored", "x");
		let writes = sink.0.lock().expect("writes lock").clone();
		assert_eq!(
			writes,
			vec![("example.com".to_string(), "sid".to_string(), Some("abc".to_string()))],
			"空 domain/name 既不落库也不发 sink"
		);
	}

	#[test]
	fn jar_refill_round_trip() {
		// 持久化回填的 jar 按域名（含子域后缀）组 Cookie 头
		let domains = HashMap::from([(
			"example.com".to_string(),
			HashMap::from([("sid".to_string(), "abc".to_string())]),
		)]);
		let jar = CookieJar::new(domains, Arc::new(crate::traits::NoopCookieSink));
		let url = Url::parse("https://m.example.com/x").unwrap();
		assert_eq!(jar.header_for(&url).as_deref(), Some("sid=abc"));
	}

	#[test]
	fn cookie_header_merges_jar_with_source() {
		// jar cookie 与源自带 Cookie 合并，而不是源设了就丢掉 jar（对齐 Aidoku）
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let domains = HashMap::from([(
			"127.0.0.1".to_string(),
			HashMap::from([("jar".to_string(), "1".to_string())]),
		)]);
		let jar = CookieJar::new(domains, Arc::new(crate::traits::NoopCookieSink));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), jar);
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		request.headers.insert(COOKIE, HeaderValue::from_static("src=1"));
		perform(&mut net, &mut request, Duration::from_secs(5)).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("cookie: jar=1; src=1"), "head: {head}");
	}

	#[test]
	fn accept_language_default_is_sent() {
		// 源没设时补默认 Accept-Language（部分 WAF 缺它就 403）
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		perform(&mut net, &mut request, Duration::from_secs(5)).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("accept-language:"), "head: {head}");
	}

	#[test]
	fn decode_body_honors_content_type_charset() {
		let mut headers = HeaderMap::new();
		headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=gbk"));
		let gbk = encoding_rs::GBK.encode("中文").0;
		assert_eq!(decode_body(&headers, &gbk), "中文");
	}

	#[test]
	fn decode_body_honors_meta_charset() {
		let mut body = b"<html><head><meta charset=\"big5\"></head><body>".to_vec();
		body.extend_from_slice(&encoding_rs::BIG5.encode("中").0);
		assert!(decode_body(&HeaderMap::new(), &body).ends_with('中'));
	}

	#[test]
	fn decode_body_defaults_to_utf8() {
		assert_eq!(decode_body(&HeaderMap::new(), "中文".as_bytes()), "中文");
	}

	/// GBK 编码的极简页面：可选 meta 声明 + 一个 p.t 段落
	fn gbk_page(with_meta: bool, text: &str) -> Vec<u8> {
		let mut body = b"<html><head>".to_vec();
		if with_meta {
			body.extend_from_slice(b"<meta charset=\"gbk\">");
		}
		body.extend_from_slice(b"</head><body><p class=\"t\">");
		body.extend_from_slice(&encoding_rs::GBK.encode(text).0);
		body.extend_from_slice(b"</p></body></html>");
		body
	}

	fn gzip_body(data: &[u8]) -> Vec<u8> {
		let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
		enc.write_all(data).expect("gzip");
		enc.finish().expect("gzip finish")
	}

	/// 线格式是 zlib 包装：tower-http 的 decompression 把 deflate 映射到
	/// ZlibDecoder，裸 DeflateEncoder 的流解不开
	fn deflate_body(data: &[u8]) -> Vec<u8> {
		let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
		enc.write_all(data).expect("deflate");
		enc.finish().expect("deflate finish")
	}

	fn brotli_body(data: &[u8]) -> Vec<u8> {
		let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
		enc.write_all(data).expect("brotli");
		enc.into_inner()
	}

	/// 最小 wasmer 宿主：直接驱动 net FFI（init/set_url/send/html），
	/// 再借 imports::html 的 FFI 验证选择器命中，不需要真 wasm guest
	struct Ffi {
		store: Store,
		env: FunctionEnv<WasmEnv>,
		memory: Memory,
		bump: u32,
	}

	impl Ffi {
		fn new() -> Self {
			let mut store = Store::default();
			let memory = Memory::new(&mut store, MemoryType::new(Pages(4), None, false)).unwrap();
			let env = FunctionEnv::new(
				&mut store,
				WasmEnv {
					memory: Some(memory.clone()),
					store: GlobalStore::new(),
					defaults: Defaults::new(HashMap::new(), Arc::new(NoopDefaultsSink), "test".into()),
					net: NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default()),
					browser: crate::browser::BrowserState::new(None, Duration::from_secs(1)),
					limits: Limits::default(),
					source_key: "test".into(),
					logger: Arc::new(StdoutLogger),
					partial_sink: None,
				},
			);
			// 指针 0 起是合法内存，测试数据从 1024 起 bump 写入避免踩到空指针路径
			Self { store, env, memory, bump: 1024 }
		}

		fn write_str(&mut self, s: &str) -> (i32, i32) {
			let ptr = self.bump;
			self.bump += s.len() as u32 + 64;
			self.memory.view(&self.store).write(ptr as u64, s.as_bytes()).unwrap();
			(ptr as i32, s.len() as i32)
		}

		fn call1(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Rid) -> FFIResult, a: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		fn call3(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Rid, Ptr, u32) -> FFIResult, a: i32, b: i32, c: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a), Value::I32(b), Value::I32(c)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		/// init(GET) → set_url → send → html，返回文档 rid；任一步失败即断言
		fn fetch_html(&mut self, url: &str) -> Rid {
			let rid = Function::new_typed_with_env(&mut self.store, &self.env, init)
				.call(&mut self.store, &[Value::I32(0)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap();
			assert!(rid > 0, "init 返回 {rid}");
			let (p, l) = self.write_str(url);
			assert_eq!(self.call3(set_url, rid, p, l), 0, "set_url");
			assert_eq!(self.call1(send, rid), 0, "send");
			let doc = self.call1(html, rid);
			assert!(doc > 0, "html 返回 {doc}");
			doc
		}

		fn select_text(&mut self, doc: Rid, query: &str) -> String {
			let (p, l) = self.write_str(query);
			let el = self.call3(crate::imports::html::select_first, doc, p, l);
			assert!(el > 0, "select_first({query}) 返回 {el}");
			let s = self.call1(crate::imports::html::text, el);
			assert!(s > 0, "text 返回 {s}");
			let Some(StoreItem::String(text)) = self.env.as_ref(&self.store).store.get(s) else {
				panic!("rid {s} 不是 StoreItem::String");
			};
			text.clone()
		}
	}

	#[test]
	fn html_decodes_gbk_via_content_type() {
		// content-type 声明 gbk：html() 按 charset 解码成功，选择器命中中文
		let page = gbk_page(false, "中文标题");
		let server = mock_server(move |_head| {
			respond_bytes("200 OK", &["content-type: text/html; charset=gbk"], &page)
		});
		let mut ffi = Ffi::new();
		let doc = ffi.fetch_html(&format!("http://{}/", server.addr));
		assert_eq!(ffi.select_text(doc, "p.t"), "中文标题");
	}

	#[test]
	fn html_decodes_gbk_via_meta_charset() {
		// 头里没有 charset 时嗅探文档内的 <meta charset>
		let page = gbk_page(true, "中文标题");
		let server = mock_server(move |_head| respond_bytes("200 OK", &["content-type: text/html"], &page));
		let mut ffi = Ffi::new();
		let doc = ffi.fetch_html(&format!("http://{}/", server.addr));
		assert_eq!(ffi.select_text(doc, "p.t"), "中文标题");
	}

	#[test]
	fn html_replaces_invalid_utf8_without_markers() {
		// 无 BOM/声明的非法 UTF-8 走 replacement 解码，html() 不返回 InvalidString
		let body = b"<html><body><p class=\"t\">caf\xE9\x80</p></body></html>".to_vec();
		let server = mock_server(move |_head| respond_bytes("200 OK", &["content-type: text/html"], &body));
		let mut ffi = Ffi::new();
		let doc = ffi.fetch_html(&format!("http://{}/", server.addr));
		let text = ffi.select_text(doc, "p.t");
		assert!(text.starts_with("caf"), "text: {text:?}");
		assert!(text.contains('\u{FFFD}'), "text: {text:?}");
	}

	/// 源自己覆写 Accept-Encoding 的请求（issue 里 br/deflate 失守的场景）
	fn get_with_accept_encoding(server: &MockServer, accept_encoding: &str) -> NetResponse {
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		request.headers.insert(
			reqwest::header::ACCEPT_ENCODING,
			HeaderValue::from_str(accept_encoding).expect("accept-encoding"),
		);
		perform(&mut net, &mut request, Duration::from_secs(5)).expect("perform");
		request.response.expect("response stored")
	}

	#[test]
	fn gzip_body_is_decoded() {
		// 默认路径不回归：声明并接受 gzip
		let server = mock_server(|_head| respond_bytes("200 OK", &["content-encoding: gzip"], &gzip_body(b"gzip-body")));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		assert_eq!(get(&mut net, &url).data, b"gzip-body");
	}

	#[test]
	fn brotli_body_is_decoded() {
		let server = mock_server(|_head| respond_bytes("200 OK", &["content-encoding: br"], &brotli_body(b"br-body")));
		assert_eq!(get_with_accept_encoding(&server, "gzip, deflate, br").data, b"br-body");
	}

	#[test]
	fn deflate_body_is_decoded() {
		let server =
			mock_server(|_head| respond_bytes("200 OK", &["content-encoding: deflate"], &deflate_body(b"deflate-body")));
		assert_eq!(get_with_accept_encoding(&server, "gzip, deflate, br").data, b"deflate-body");
	}

	#[test]
	fn accept_encoding_defaults_to_gzip_br_deflate() {
		// 对齐 URLSession 的协商集：源不设时三种编码都声明（补头只在空位发生）
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		get(&mut net, &url);
		let head = server.heads()[0].to_ascii_lowercase();
		let line = head
			.lines()
			.find(|l| l.starts_with("accept-encoding:"))
			.expect("accept-encoding header");
		let tokens: Vec<&str> = line["accept-encoding:".len()..].split(',').map(str::trim).collect();
		for enc in ["gzip", "br", "deflate"] {
			assert!(tokens.contains(&enc), "accept-encoding: {line}");
		}
	}

	#[test]
	fn cookie_header_uses_jar_when_source_silent() {
		// 源没设 Cookie 时行为不变：只带 jar 值
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let domains = HashMap::from([(
			"127.0.0.1".to_string(),
			HashMap::from([("jar".to_string(), "1".to_string())]),
		)]);
		let jar = CookieJar::new(domains, Arc::new(crate::traits::NoopCookieSink));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), jar);
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		perform(&mut net, &mut request, Duration::from_secs(5)).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("cookie: jar=1"), "head: {head}");
	}
}
