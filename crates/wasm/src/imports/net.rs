use crate::{
	WasmEnv,
	store::{FFIResult, ImageData, Ptr, Rid, StoreItem},
};
use image::ImageReader;
use reqwest::{
	StatusCode,
	header::{COOKIE, HeaderMap, HeaderName, HeaderValue, SET_COOKIE, USER_AGENT},
};
use std::{collections::HashMap, io::Cursor, str::FromStr, time::Duration};
use url::Url;
use wasmer::FunctionEnvMut;

const FALLBACK_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
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

#[derive(Debug, Default)]
pub struct CookieJar {
	domains: HashMap<String, HashMap<String, String>>,
}

impl CookieJar {
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
				"max-age" => {
					if v.trim().parse::<i64>().map(|s| s <= 0).unwrap_or(false) {
						expired = true;
					}
				}
				_ => {}
			}
		}
		let entry = self.domains.entry(domain).or_default();
		if expired {
			entry.remove(name);
		} else {
			entry.insert(name.to_string(), value.to_string());
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

fn common_send(env: &mut FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let url = {
		let Some(request) = env.data_mut().store.get_mut(rid).and_then(|item| item.as_request()) else {
			return Result::InvalidDescriptor.into();
		};
		let Some(url) = request.url.clone() else {
			return Result::InvalidUrl.into();
		};
		url
	};
	let route = env.data().net.policy.route_for(&url);
	if env.data_mut().net.client_for(&route).is_err() {
		return Result::RequestError.into();
	}

	env.data_mut().net.rate.acquire();

	let max_attempts = MAX_ATTEMPTS;
	let mut attempt = 0;
	loop {
		attempt += 1;
		let outcome = {
			let data = env.data_mut();
			let Some(request) = data.store.get_mut(rid).and_then(|item| item.as_request()) else {
				return Result::InvalidDescriptor.into();
			};
			let Some(url) = request.url.as_ref() else {
				return Result::InvalidUrl.into();
			};
			if !request.headers.contains_key(USER_AGENT) {
				let ua = data
					.net
					.policy
					.user_agent()
					.unwrap_or_else(|| FALLBACK_USER_AGENT.to_string());
				if let Ok(value) = HeaderValue::from_str(&ua) {
					request.headers.insert(USER_AGENT, value);
				}
			}
			if let Some(jar_header) = data.net.jar.header_for(url) {
				if let Ok(value) = HeaderValue::from_str(&jar_header) {
					request.headers.entry(COOKIE).or_insert(value);
				}
			}
			let timeout = request
				.timeout
				.map(|t| Duration::from_secs_f64(t.max(0.0)))
				.map(|t| t.min(data.limits.net_timeout_max))
				.unwrap_or(data.limits.net_timeout_default);

			let client = data.net.clients.get(&route).expect("client built above");
			let mut builder = client
				.request(request.method.into(), url.to_string())
				.headers(request.headers.clone())
				.timeout(timeout);
			if let Some(body) = request.body.take() {
				builder = builder.body(body);
			}
			builder.send()
		};

		let Ok(response) = outcome else {
			return Result::RequestError.into();
		};
		let status = response.status();
		let retry_after = response
			.headers()
			.get(reqwest::header::RETRY_AFTER)
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse::<u64>().ok());
		if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && attempt < max_attempts {
			let wait = retry_after
				.map(Duration::from_secs)
				.unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1)));
			std::thread::sleep(wait);
			continue;
		}

		let url = response.url().clone();
		let headers = response.headers().clone();
		let data = response.bytes();
		let Ok(bytes) = data else {
			return Result::RequestError.into();
		};

		let env_data = env.data_mut();
		for value in headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()) {
			env_data.net.jar.store(&url, value);
		}
		let Some(request) = env_data.store.get_mut(rid).and_then(|item| item.as_request()) else {
			return Result::InvalidDescriptor.into();
		};
		request.response = Some(NetResponse {
			url,
			status,
			headers,
			data: bytes.into(),
		});
		return Result::Success.into();
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
	let Some(rgb) = ImageReader::new(cursor)
		.with_guessed_format()
		.ok()
		.and_then(|r| r.decode().ok())
		.map(|img| img.to_rgb8())
	else {
		return Result::NotAnImage.into();
	};
	let image = ImageData {
		width: rgb.width() as i32,
		height: rgb.height() as i32,
		data: rgb.into_raw(),
	};
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
	let parsed = match with_response(&mut env, rid, Result::MissingResponse, |r| {
		std::str::from_utf8(&r.data)
			.ok()
			.map(|text| (text.to_string(), r.url.as_str().to_string()))
	}) {
		Ok(Some(p)) => p,
		Ok(None) => return Result::InvalidString.into(),
		Err(e) => return e,
	};
	let document = crate::html_dom::HtmlDocument::parse(&parsed.0, Some(&parsed.1));
	env.data_mut().store.store(StoreItem::HtmlDocument(document))
}

pub fn set_rate_limit(mut env: FunctionEnvMut<WasmEnv>, permits: i32, period: i32, unit: i32) {
	env.data_mut().net.rate.set_rate(permits, period, unit);
}
