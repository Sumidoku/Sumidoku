//! net 族的请求生命周期与 `perform`：生产实现的直连精简版（无 policy/jar/CF 升级——
//! 那些是引擎无关的装饰，移植是整文件搬移）。保留 Limits 语义：超时钳帽、响应体上限。

use std::io::Read;
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl From<u8> for HttpMethod {
	fn from(value: u8) -> Self {
		match value {
			1 => Self::Post,
			2 => Self::Put,
			3 => Self::Head,
			4 => Self::Delete,
			5 => Self::Patch,
			6 => Self::Options,
			7 => Self::Connect,
			8 => Self::Trace,
			_ => Self::Get,
		}
	}
}

impl From<HttpMethod> for reqwest::Method {
	fn from(method: HttpMethod) -> Self {
		match method {
			HttpMethod::Get => reqwest::Method::GET,
			HttpMethod::Post => reqwest::Method::POST,
			HttpMethod::Put => reqwest::Method::PUT,
			HttpMethod::Head => reqwest::Method::HEAD,
			HttpMethod::Delete => reqwest::Method::DELETE,
			HttpMethod::Patch => reqwest::Method::PATCH,
			HttpMethod::Options => reqwest::Method::OPTIONS,
			HttpMethod::Connect => reqwest::Method::CONNECT,
			HttpMethod::Trace => reqwest::Method::TRACE,
		}
	}
}

#[derive(Debug)]
pub struct NetResponse {
	pub url: String,
	pub status: u16,
	pub headers: Vec<(String, String)>,
	pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct NetRequest {
	pub method: HttpMethod,
	pub url: Option<Url>,
	pub headers: Vec<(String, String)>,
	pub body: Option<Vec<u8>>,
	pub timeout: Option<f64>,
	pub response: Option<NetResponse>,
}

impl NetRequest {
	pub fn new(method: HttpMethod) -> Self {
		Self {
			method,
			url: None,
			headers: Vec::new(),
			body: None,
			timeout: None,
			response: None,
		}
	}
}

#[derive(Debug)]
pub enum SendFailure {
	InvalidUrl,
	Request,
	BodyTooLarge,
}

/// 有界读响应体：与生产同一防线（头声明超线快败、流式累计截停），max 为 0 不设限
fn read_body_capped(mut response: reqwest::blocking::Response, max: u64) -> Result<Vec<u8>, SendFailure> {
	if max > 0
		&& let Some(len) = response.content_length()
		&& len > max
	{
		return Err(SendFailure::BodyTooLarge);
	}
	let mut out = Vec::new();
	let mut buf = [0u8; 64 * 1024];
	loop {
		match response.read(&mut buf) {
			Ok(0) => break,
			Ok(n) => {
				if max > 0 && out.len() as u64 + n as u64 > max {
					return Err(SendFailure::BodyTooLarge);
				}
				out.extend_from_slice(&buf[..n]);
			}
			Err(_) => return Err(SendFailure::Request),
		}
	}
	Ok(out)
}

pub fn perform(
	client: &reqwest::blocking::Client,
	request: &mut NetRequest,
	timeout: Duration,
	body_max: u64,
) -> Result<(), SendFailure> {
	let Some(url) = request.url.clone() else {
		return Err(SendFailure::InvalidUrl);
	};
	let mut builder = client.request(request.method.into(), url.as_str()).timeout(timeout);
	for (name, value) in &request.headers {
		builder = builder.header(name, value);
	}
	if let Some(body) = request.body.clone() {
		builder = builder.body(body);
	}
	let Ok(response) = builder.send() else {
		return Err(SendFailure::Request);
	};
	let status = response.status().as_u16();
	let final_url = response.url().to_string();
	let headers: Vec<(String, String)> = response
		.headers()
		.iter()
		.map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
		.collect();
	let data = read_body_capped(response, body_max)?;
	request.response = Some(NetResponse {
		url: final_url,
		status,
		headers,
		data,
	});
	Ok(())
}
