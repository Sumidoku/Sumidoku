use crate::{
	runtime::{
		env::Ctx,
		store::{FFIResult, ImageData, Ptr, Rid, StoreItem},
	},
	traits::NetRoute,
};
use image::ImageReader;
use reqwest::{
	StatusCode,
	header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, SET_COOKIE, USER_AGENT},
};
use crate::traits::CookieSink;
use std::{collections::HashMap, io::{Cursor, Read}, str::FromStr, sync::Arc, time::Duration};
use url::Url;

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

	/// Cookies flowing back from the browser land in the jar directly by their Domain
	/// attribute (there is no raw Set-Cookie to parse); persisted via the sink like store,
	/// otherwise the login state lives only in memory and is lost on restart
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

// wasmi host params have no u8, so the 0..=8 check on method lives in the function body
pub fn init(ctx: &mut impl Ctx, method: i32) -> FFIResult {
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
	ctx.host()
		.store
		.store(StoreItem::Request(Box::new(NetRequest::new(method))))
}

/// The failure surface of perform, mapped to send's FFI error codes
#[derive(Debug)]
pub(crate) enum SendFailure {
	InvalidUrl,
	Request,
	BodyTooLarge,
}

/// Send a request routed per policy: back off and retry on 429/5xx, escalate once via
/// trawl when a direct connection hits a CF challenge, resend once without the injected
/// browser header suite when DRF answers its browsable-API page, and put response
/// Set-Cookie into the jar.
/// Any HTTP response (including non-2xx) is written back to request.response; only
/// network-layer failures are errors.
pub(crate) fn perform(
	net: &mut crate::runtime::env::NetState,
	request: &mut NetRequest,
	timeout: Duration,
	body_max: u64,
) -> std::result::Result<(), SendFailure> {
	let Some(url) = request.url.clone() else {
		return Err(SendFailure::InvalidUrl);
	};
	let mut route = net.policy.route_for(&url);
	if net.client_for(&route).is_err() {
		return Err(SendFailure::Request);
	}

	net.rate.acquire();

	// A login sequence runs inside a trawl browser session: the session accumulates the
	// site's cookies across calls (warm-up GETs then the POST), which plain direct/proxy
	// hops cannot. The first login request tries direct; on a CF challenge the host
	// escalates to its login session (see the challenge branch below) and the rest of the
	// sequence rides it. Only basic_login() sets this, so ordinary requests never do.
	let host = url.host_str().unwrap_or_default().to_string();
	if net.login_in_progress()
		&& net.trawl_api().is_some()
		&& let Some(sid) = net.login_session(&host)
	{
		// Already escalated in this sequence: everything left goes through the session
		return net.session_scrape(&url, request, &sid, timeout, body_max);
	}
	let mut replay = net.policy.clearance_replay(&host);
	if let Some(replay) = &replay {
		// The clearance is bound to the solving browser's UA, which wins outright here —
		// replaying under any other UA just re-challenges
		if let Ok(value) = HeaderValue::from_str(&replay.user_agent) {
			request.headers.insert(USER_AGENT, value);
		}
	} else if !request.headers.contains_key(USER_AGENT) {
		let ua = net
			.policy
			.user_agent()
			.unwrap_or_else(|| FALLBACK_USER_AGENT.to_string());
		if let Ok(value) = HeaderValue::from_str(&ua) {
			request.headers.insert(USER_AGENT, value);
		}
	}
	if !request.headers.contains_key(ACCEPT_LANGUAGE)
		&& !net.accept_language.is_empty()
		&& let Ok(value) = HeaderValue::from_str(&net.accept_language)
	{
		request.headers.insert(ACCEPT_LANGUAGE, value);
	}
	// The injected subset is remembered by name so a DRF browsable-API answer can be retried
	// without exactly these — a same-named header the source declared was never inserted
	// here and must survive the strip
	let mut injected_suite: Vec<HeaderName> = Vec::new();
	if net.policy.browser_headers(&url) {
		for &(name, value) in crate::traits::BROWSER_HEADERS {
			// A clearance replay speaks as the solver's Firefox: no sec-ch-ua (Firefox has
			// no UA client hints) and Firefox's navigation Accept (no signed-exchange)
			if replay.is_some() && crate::traits::CHROME_ONLY_HEADERS.contains(&name) {
				continue;
			}
			let value = if replay.is_some() && name == "accept" {
				crate::traits::FIREFOX_DOCUMENT_ACCEPT
			} else {
				value
			};
			if !request.headers.contains_key(name) {
				let name = HeaderName::from_static(name);
				request.headers.insert(name.clone(), HeaderValue::from_static(value));
				injected_suite.push(name);
			}
		}
	}
	// Aligned with Aidoku: jar cookies are spliced ahead of the source's own Cookie header, not dropped when the source sets one
	if let Some(jar_header) = net.jar.header_for(&url) {
		let merged = match request.headers.get(COOKIE).and_then(|v| v.to_str().ok()) {
			Some(existing) if !existing.is_empty() => format!("{jar_header}; {existing}"),
			_ => jar_header,
		};
		if let Ok(value) = HeaderValue::from_str(&merged) {
			request.headers.insert(COOKIE, value);
		}
	}
	if let Some(replay) = &replay {
		splice_clearance(&mut request.headers, &replay.clearance);
	}

	let mut attempt = 0;
	let mut escalated = false;
	let mut suite_stripped = false;
	let mut solved = false;
	loop {
		attempt += 1;
		let client = net.clients.get(&route).expect("client built above");
		let mut builder = client
			.request(request.method.into(), url.as_str())
			.headers(request.headers.clone())
			.timeout(timeout);
		// Retries must carry the body too: take would make the second attempt (including the trawl escalation) lose the POST payload
		if let Some(body) = request.body.clone() {
			builder = builder.body(body);
		}
		let Ok(response) = builder.send() else {
			return Err(SendFailure::Request);
		};
		let status = response.status();
		if route == NetRoute::Direct && crate::traits::is_cf_challenge(status, response.headers()) {
			// A cached clearance that still meets a challenge is stale: drop it before deciding
			if replay.take().is_some() {
				net.policy.clear_clearance(&host);
			}
			// Login flow on a challenged host: escalate straight to the browser session.
			// Solving + replaying direct cannot carry the login form's sequence (warm-up
			// cookies, POST body), so the whole login rides a dedicated session.
			if net.login_in_progress() && net.trawl_api().is_some() {
				let sid = crate::browser::trawl::login_session_id(&host);
				if net.ensure_login_session(&sid) {
					net.set_login_session(host.clone(), sid.clone());
					return net.session_scrape(&url, request, &sid, timeout, body_max);
				}
				// Session creation failed (pool initializing / saturated): fall through to
				// the normal solve → proxy path
			}
			// Solve once per request and replay direct: the proxy returns browser-serialized
			// DOM on its browser tiers, so it stays the fallback for when solving fails
			if !solved {
				solved = true;
				if let Some(fresh) = net.policy.solve_clearance(&url) {
					apply_clearance_replay(&mut request.headers, &fresh, &injected_suite);
					replay = Some(fresh);
					attempt = 0;
					continue;
				}
			}
			// CF escalation comes before backoff: a 503 challenge page must not burn the direct connection's backoff attempts for nothing
			if !escalated
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
		let bytes = match read_body_capped(response, body_max) {
			Ok(bytes) => bytes,
			Err(BodyRead::TooLarge) => return Err(SendFailure::BodyTooLarge),
			Err(BodyRead::Request) => return Err(SendFailure::Request),
		};

		for value in headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()) {
			net.jar.store(&url, value);
		}
		// DRF negotiates on Accept and answers the suite's document-class Accept with its
		// browsable-API HTML shell; AidokuRunner injects nothing and gets JSON. Resend once
		// without the injected headers — a second DRF page is returned as is
		if !suite_stripped
			&& route == NetRoute::Direct
			&& !injected_suite.is_empty()
			&& crate::traits::is_drf_browsable_page(status, &headers, &bytes)
		{
			for name in &injected_suite {
				request.headers.remove(name);
			}
			suite_stripped = true;
			attempt = 0;
			continue;
		}
		request.response = Some(NetResponse {
			url,
			status,
			headers,
			data: bytes,
		});
		return Ok(());
	}
}

impl crate::runtime::env::NetState {
	/// Create the host's login session idempotently (201 created / 409 already alive).
	fn ensure_login_session(&self, sid: &str) -> bool {
		let Some(api) = self.trawl_api() else {
			return false;
		};
		let Ok(client) = reqwest::blocking::Client::builder().use_rustls_tls().build() else {
			return false;
		};
		crate::browser::trawl::ensure_session(api, sid, &client)
	}

	/// Run a request inside a trawl browser session: warm-up GETs and the POST login share
	/// one session so the site's cookies accumulate in the browser context; the returned
	/// cookies are mirrored into our jar for later non-browser requests. The response body
	/// and status are written back onto the request like any other hop.
	fn session_scrape(
		&mut self,
		url: &Url,
		request: &mut NetRequest,
		sid: &str,
		timeout: Duration,
		body_max: u64,
	) -> std::result::Result<(), SendFailure> {
		let Some(api) = self.trawl_api().map(str::to_string) else {
			return Err(SendFailure::Request);
		};
		let method = match request.method {
			HttpMethod::Get => "GET",
			HttpMethod::Post => "POST",
			HttpMethod::Put => "PUT",
			HttpMethod::Head => "HEAD",
			HttpMethod::Delete => "DELETE",
			HttpMethod::Patch => "PATCH",
			HttpMethod::Options => "OPTIONS",
			HttpMethod::Connect => "CONNECT",
			HttpMethod::Trace => "TRACE",
		};
		let headers: Vec<(String, String)> = request
			.headers
			.iter()
			.map(|(name, value)| {
				(
					name.as_str().to_string(),
					value.to_str().map(str::to_string).unwrap_or_default(),
				)
			})
			.collect();
		let outcome = crate::browser::trawl::session_request(
			&api,
			sid,
			method,
			url.as_str(),
			&headers,
			request.body.as_deref(),
			timeout,
		)
		.ok_or(SendFailure::Request)?;
		let status = reqwest::StatusCode::from_u16(outcome.status)
			.unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
		// Mirror the session's site cookies into the jar (login state): later
		// non-browser requests carry them without a browser. cf_clearance is deliberately
		// excluded — it is bound to the session browser's own fingerprint, so on any
		// non-solver request (MITM fast-path sends our UA, not the browser's) it is
		// refused by CF, and two same-named clearances confuse the WAF. Clearance is
		// managed exclusively by the solve path (splice_clearance replaces it).
		for cookie in outcome.cookies {
			if cookie.domain.is_empty()
				|| cookie.name.is_empty()
				|| cookie.name == crate::traits::CF_CLEARANCE_COOKIE
			{
				continue;
			}
			let domain = cookie.domain.trim_start_matches('.').to_string();
			self.jar.insert(&domain, &cookie.name, &cookie.value);
		}
		if outcome.body.len() as u64 > body_max {
			return Err(SendFailure::BodyTooLarge);
		}
		request.response = Some(NetResponse {
			url: url.clone(),
			status,
			headers: HeaderMap::new(),
			data: outcome.body,
		});
		Ok(())
	}
}

enum BodyRead {
	TooLarge,
	Request,
}

/// Replay a freshly solved clearance on already-assembled headers: the solver's UA
/// replaces whatever is there (the cookie is bound to it), the injected Chrome-only
/// headers come off, an injected Accept becomes Firefox's navigation value, and
/// cf_clearance leads the Cookie header. Source-declared headers are never in
/// `injected_suite`, so they survive untouched
fn apply_clearance_replay(
	headers: &mut HeaderMap,
	replay: &crate::traits::ClearanceReplay,
	injected_suite: &[HeaderName],
) {
	if let Ok(value) = HeaderValue::from_str(&replay.user_agent) {
		headers.insert(USER_AGENT, value);
	}
	for &name in crate::traits::CHROME_ONLY_HEADERS {
		let name = HeaderName::from_static(name);
		if injected_suite.contains(&name) {
			headers.remove(&name);
		}
	}
	if injected_suite.contains(&ACCEPT) {
		headers.insert(
			ACCEPT,
			HeaderValue::from_static(crate::traits::FIREFOX_DOCUMENT_ACCEPT),
		);
	}
	splice_clearance(headers, &replay.clearance);
}

/// cf_clearance leads the Cookie header; any older value under the same name is dropped
/// (a stale clearance re-challenges, and two same-named cookies confuse the WAF)
fn splice_clearance(headers: &mut HeaderMap, clearance: &str) {
	if clearance.is_empty() {
		return;
	}
	let existing = headers
		.get(COOKIE)
		.and_then(|v| v.to_str().ok())
		.unwrap_or_default();
	let mut merged = format!("{}={clearance}", crate::traits::CF_CLEARANCE_COOKIE);
	for part in existing
		.split(';')
		.map(str::trim)
		.filter(|part| !part.is_empty() && !part.starts_with("cf_clearance="))
	{
		merged.push_str("; ");
		merged.push_str(part);
	}
	if let Ok(value) = HeaderValue::from_str(&merged) {
		headers.insert(COOKIE, value);
	}
}

/// Bounded response-body read: when the header-declared length exceeds the cap, fail fast;
/// for compressed responses the header length is the compressed size (or even absent) and
/// what lands in memory is decoded bytes, so cutting off by streaming accumulation is the
/// real defense. max = 0 means unlimited
fn read_body_capped(mut response: reqwest::blocking::Response, max: u64) -> std::result::Result<Vec<u8>, BodyRead> {
	if max > 0
		&& let Some(len) = response.content_length()
		&& len > max
	{
		return Err(BodyRead::TooLarge);
	}
	let mut out = Vec::new();
	let mut buf = [0u8; 64 * 1024];
	loop {
		match response.read(&mut buf) {
			Ok(0) => break,
			Ok(n) => {
				if max > 0 && out.len() as u64 + n as u64 > max {
					return Err(BodyRead::TooLarge);
				}
				out.extend_from_slice(&buf[..n]);
			}
			Err(_) => return Err(BodyRead::Request),
		}
	}
	Ok(out)
}

fn common_send(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let data = ctx.host();
	let Some(request) = data.store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	let timeout = request
		.timeout
		.map(|t| Duration::from_secs_f64(t.max(0.0)))
		.map(|t| t.min(data.limits.net_timeout_max))
		.unwrap_or(data.limits.net_timeout_default);
	let result = perform(&mut data.net, request, timeout, data.limits.net_body_max);
	if let Err(SendFailure::BodyTooLarge) = &result {
		data.log(&format!(
			"response body exceeds the {} byte net_body_max limit",
			data.limits.net_body_max
		));
	}
	match result {
		Ok(()) => Result::Success.into(),
		Err(SendFailure::InvalidUrl) => Result::InvalidUrl.into(),
		// The ABI has no dedicated "body over cap" code, so the source only gets RequestError; the cap and the reason go into the source log above
		Err(SendFailure::Request | SendFailure::BodyTooLarge) => Result::RequestError.into(),
	}
}

pub fn send(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	common_send(ctx, rid)
}

pub fn send_all(ctx: &mut impl Ctx, rid_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(rids) = ctx.read_i32s(rid_ptr, len) else {
		return Result::InvalidDescriptor.into();
	};
	let mut errors = Vec::new();
	let mut was_error = false;
	for rid in rids {
		let result = common_send(ctx, rid);
		if result != Into::<i32>::into(Result::Success) {
			was_error = true;
		}
		errors.push(result);
	}
	if ctx.write_i32s(rid_ptr, &errors).is_err() {
		Result::FailedMemoryWrite.into()
	} else if was_error {
		Result::RequestError.into()
	} else {
		Result::Success.into()
	}
}

pub fn set_url(ctx: &mut impl Ctx, rid: Rid, ptr: Ptr, len: u32) -> FFIResult {
	let Ok(string) = ctx.read_string(ptr, len) else {
		return Result::InvalidString.into();
	};
	let Ok(url) = Url::parse(&string) else {
		return Result::InvalidUrl.into();
	};
	let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.url = Some(url);
	Result::Success.into()
}

pub fn set_header(
	ctx: &mut impl Ctx,
	rid: Rid,
	key_ptr: Ptr,
	key_len: u32,
	val_ptr: Ptr,
	val_len: u32,
) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(val) = ctx.read_string(val_ptr, val_len) else {
		return Result::InvalidString.into();
	};
	let Ok(name) = HeaderName::from_str(&key) else {
		return Result::InvalidString.into();
	};
	let Ok(value) = HeaderValue::from_str(&val) else {
		return Result::InvalidString.into();
	};
	let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.headers.insert(name, value);
	Result::Success.into()
}

pub fn set_body(ctx: &mut impl Ctx, rid: Rid, ptr: Ptr, len: u32) -> FFIResult {
	let Ok(body) = ctx.read_bytes(ptr, len) else {
		return Result::InvalidString.into();
	};
	let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.body = Some(body);
	Result::Success.into()
}

pub fn set_timeout(ctx: &mut impl Ctx, rid: Rid, value: f64) -> FFIResult {
	let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Result::InvalidDescriptor.into();
	};
	request.timeout = Some(value);
	Result::Success.into()
}

fn with_response<T>(
	ctx: &mut impl Ctx,
	rid: Rid,
	missing: Result,
	f: impl FnOnce(&NetResponse) -> T,
) -> std::result::Result<T, i32> {
	let Some(request) = ctx.host().store.get_mut(rid).and_then(|item| item.as_request()) else {
		return Err(Result::InvalidDescriptor.into());
	};
	let Some(response) = request.response.as_ref() else {
		return Err(missing.into());
	};
	Ok(f(response))
}

pub fn data_len(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	match with_response(ctx, rid, Result::MissingResponse, |r| r.data.len() as i32) {
		Ok(v) => v,
		Err(e) => e,
	}
}

pub fn read_data(ctx: &mut impl Ctx, rid: Rid, buffer: Ptr, size: u32) -> FFIResult {
	let data = match with_response(ctx, rid, Result::MissingResponse, |r| r.data.clone()) {
		Ok(d) => d,
		Err(e) => return e,
	};
	if size as usize > data.len() {
		return Result::FailedMemoryWrite.into();
	}
	if ctx.write_buffer(buffer, &data[..size as usize]).is_err() {
		Result::FailedMemoryWrite.into()
	} else {
		Result::Success.into()
	}
}

pub fn get_image(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let data = match with_response(ctx, rid, Result::MissingResponse, |r| r.data.clone()) {
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
	ctx.host().store.store(StoreItem::ImageData(image))
}

pub fn get_status_code(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	match with_response(ctx, rid, Result::MissingResponse, |r| r.status.as_u16() as i32) {
		Ok(v) => v,
		Err(e) => e,
	}
}

pub fn get_url(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let url = match with_response(ctx, rid, Result::MissingResponse, |r| r.url.as_str().to_string()) {
		Ok(u) => u,
		Err(e) => return e,
	};
	ctx.host().store.store(StoreItem::String(url))
}

pub fn get_header(ctx: &mut impl Ctx, rid: Rid, key_ptr: Ptr, key_len: u32) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(name) = HeaderName::from_str(&key) else {
		return Result::InvalidString.into();
	};
	let value = match with_response(ctx, rid, Result::MissingResponse, |r| {
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
	ctx.host().store.store(StoreItem::String(value))
}

pub fn html(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let (text, url) = match with_response(ctx, rid, Result::MissingResponse, |r| {
		(decode_body(&r.headers, &r.data), r.url.as_str().to_string())
	}) {
		Ok(p) => p,
		Err(e) => return e,
	};
	let document = crate::html_dom::HtmlDocument::parse(&text, Some(&url));
	ctx.host().store.store(StoreItem::HtmlDocument(document))
}

/// Decode by the response body's charset: BOM → declared UTF-16 → strict UTF-8
/// validation → Content-Type charset → `<meta charset>` sniff → UTF-8 lossy. If decoding
/// produced replacement characters and the document declares another charset, re-decode
/// the original bytes once.
///
/// The strict UTF-8 check outranks a declared single-byte charset on purpose: servers
/// frequently serve UTF-8 pages with a lying label such as `iso-8859-1`, and decoding
/// UTF-8 bytes as windows-1252 produces classic mojibake (e.g. "第1話" -> "ç¬¬1è©±").
/// A full-document UTF-8 validation is a much stronger signal than the header, and the
/// Aidoku host decodes bodies as UTF-8 outright (WasmNet.swift). Genuinely non-UTF-8
/// pages (GBK/Big5) fail the validation and follow the declared/sniffed charset as
/// before.
fn decode_body(headers: &HeaderMap, data: &[u8]) -> String {
	let encoding = detect_encoding(headers, data);
	let (text, _, _) = encoding.decode(data);
	let text = text.into_owned();
	if !text.contains('\u{FFFD}') {
		return text;
	}
	// Decoding replaced bytes: the document may declare its real charset past the sniff
	// window. ASCII-compatible encodings keep the <meta> tag itself readable, so the
	// declaration can be recovered from the decoded text; re-decode the raw bytes once.
	let head = &text[..text.len().min(8192)];
	if let Some(enc) = charset_label_in(head).filter(|enc| *enc != encoding) {
		let (retry, _, _) = enc.decode(data);
		return retry.into_owned();
	}
	text
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

	let declared = headers
		.get(CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.and_then(charset_from_content_type)
		.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()));

	// A declared UTF-16 encoding is authoritative even without a BOM: its bytes can
	// otherwise pass the UTF-8 validation below (pure-ASCII UTF-16LE is full of NULs,
	// which are legal UTF-8).
	if let Some(enc) = declared
		&& (enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE)
	{
		return enc;
	}

	if std::str::from_utf8(data).is_ok() {
		return encoding_rs::UTF_8;
	}

	if let Some(enc) = declared {
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

/// Sniff `<meta charset>` / `<meta http-equiv=content-type>` from the document's first 1024 bytes
fn meta_charset(data: &[u8]) -> Option<&'static encoding_rs::Encoding> {
	let head: String = data[..data.len().min(1024)].iter().map(|&b| char::from(b)).collect();
	charset_label_in(&head)
}

/// Find a `charset` label in an ASCII-compatible text head
fn charset_label_in(head: &str) -> Option<&'static encoding_rs::Encoding> {
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

pub fn set_rate_limit(ctx: &mut impl Ctx, permits: i32, period: i32, unit: i32) {
	ctx.host().net.rate.set_rate(permits, period, unit);
}


#[cfg(test)]
mod tests {
	use super::*;
	use crate::WasmEnv;
	use crate::runtime::env::{NetState, test_ctx::TestCtx};
	use crate::runtime::limits::Limits;
	use crate::traits::{DirectPolicy, NetPolicy, NetRoute};
	use std::io::{Read, Write};
	use std::net::TcpListener;
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	struct EscalationPolicy {
		trawl: Option<String>,
		solver: Option<String>,
		clearances: Mutex<HashMap<String, crate::traits::ClearanceReplay>>,
	}

	/// Existing tests unrelated to the cap all get a generous limit; the cap's behavior is pinned by dedicated tests
	const TEST_BODY_MAX: u64 = 64 * 1024 * 1024;

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

		fn clearance_replay(&self, host: &str) -> Option<crate::traits::ClearanceReplay> {
			self.clearances.lock().expect("clearances lock").get(host).cloned()
		}

		fn solve_clearance(&self, url: &Url) -> Option<crate::traits::ClearanceReplay> {
			let api = self.solver.as_ref()?;
			let host = url.host_str().unwrap_or_default();
			let replay = crate::browser::trawl::solve_clearance(api, host)?;
			self.clearances
				.lock()
				.expect("clearances lock")
				.insert(host.to_string(), replay.clone());
			Some(replay)
		}

		fn clear_clearance(&self, host: &str) {
			self.clearances.lock().expect("clearances lock").remove(host);
		}
	}

	struct MockServer {
		addr: String,
		hits: Arc<AtomicUsize>,
		heads: Arc<Mutex<Vec<String>>>,
	}

	/// Bare HTTP/1.1 mock: one request per connection (the response carries connection:
	/// close); the handler receives the request-head text and returns complete response
	/// bytes. Shared by the origin and the trawl proxy.
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
		perform(net, &mut request, Duration::from_secs(10), TEST_BODY_MAX).expect("perform");
		request.response.expect("response stored")
	}

	/// Login-session trawl mock: /sessions creates (201), /scrape returns a login page
	/// plus a site cookie and the session browser's own cf_clearance, and every request
	/// head is recorded
	fn login_trawl_server() -> MockServer {
		mock_server(|head| {
			if head.starts_with("POST /sessions") {
				respond(
					"201 Created",
					&["content-type: application/json"],
					r#"{"id":"s","createdAt":0,"expiresAt":0,"busy":false,"keepPage":true}"#,
				)
			} else {
				respond(
					"200 OK",
					&["content-type: application/json"],
					r#"{"cookies":[{"name":"PHPSESSID","value":"sess1","domain":".127.0.0.1","path":"/"},{"name":"cf_clearance","value":"solver-bound","domain":".127.0.0.1","path":"/"}],"html":"登录成功"}"#,
				)
			}
		})
	}

	#[test]
	fn login_flow_escalates_to_session_on_challenge_and_rides_it() {
		let origin = mock_server(|_| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let trawl = login_trawl_server();
		let policy = Arc::new(EscalationPolicy {
			trawl: None,
			solver: None,
			..Default::default()
		});
		let mut net = NetState::new(policy, CookieJar::default());
		net.set_trawl(&format!("http://{}", trawl.addr));
		net.set_login_in_progress(true);
		let url = Url::parse(&format!("http://{}/login.php", origin.addr)).unwrap();

		// First login request: direct hits the challenge → escalate to the login session
		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(response.data, "登录成功".as_bytes());
		let heads = trawl.heads();
		assert_eq!(heads.len(), 2, "升会话：先幂等建会话，再 /scrape");
		assert!(heads[0].starts_with("POST /sessions"), "create first: {}", heads[0]);
		assert!(heads[1].starts_with("POST /scrape"), "scrape second: {}", heads[1]);
		// The session's site cookies flow back into the jar for later non-browser
		// requests; its cf_clearance must not (UA-bound to the session browser, it only
		// confuses CF on our own requests)
		let cookie = net.jar.header_for(&url).expect("jar header");
		assert!(cookie.contains("PHPSESSID=sess1"), "jar: {cookie}");
		assert!(!cookie.contains("cf_clearance"), "jar must not mirror session clearance: {cookie}");
		// Only requests inside basic_login() escalate; outside it the session is ignored
		net.set_login_in_progress(false);

		// Next login request rides the established session without re-creating it
		net.set_login_in_progress(true);
		let second = get(&mut net, &url);
		assert_eq!(second.data, "登录成功".as_bytes());
		let heads = trawl.heads();
		assert_eq!(heads.len(), 3, "已升会话后不再建会话");
		assert!(heads[2].starts_with("POST /scrape"), "third: {}", heads[2]);
	}

	#[test]
	fn login_flow_without_challenge_stays_direct() {
		let origin = mock_server(|_| respond("200 OK", &[], "plain-login-page"));
		let trawl = login_trawl_server();
		let policy = Arc::new(EscalationPolicy {
			trawl: None,
			solver: None,
			..Default::default()
		});
		let mut net = NetState::new(policy, CookieJar::default());
		net.set_trawl(&format!("http://{}", trawl.addr));
		net.set_login_in_progress(true);
		let url = Url::parse(&format!("http://{}/login.php", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.data, b"plain-login-page");
		assert_eq!(trawl.hits(), 0, "无 CF 挑战的登录源零浏览器开销");
	}

	#[test]
	fn login_session_id_is_isolated_from_solve_sessions() {
		assert_eq!(crate::browser::trawl::login_session_id("www.wenku8.net"), "sumidoku-login-www_wenku8_net");
		assert_ne!(
			crate::browser::trawl::login_session_id("www.wenku8.net"),
			"sumidoku-net-www_wenku8_net",
			"login and solve sessions must not share state"
		);
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
			..Default::default()
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

		// The clearance is in the jar: later requests carry it over direct connections without disturbing trawl
		let second = get(&mut net, &url);
		assert_eq!(second.status, StatusCode::OK);
		assert_eq!(second.data, b"direct-with-clearance");
		assert_eq!(origin.hits(), 2);
		assert_eq!(trawl.hits(), 1);
	}

	const SOLVER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0";

	fn solver_server() -> MockServer {
		mock_server(|head| {
			if head.starts_with("POST /sessions") {
				// solve_clearance creates the host session before scraping (201 created /
				// 409 already alive); any other status makes the solve fail
				respond(
					"201 Created",
					&["content-type: application/json"],
					r#"{"id":"s","createdAt":0,"expiresAt":0,"busy":false,"keepPage":true}"#,
				)
			} else {
				respond(
					"200 OK",
					&["content-type: application/json"],
					&format!(
						r#"{{"cookies":[{{"name":"cf_clearance","value":"ok"}}],"userAgent":"{SOLVER_UA}"}}"#
					),
				)
			}
		})
	}

	fn challenged_without_clearance(head: &str) -> Vec<u8> {
		if head.contains("cf_clearance=ok") {
			respond("200 OK", &[], "direct-with-clearance")
		} else {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		}
	}

	/// The replayed request must look like the solver's Firefox: its UA, no sec-ch-ua,
	/// Firefox's navigation Accept, cf_clearance leading the Cookie header
	fn assert_replay_head(head: &str, clearance: &str) {
		let head = head.to_ascii_lowercase();
		assert!(head.contains(&format!("user-agent: {}", SOLVER_UA.to_ascii_lowercase())), "UA: {head}");
		for &name in crate::traits::CHROME_ONLY_HEADERS {
			assert!(!head.contains(&format!("{name}:")), "重放不该带 {name}: {head}");
		}
		assert!(
			head.contains(&format!("accept: {}", crate::traits::FIREFOX_DOCUMENT_ACCEPT)),
			"Firefox Accept: {head}"
		);
		assert!(!head.contains("signed-exchange"), "Firefox 不发 signed-exchange: {head}");
		assert!(head.contains(&format!("cookie: cf_clearance={clearance}")), "cookie: {head}");
	}

	#[test]
	fn cf_challenge_solves_clearance_and_replays_direct() {
		let origin = mock_server(challenged_without_clearance);
		let solver = solver_server();
		let trawl = mock_server(|_head| respond("200 OK", &[], "unexpected"));
		let policy = Arc::new(EscalationPolicy {
			trawl: Some(format!("http://{}", trawl.addr)),
			solver: Some(format!("http://{}", solver.addr)),
			..Default::default()
		});
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/get", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(response.data, b"direct-with-clearance");
		assert_eq!(origin.hits(), 2, "挑战一次，解题后直连重放");
		assert_eq!(solver.hits(), 2, "解题一次：先幂等建会话，再 /scrape 解题");
		assert_eq!(trawl.hits(), 0, "解题成功就不该惊动 trawl 代理");
		assert_replay_head(&origin.heads()[1], "ok");
	}

	#[test]
	fn cached_clearance_replays_without_a_challenge_round_trip() {
		let origin = mock_server(challenged_without_clearance);
		let policy = Arc::new(EscalationPolicy::default());
		policy.clearances.lock().expect("clearances lock").insert(
			"127.0.0.1".into(),
			crate::traits::ClearanceReplay {
				user_agent: SOLVER_UA.into(),
				clearance: "ok".into(),
			},
		);
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/get", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(origin.hits(), 1, "缓存的 clearance 直接生效，不用先撞一次挑战");
		assert_replay_head(&origin.heads()[0], "ok");
	}

	#[test]
	fn refused_clearance_is_dropped_and_solved_fresh() {
		let origin = mock_server(|head| {
			if head.contains("cf_clearance=fresh") {
				respond("200 OK", &[], "direct-with-clearance")
			} else {
				respond(
					"403 Forbidden",
					&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
					"challenged",
				)
			}
		});
		let solver = mock_server(|head| {
			if head.starts_with("POST /sessions") {
				respond(
					"201 Created",
					&["content-type: application/json"],
					r#"{"id":"s","createdAt":0,"expiresAt":0,"busy":false,"keepPage":true}"#,
				)
			} else {
				respond(
					"200 OK",
					&["content-type: application/json"],
					&format!(
						r#"{{"cookies":[{{"name":"cf_clearance","value":"fresh"}}],"userAgent":"{SOLVER_UA}"}}"#
					),
				)
			}
		});
		let policy = Arc::new(EscalationPolicy {
			solver: Some(format!("http://{}", solver.addr)),
			..Default::default()
		});
		policy.clearances.lock().expect("clearances lock").insert(
			"127.0.0.1".into(),
			crate::traits::ClearanceReplay {
				user_agent: SOLVER_UA.into(),
				clearance: "old".into(),
			},
		);
		let policy_ref = policy.clone();
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/get", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(solver.hits(), 2, "被拒的旧 clearance 触发重解：先幂等建会话，再 /scrape 解题");
		assert!(origin.heads()[0].contains("cf_clearance=old"), "先用旧值: {}", origin.heads()[0]);
		assert_replay_head(&origin.heads()[1], "fresh");
		assert_eq!(
			policy_ref.clearances.lock().expect("clearances lock")["127.0.0.1"].clearance,
			"fresh",
			"缓存里换上了新值"
		);
	}

	#[test]
	fn failed_solve_falls_back_to_trawl_proxy() {
		let origin = mock_server(|_head| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let solver = mock_server(|_head| respond("500 Internal Server Error", &[], "busy"));
		let trawl = mock_server(|_head| respond("200 OK", &["content-type: text/html"], "solved-via-trawl"));
		let policy = Arc::new(EscalationPolicy {
			trawl: Some(format!("http://{}", trawl.addr)),
			solver: Some(format!("http://{}", solver.addr)),
			..Default::default()
		});
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(response.data, b"solved-via-trawl");
		assert_eq!(solver.hits(), 1, "先试解题");
		assert_eq!(trawl.hits(), 1, "解题失败落到 trawl 代理");
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
			..Default::default()
		});
		let mut net = NetState::new(policy, CookieJar::default());
		let url = Url::parse(&format!("http://{}/private", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::FORBIDDEN);
		assert_eq!(response.data, b"forbidden");
		assert_eq!(trawl.hits(), 0, "源站自己的 403 不该走 trawl");
	}

	#[test]
	fn browser_header_suite_fills_missing_and_keeps_source_values() {
		let origin = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url.clone());
		// A suite member provided by the source must win over the suite default, and there can only be one: inserting the default first would merge into two same-named headers
		request
			.headers
			.insert(reqwest::header::ACCEPT, HeaderValue::from_static("application/json"));
		perform(&mut net, &mut request, Duration::from_secs(10), TEST_BODY_MAX).expect("perform");

		let head = origin.heads().join("\n").to_ascii_lowercase();
		let accepts: Vec<&str> = head.lines().filter(|l| l.starts_with("accept:")).collect();
		assert_eq!(accepts.len(), 1, "Accept 不得被追加成两个: {head}");
		assert_eq!(accepts[0].trim(), "accept: application/json", "{head}");
		for &(name, value) in crate::traits::BROWSER_HEADERS {
			if name == "accept" {
				continue;
			}
			let expected = format!("{}: {}", name, value.to_ascii_lowercase());
			assert!(head.contains(&expected), "缺浏览器头 {name}: {head}");
		}
	}

	struct NoSuitePolicy;
	impl NetPolicy for NoSuitePolicy {
		fn route_for(&self, _url: &Url) -> NetRoute {
			NetRoute::Direct
		}
		fn user_agent(&self) -> Option<String> {
			None
		}
		fn browser_headers(&self, _url: &Url) -> bool {
			false
		}
	}

	#[test]
	fn browser_headers_disabled_omits_suite() {
		let origin = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(NoSuitePolicy), CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		get(&mut net, &url);
		let head = origin.heads().join("\n").to_ascii_lowercase();
		for &(name, _) in crate::traits::BROWSER_HEADERS {
			if name == "accept" {
				// reqwest falls back to accept: */* on its own, so we can only pin that the suite value is absent
				assert!(!head.contains("accept: text/html"), "{head}");
				continue;
			}
			assert!(!head.contains(&format!("{name}:")), "旋钮关掉后不该出现 {name}: {head}");
		}
		// The knob only governs the suite: the UA fallback is still added
		assert!(head.contains("user-agent:"), "{head}");
	}

	const DRF_PAGE: &str =
		"<!DOCTYPE html><html><head><title>Api Root – Django REST framework</title></head><body>Django REST framework</body></html>";

	/// Every suite header must be absent from the retried request; reqwest fills accept on
	/// its own, so for accept only the suite value can be pinned
	fn assert_suite_absent(head: &str) {
		for &(name, _) in crate::traits::BROWSER_HEADERS {
			if name == "accept" {
				assert!(!head.contains("accept: text/html"), "{head}");
				continue;
			}
			assert!(!head.contains(&format!("{name}:")), "suite header {name} survived the strip: {head}");
		}
	}

	#[test]
	fn drf_page_strips_injected_suite_and_retries_once() {
		let origin = mock_server(|head| {
			if head.to_ascii_lowercase().contains("sec-fetch-mode:") {
				respond("200 OK", &["content-type: text/html; charset=utf-8"], DRF_PAGE)
			} else {
				respond("200 OK", &["content-type: application/json"], "{\"code\":200,\"results\":\"ok\"}")
			}
		});
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/api/chapters", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.status, StatusCode::OK);
		assert_eq!(response.data, b"{\"code\":200,\"results\":\"ok\"}");
		assert_eq!(origin.hits(), 2, "the DRF page must trigger exactly one retry");
		let heads = origin.heads();
		assert!(heads[0].to_ascii_lowercase().contains("sec-fetch-mode:"), "first request carries the suite");
		assert_suite_absent(&heads[1].to_ascii_lowercase());
	}

	#[test]
	fn plain_html_page_does_not_trigger_drf_retry() {
		let origin = mock_server(|_head| respond("200 OK", &["content-type: text/html"], "<html><body>chapter list</body></html>"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.data, b"<html><body>chapter list</body></html>");
		assert_eq!(origin.hits(), 1, "a legitimate page fetch must not be retried");
	}

	#[test]
	fn drf_page_without_injected_suite_does_not_retry() {
		let origin = mock_server(|_head| respond("200 OK", &["content-type: text/html"], DRF_PAGE));
		let mut net = NetState::new(Arc::new(NoSuitePolicy), CookieJar::default());
		let url = Url::parse(&format!("http://{}/api/chapters", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.data, DRF_PAGE.as_bytes());
		assert_eq!(origin.hits(), 1, "nothing was injected, so there is nothing to strip");
	}

	#[test]
	fn drf_page_retries_only_once() {
		let origin = mock_server(|_head| respond("200 OK", &["content-type: text/html"], DRF_PAGE));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/api/chapters", origin.addr)).unwrap();

		let response = get(&mut net, &url);
		assert_eq!(response.data, DRF_PAGE.as_bytes());
		assert_eq!(origin.hits(), 2, "the fallback is bounded: the second DRF page is returned as is");
	}

	#[test]
	fn drf_retry_keeps_source_accept_header() {
		let origin = mock_server(|head| {
			if head.to_ascii_lowercase().contains("sec-fetch-mode:") {
				respond("200 OK", &["content-type: text/html"], DRF_PAGE)
			} else {
				respond("200 OK", &["content-type: application/json"], "{\"ok\":true}")
			}
		});
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/api/chapters", origin.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		request
			.headers
			.insert(reqwest::header::ACCEPT, HeaderValue::from_static("application/json"));
		perform(&mut net, &mut request, Duration::from_secs(10), TEST_BODY_MAX).expect("perform");

		assert_eq!(request.response.expect("response stored").data, b"{\"ok\":true}");
		assert_eq!(origin.hits(), 2);
		let heads = origin.heads();
		let second = heads[1].to_ascii_lowercase();
		let accepts: Vec<&str> = second.lines().filter(|l| l.starts_with("accept:")).collect();
		assert_eq!(accepts, ["accept: application/json"], "the source's own Accept is not part of the injected set: {second}");
		assert_suite_absent(&second);
	}

	/// Live smoke (wasm blocking net host): hits real sites, run manually only.
	/// Same assertion surface as the server-side smoke: protocol echo h2, a passive-CF
	/// sample site returning 200 with no challenge page, and no regression on a normal site.
	#[test]
	#[ignore = "打真实站点，只手动跑"]
	fn smoke_wasm_net_host_fingerprint() {
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());

		let peet = Url::parse("https://tls.peet.ws/api/all").unwrap();
		let response = get(&mut net, &peet);
		assert_eq!(response.status, StatusCode::OK);
		let body = String::from_utf8_lossy(&response.data);
		assert!(body.contains("\"http_version\": \"h2\""), "{body}");

		for site in ["https://toonily.com/", "https://bakamh.com/"] {
			let url = Url::parse(site).unwrap();
			let response = get(&mut net, &url);
			assert_eq!(response.status, StatusCode::OK, "{site}");
			let body = String::from_utf8_lossy(&response.data);
			assert!(!body.contains("Just a moment"), "{site} 不应是挑战页");
			assert!(
				body.len() > 20_000,
				"{site} 正文过短（{} 字节），疑似挑战页",
				body.len()
			);
			println!("{site} 200, {} bytes", body.len());
		}

		let mangadex = Url::parse("https://mangadex.org/").unwrap();
		let response = get(&mut net, &mangadex);
		assert_eq!(response.status, StatusCode::OK, "mangadex 无回归");
		println!("https://mangadex.org/ 200");
	}

	#[test]
	fn jar_sink_fires_on_set_and_expiry() {
		// Jar changes must be synced to the sink (the only persistence channel): a write fires Some, a real delete fires None
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
		// Browser reflux (insert) shares the same persistence channel as store, otherwise the login state is lost on restart
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
		// The jar backfilled from persistence builds the Cookie header by domain (including subdomain suffixes)
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
		// jar cookies are merged with the source's own Cookie header, not dropped when the source sets one (aligned with Aidoku)
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
		perform(&mut net, &mut request, Duration::from_secs(5), TEST_BODY_MAX).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("cookie: jar=1; src=1"), "head: {head}");
	}

	#[test]
	fn accept_language_default_is_sent() {
		// Add the default Accept-Language when the source does not set one (some WAFs 403 without it)
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		perform(&mut net, &mut request, Duration::from_secs(5), TEST_BODY_MAX).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("accept-language:"), "head: {head}");
	}

	#[test]
	fn accept_language_uses_source_value() {
		// The host's value derived from the source languages replaces the built-in default
		let server = mock_server(|_head| respond("200 OK", &[], "ok"));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		net.accept_language = "zh-CN,zh;q=0.9".into();
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		perform(&mut net, &mut request, Duration::from_secs(5), TEST_BODY_MAX).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("accept-language: zh-cn,zh;q=0.9"), "head: {head}");
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

	#[test]
	fn decode_body_ignores_lying_single_byte_charset() {
		// UTF-8 content served with a wrong single-byte label: strict UTF-8 validation
		// wins, otherwise "第1話" comes out as "ç¬¬1è©±"
		let mut headers = HeaderMap::new();
		headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=iso-8859-1"));
		assert_eq!(decode_body(&headers, "第1話".as_bytes()), "第1話");
	}

	#[test]
	fn decode_body_redetects_charset_past_sniff_window() {
		// The meta declaration sits beyond the 1024-byte sniff window and no Content-Type
		// charset is present: the lossy first pass leaves replacement chars, and the retry
		// recovers the declaration from the decoded text
		let mut body = b"<html><head>".to_vec();
		body.extend_from_slice(&[b'x'; 1500]);
		body.extend_from_slice(b"<meta charset=\"gbk\"></head><body>");
		body.extend_from_slice(&encoding_rs::GBK.encode("中文章节").0);
		let text = decode_body(&HeaderMap::new(), &body);
		assert!(text.ends_with("中文章节"), "text: {text}");
		assert!(!text.contains('\u{FFFD}'), "text: {text}");
	}

	#[test]
	fn decode_body_honors_declared_utf16_without_bom() {
		// Pure-ASCII UTF-16LE bytes are valid UTF-8 (NULs are legal), so the declared
		// UTF-16 charset must win over the strict UTF-8 validation
		let mut headers = HeaderMap::new();
		headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-16"));
		let utf16: Vec<u8> = "ab中".encode_utf16().flat_map(u16::to_le_bytes).collect();
		assert_eq!(decode_body(&headers, &utf16), "ab中");
	}

	/// Minimal GBK-encoded page: optional meta declaration + one p.t paragraph
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

	/// The wire format is zlib-wrapped: tower-http's decompression maps deflate to
	/// ZlibDecoder, and a bare DeflateEncoder stream cannot be unpacked
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

	/// Engine-less host: drive the net FFI (init/set_url/send/html) directly through
	/// TestCtx, then verify selector hits via the imports::html FFI — no real wasm guest
	/// needed
	struct Ffi {
		ctx: TestCtx,
	}

	impl Ffi {
		fn new() -> Self {
			let env = WasmEnv::for_test(
				Limits::default(),
				crate::browser::BrowserState::unconfigured(Duration::from_secs(1)),
			);
			Self { ctx: TestCtx::new(env) }
		}

		/// init(GET) → set_url → send → html, returning the document rid; any failed step asserts
		fn fetch_html(&mut self, url: &str) -> Rid {
			let rid = init(&mut self.ctx, 0);
			assert!(rid > 0, "init 返回 {rid}");
			let (p, l) = self.ctx.write_str(url);
			assert_eq!(set_url(&mut self.ctx, rid, p, l), 0, "set_url");
			assert_eq!(send(&mut self.ctx, rid), 0, "send");
			let doc = html(&mut self.ctx, rid);
			assert!(doc > 0, "html 返回 {doc}");
			doc
		}

		fn select_text(&mut self, doc: Rid, query: &str) -> String {
			let (p, l) = self.ctx.write_str(query);
			let el = crate::imports::html::select_first(&mut self.ctx, doc, p, l);
			assert!(el > 0, "select_first({query}) 返回 {el}");
			let s = crate::imports::html::text(&mut self.ctx, el);
			assert!(s > 0, "text 返回 {s}");
			let Some(StoreItem::String(text)) = self.ctx.env.store.get(s) else {
				panic!("rid {s} 不是 StoreItem::String");
			};
			text.clone()
		}
	}

	#[test]
	fn html_decodes_gbk_via_content_type() {
		// content-type declares gbk: html() decodes by the charset successfully and the selector hits the Chinese text
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
		// When the header has no charset, sniff the <meta charset> inside the document
		let page = gbk_page(true, "中文标题");
		let server = mock_server(move |_head| respond_bytes("200 OK", &["content-type: text/html"], &page));
		let mut ffi = Ffi::new();
		let doc = ffi.fetch_html(&format!("http://{}/", server.addr));
		assert_eq!(ffi.select_text(doc, "p.t"), "中文标题");
	}

	#[test]
	fn html_replaces_invalid_utf8_without_markers() {
		// Illegal UTF-8 with no BOM/declaration goes through replacement decoding; html() does not return InvalidString
		let body = b"<html><body><p class=\"t\">caf\xE9\x80</p></body></html>".to_vec();
		let server = mock_server(move |_head| respond_bytes("200 OK", &["content-type: text/html"], &body));
		let mut ffi = Ffi::new();
		let doc = ffi.fetch_html(&format!("http://{}/", server.addr));
		let text = ffi.select_text(doc, "p.t");
		assert!(text.starts_with("caf"), "text: {text:?}");
		assert!(text.contains('\u{FFFD}'), "text: {text:?}");
	}

	/// A request where the source overrides Accept-Encoding itself (the scenario where br/deflate were lost in the issue)
	fn get_with_accept_encoding(server: &MockServer, accept_encoding: &str) -> NetResponse {
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		request.headers.insert(
			reqwest::header::ACCEPT_ENCODING,
			HeaderValue::from_str(accept_encoding).expect("accept-encoding"),
		);
		perform(&mut net, &mut request, Duration::from_secs(5), TEST_BODY_MAX).expect("perform");
		request.response.expect("response stored")
	}

	#[test]
	fn gzip_body_is_decoded() {
		// No regression on the default path: gzip is declared and accepted
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
		// Aligned with URLSession's negotiation set: when the source does not set one, all three encodings are declared (header filling only happens in empty slots)
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
		// Behavior is unchanged when the source does not set a Cookie header: only the jar value is sent
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
		perform(&mut net, &mut request, Duration::from_secs(5), TEST_BODY_MAX).expect("perform");
		let head = server.heads().join("\n").to_ascii_lowercase();
		assert!(head.contains("cookie: jar=1"), "head: {head}");
	}

	fn perform_get(net: &mut NetState, url: &Url, body_max: u64) -> std::result::Result<NetResponse, SendFailure> {
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url.clone());
		perform(net, &mut request, Duration::from_secs(5), body_max)?;
		Ok(request.response.take().expect("response stored"))
	}

	#[test]
	fn body_at_cap_is_stored() {
		// Pin the boundary down: exactly at the cap passes, one byte over fails
		let body = vec![b'x'; 1024];
		let server = mock_server(move |_head| respond_bytes("200 OK", &[], &body));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let response = perform_get(&mut net, &url, 1024).expect("恰好在帽内");
		assert_eq!(response.data.len(), 1024);

		let server = mock_server(move |_head| respond_bytes("200 OK", &[], &vec![b'x'; 1025]));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		let result = perform(&mut net, &mut request, Duration::from_secs(5), 1024);
		assert!(matches!(result, Err(SendFailure::BodyTooLarge)), "got: {result:?}");
		assert!(request.response.is_none(), "超限响应不得落 store");
	}

	#[test]
	fn declared_length_over_cap_fails_fast() {
		let body = vec![b'x'; 2048];
		let server = mock_server(move |_head| respond_bytes("200 OK", &[], &body));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		let result = perform(&mut net, &mut request, Duration::from_secs(5), 1024);
		assert!(matches!(result, Err(SendFailure::BodyTooLarge)), "got: {result:?}");
		assert_eq!(server.hits(), 1, "超限不是重试面");
	}

	#[test]
	fn unannounced_over_cap_fails_while_streaming() {
		// A response without content-length is cut off by streaming accumulation, not discovered after reading to the end
		let body = vec![b'x'; 2048];
		let server = mock_server(move |_head| {
			let mut out = b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n".to_vec();
			out.extend_from_slice(&body);
			out
		});
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		let result = perform(&mut net, &mut request, Duration::from_secs(5), 1024);
		assert!(matches!(result, Err(SendFailure::BodyTooLarge)), "got: {result:?}");
	}

	#[test]
	fn gzip_decoded_bytes_count_against_cap() {
		// The header's content-length is the compressed size while what lands in memory is the decoded amount: the cap counts decoded bytes
		let body = gzip_body(&vec![b'x'; 2048]);
		assert!((body.len() as u64) < 1024, "夹具前提：压缩后小于帽");
		let server = mock_server(move |_head| respond_bytes("200 OK", &["content-encoding: gzip"], &body));
		let mut net = NetState::new(Arc::new(DirectPolicy { user_agent: None }), CookieJar::default());
		let url = Url::parse(&format!("http://{}/x", server.addr)).unwrap();
		let mut request = NetRequest::new(HttpMethod::Get);
		request.url = Some(url);
		let result = perform(&mut net, &mut request, Duration::from_secs(5), 1024);
		assert!(matches!(result, Err(SendFailure::BodyTooLarge)), "got: {result:?}");
	}

	#[test]
	fn send_reports_request_error_and_stores_nothing_over_cap() {
		// The source-side surface: send gets -10 (the ABI has no dedicated over-cap code) and the response does not land in the store
		let body = vec![b'x'; 2048];
		let server = mock_server(move |_head| respond_bytes("200 OK", &[], &body));
		let mut ffi = Ffi::new();
		ffi.ctx.env.limits.net_body_max = 1024;
		let rid = init(&mut ffi.ctx, 0);
		assert!(rid > 0, "init 返回 {rid}");
		let (p, l) = ffi.ctx.write_str(&format!("http://{}/x", server.addr));
		assert_eq!(set_url(&mut ffi.ctx, rid, p, l), 0, "set_url");
		assert_eq!(send(&mut ffi.ctx, rid), -10, "超限必须失败而不是无界缓冲");
		assert_eq!(data_len(&mut ffi.ctx, rid), -8, "超限响应不得可读");
	}
}
