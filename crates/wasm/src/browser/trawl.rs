//! trawl session API backend: one keepPage session per webview rid, with id
//! sumidoku-<sanitized source key>-<rid> so ownership is recognizable in trawl's sessions
//! list. HTTP has no persistent connection, so every error is treated as a single-op
//! (transient) failure — there is no connection to drop, and the next operation naturally
//! retries:
//! - 404/410 (session unknown/expired, underlying browser swapped, eval retired on timeout)
//!   → recreate under the same id and replay once; if the recreate hits 409 a concurrent
//!   side has already built it, so adopt it. A lost session means the browser state
//!   (including login cookies) is unrecoverable; the replay yields the honest result of a
//!   brand-new session
//! - 409 busy / 429 registry full / 503 sessions unavailable → single-op failure, passed
//!   up for logging
//! - create hits 409 → adopt: after a Sumidoku restart the rid counter resets to zero and a
//!   same-id session may still be alive
//!
//! eval value contract (aligned with the Boa path's JS ToString where alignable): string
//! as-is, number/bool literal, null (trawl maps undefined to null too) → "null";
//! objects/arrays get compact JSON — sources eval to extract scalar tokens, and JSON is
//! more useful than "[object Object]" and can be parsed again
use super::{BrowserClient, BrowserConnector, BrowserCookie, BrowserError};
use std::time::Duration;

/// The trawl server waits at most 30s for browser capacity when creating a session; the client leaves extra headroom
const SESSION_CREATE_TIMEOUT: Duration = Duration::from_secs(40);
/// cookies/delete are second-level operations
const SESSION_OP_TIMEOUT: Duration = Duration::from_secs(30);
/// The server-side maxTimeout must expire first to return a clean error (a timed-out
/// scrape/eval retires the session); the client timeout must be wider than it, otherwise
/// what we get is an uninformative interruption
const CLIENT_GRACE: Duration = Duration::from_secs(10);

/// Challenge solve budget. A solve runs inside a source call on the wasm actor thread,
/// so it must stay well under the call timeout (60s default); the async server path has
/// no such bound but shares the value
const SOLVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Challenge solve through the trawl scrape API — the shape aidoku-cf-relay uses: a
/// plain POST navigating the site's root. The answer carries the cf_clearance cookie and
/// the solving browser's user agent; CF binds the cookie to that UA, so both come back
/// together as a ClearanceReplay. The call carries a deterministic host-level session id
/// (sumidoku-net-<host>): the backend reuses the same session for every solve of this
/// host, so a later solve (after the challenged cooldown, or after a refused clearance)
/// rides the already-solved session instead of re-solving Turnstile from scratch.
/// Blocking: the wasm net host calls this on the actor thread, the async server path
/// wraps it in spawn_blocking.
pub fn solve_clearance(api_base: &str, host: &str) -> Option<crate::traits::ClearanceReplay> {
	#[derive(serde::Deserialize)]
	struct SolveAnswer {
		#[serde(default)]
		cookies: Vec<TrawlCookie>,
		#[serde(default, rename = "userAgent")]
		user_agent: Option<String>,
	}
	let client = reqwest::blocking::Client::builder()
		.use_rustls_tls()
		.timeout(SOLVE_TIMEOUT + CLIENT_GRACE)
		.build()
		.ok()?;
	// Deterministic host-level session id: every solve of the same host reuses the
	// backend's session and its solved challenge — one Turnstile solve per host, not
	// per request
	let session = session_id(
		"sumidoku-net",
		&host.replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
	);
	let body = serde_json::json!({
		"url": format!("https://{host}/"),
		"maxTimeout": SOLVE_TIMEOUT.as_millis() as u64,
		"sessionId": session,
		// Browser sessions require maxTier >= 3; tier 3 needs no proxy on session creation
		"maxTier": 3,
	});
	let text = client
		.post(format!("{}/scrape", api_base.trim_end_matches('/')))
		.header(reqwest::header::CONTENT_TYPE, "application/json")
		.body(body.to_string())
		.send()
		.and_then(|r| r.error_for_status())
		.and_then(|r| r.text())
		.ok()?;
	let answer: SolveAnswer = serde_json::from_str(&text).ok()?;
	let user_agent = answer.user_agent.filter(|ua| !ua.is_empty())?;
	// No cf_clearance means the site showed no challenge this time; the UA alone is still
	// a usable replay (aidoku-cf-relay forwards the same way)
	let clearance = answer
		.cookies
		.into_iter()
		.find(|c| c.name == crate::traits::CF_CLEARANCE_COOKIE)
		.map(|c| c.value)
		.unwrap_or_default();
	Some(crate::traits::ClearanceReplay { user_agent, clearance })
}

pub struct TrawlConnector {
	base: String,
	id_prefix: String,
}

impl TrawlConnector {
	pub fn new(api_url: &str, source_key: &str) -> Self {
		Self {
			base: api_url.trim_end_matches('/').to_string(),
			id_prefix: format!("sumidoku-{}", sanitize_key(source_key)),
		}
	}
}

impl BrowserConnector for TrawlConnector {
	fn connect(&self) -> Result<Box<dyn BrowserClient>, BrowserError> {
		let client = reqwest::blocking::Client::builder()
			.use_rustls_tls()
			.build()
			.map_err(|e| BrowserError::fatal(format!("trawl http client: {e}")))?;
		Ok(Box::new(TrawlClient {
			client,
			base: self.base.clone(),
			id_prefix: self.id_prefix.clone(),
		}))
	}

	fn supports_eval(&self) -> bool {
		true
	}

	fn supports_cookie_edit(&self) -> bool {
		false
	}
}

/// A trawl session id only allows letters/digits/underscore/hyphen (1-128 chars); dots,
/// spaces etc. in the source key are all squashed to hyphens
fn sanitize_key(key: &str) -> String {
	key.chars()
		.map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
		.collect()
}

fn session_id(id_prefix: &str, label: &str) -> String {
	let mut id = format!("{id_prefix}-{label}");
	id.truncate(128);
	id
}

/// Failure of a single operation: session_gone triggers a same-id recreate and replay; anything else is reported as-is
struct OpError {
	message: String,
	session_gone: bool,
}

impl OpError {
	fn gone(message: impl Into<String>) -> Self {
		Self { message: message.into(), session_gone: true }
	}

	fn other(message: impl Into<String>) -> Self {
		Self { message: message.into(), session_gone: false }
	}
}

impl From<OpError> for BrowserError {
	fn from(e: OpError) -> Self {
		BrowserError::transient(e.message)
	}
}

struct TrawlClient {
	client: reqwest::blocking::Client,
	base: String,
	id_prefix: String,
}

impl TrawlClient {
	/// Session-loss errors (404/410) are recreated under the same id and replayed once; if the replay fails again, report as-is
	fn with_recreate<T>(&self, session: &str, op: impl Fn(&Self) -> Result<T, OpError>) -> Result<T, OpError> {
		match op(self) {
			Err(e) if e.session_gone => {
				self.create_session(session)?;
				op(self)
			}
			result => result,
		}
	}

	fn create_session(&self, id: &str) -> Result<(), OpError> {
		let body = serde_json::json!({"id": id, "keepPage": true});
		let response = self
			.client
			.post(format!("{}/sessions", self.base))
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.body(body.to_string())
			.timeout(SESSION_CREATE_TIMEOUT)
			.send()
			.map_err(|e| OpError::other(format!("trawl session create: {e}")))?;
		match response.status().as_u16() {
			// 409 = an old session with the same id is still alive: adopt idempotently after a restart (keepPage was likewise set by the old instance)
			201 | 409 => Ok(()),
			status => Err(OpError::other(format!(
				"trawl session create failed (http {status}): {}",
				error_message(&body_text(response))
			))),
		}
	}

	fn delete_session(&self, id: &str) -> Result<(), OpError> {
		let response = self
			.client
			.delete(format!("{}/sessions/{id}", self.base))
			.timeout(SESSION_OP_TIMEOUT)
			.send()
			.map_err(|e| OpError::other(format!("trawl session delete: {e}")))?;
		match response.status().as_u16() {
			// 404 = already expired/lost, same effect as a delete
			200 | 404 => Ok(()),
			status => Err(OpError::other(format!(
				"trawl session delete failed (http {status}): {}",
				error_message(&body_text(response))
			))),
		}
	}

	fn scrape(
		&self,
		session: &str,
		url: &str,
		headers: &[(String, String)],
		timeout: Duration,
	) -> Result<Vec<BrowserCookie>, OpError> {
		let mut body = serde_json::json!({
			"url": url,
			"sessionId": session,
			"method": "GET",
			"maxTimeout": timeout.as_millis() as u64,
		});
		if !headers.is_empty() {
			let map: serde_json::Map<String, serde_json::Value> = headers
				.iter()
				.map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
				.collect();
			body["headers"] = map.into();
		}
		let response = self
			.client
			.post(format!("{}/scrape", self.base))
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.body(body.to_string())
			.timeout(timeout + CLIENT_GRACE)
			.send()
			.map_err(|e| OpError::other(format!("trawl scrape: {e}")))?;
		let status = response.status().as_u16();
		let text = body_text(response);
		match status {
			200 => {
				let result: ScrapeResult = serde_json::from_str(&text)
					.map_err(|e| OpError::other(format!("trawl scrape result parse: {e}")))?;
				Ok(result.cookies.into_iter().map(BrowserCookie::from).collect())
			}
			404 | 410 => Err(OpError::gone(format!(
				"trawl session {session} gone (http {status}): {}",
				error_message(&text)
			))),
			_ => Err(OpError::other(format!(
				"trawl scrape failed (http {status}): {}",
				error_message(&text)
			))),
		}
	}

	fn session_cookies(&self, session: &str) -> Result<Vec<BrowserCookie>, OpError> {
		let response = self
			.client
			.get(format!("{}/sessions/{session}/cookies", self.base))
			.timeout(SESSION_OP_TIMEOUT)
			.send()
			.map_err(|e| OpError::other(format!("trawl session cookies: {e}")))?;
		let status = response.status().as_u16();
		let text = body_text(response);
		match status {
			200 => {
				let result: CookiesResult = serde_json::from_str(&text)
					.map_err(|e| OpError::other(format!("trawl cookies result parse: {e}")))?;
				Ok(result.cookies.into_iter().map(BrowserCookie::from).collect())
			}
			404 | 410 => Err(OpError::gone(format!(
				"trawl session {session} gone (http {status}): {}",
				error_message(&text)
			))),
			_ => Err(OpError::other(format!(
				"trawl session cookies failed (http {status}): {}",
				error_message(&text)
			))),
		}
	}

	fn session_eval(&self, session: &str, source: &str, timeout: Duration) -> Result<String, OpError> {
		let body = serde_json::json!({
			"expression": source,
			"maxTimeout": timeout.as_millis() as u64,
		});
		let response = self
			.client
			.post(format!("{}/sessions/{session}/eval", self.base))
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.body(body.to_string())
			.timeout(timeout + CLIENT_GRACE)
			.send()
			.map_err(|e| OpError::other(format!("trawl session eval: {e}")))?;
		let status = response.status().as_u16();
		let text = body_text(response);
		match status {
			200 => {
				let result: EvalResult = serde_json::from_str(&text)
					.map_err(|e| OpError::other(format!("trawl eval result parse: {e}")))?;
				Ok(value_to_string(&result.value))
			}
			404 | 410 => Err(OpError::gone(format!(
				"trawl session {session} gone (http {status}): {}",
				error_message(&text)
			))),
			_ => Err(OpError::other(format!(
				"trawl session eval failed (http {status}): {}",
				error_message(&text)
			))),
		}
	}
}

impl BrowserClient for TrawlClient {
	fn create_context(&mut self, label: &str) -> Result<String, BrowserError> {
		let id = session_id(&self.id_prefix, label);
		self.create_session(&id)?;
		Ok(id)
	}

	fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
		Ok(self.delete_session(context)?)
	}

	fn load(
		&mut self,
		context: &str,
		url: &str,
		headers: &[(String, String)],
		timeout: Duration,
	) -> Result<Vec<BrowserCookie>, BrowserError> {
		Ok(self.with_recreate(context, |c| c.scrape(context, url, headers, timeout))?)
	}

	fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError> {
		Ok(self.with_recreate(context, |c| c.session_cookies(context))?)
	}

	fn eval(&mut self, context: &str, source: &str, timeout: Duration) -> Result<String, BrowserError> {
		Ok(self.with_recreate(context, |c| c.session_eval(context, source, timeout))?)
	}

	// The FFI intercepts per the connector capability bits and never reaches here; this is direct defense
	fn delete_cookie(
		&mut self,
		_context: &str,
		_name: &str,
		_domain: Option<&str>,
		_path: Option<&str>,
	) -> Result<(), BrowserError> {
		Err(BrowserError::transient("cookie edit not supported by the trawl backend"))
	}

	fn clear_cookies(&mut self, _context: &str) -> Result<(), BrowserError> {
		Err(BrowserError::transient("cookie edit not supported by the trawl backend"))
	}
}

/// trawl error bodies come in two shapes: native {"error": …} and the FlareSolverr
/// envelope {"message": …}; when neither is recognized, return the raw body (truncated to
/// avoid flooding the log)
fn error_message(body: &str) -> String {
	serde_json::from_str::<serde_json::Value>(body)
		.ok()
		.and_then(|v| {
			v.get("error")
				.or_else(|| v.get("message"))
				.and_then(|m| m.as_str())
				.map(str::to_string)
		})
		.unwrap_or_else(|| body.chars().take(200).collect())
}

fn body_text(response: reqwest::blocking::Response) -> String {
	response.text().unwrap_or_else(|e| format!("<body unreadable: {e}>"))
}

fn value_to_string(value: &serde_json::Value) -> String {
	match value {
		serde_json::Value::String(s) => s.clone(),
		serde_json::Value::Number(n) => n.to_string(),
		serde_json::Value::Bool(b) => b.to_string(),
		serde_json::Value::Null => "null".to_string(),
		other => serde_json::to_string(other).expect("json 序列化不会失败"),
	}
}

/// Only the cookies of a /scrape response are consumed: the post-navigation context snapshot comes back with the response, saving a round trip
#[derive(serde::Deserialize)]
struct ScrapeResult {
	#[serde(default)]
	cookies: Vec<TrawlCookie>,
}

#[derive(serde::Deserialize)]
struct CookiesResult {
	#[serde(default)]
	cookies: Vec<TrawlCookie>,
}

#[derive(serde::Deserialize)]
struct EvalResult {
	value: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct TrawlCookie {
	name: String,
	value: String,
	#[serde(default)]
	domain: String,
	#[serde(default)]
	path: String,
	#[serde(default)]
	expires: Option<i64>,
	#[serde(default)]
	secure: bool,
	#[serde(default, rename = "httpOnly")]
	http_only: bool,
}

impl From<TrawlCookie> for BrowserCookie {
	fn from(cookie: TrawlCookie) -> Self {
		Self {
			name: cookie.name,
			value: cookie.value,
			domain: cookie.domain,
			path: cookie.path,
			expires: cookie.expires,
			secure: cookie.secure,
			http_only: cookie.http_only,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::super::BrowserState;
	use super::*;
	use std::io::{BufRead, Read, Write};
	use std::sync::{Arc, Mutex};

	/// Bare HTTP/1.1 mock: one request per connection; records (method, path, body) and answers per the handler
	struct MockTrawl {
		url: String,
		requests: Arc<Mutex<Vec<(String, String, String)>>>,
	}

	impl MockTrawl {
		fn new(handler: impl Fn(&str, &str, &str) -> (u16, String) + Send + 'static) -> Self {
			let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock trawl");
			let port = listener.local_addr().expect("local addr").port();
			let requests = Arc::new(Mutex::new(Vec::new()));
			let seen = requests.clone();
			std::thread::spawn(move || {
				for stream in listener.incoming() {
					let Ok(mut stream) = stream else { break };
					let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone stream"));
					let mut head = String::new();
					loop {
						let mut line = String::new();
						match reader.read_line(&mut line) {
							Ok(0) | Err(_) => break,
							Ok(_) => {
								head.push_str(&line);
								if line == "\r\n" {
									break;
								}
							}
						}
					}
					if head.is_empty() {
						continue;
					}
					let mut lines = head.lines();
					let mut request_line = lines.next().unwrap_or("").split_whitespace();
					let method = request_line.next().unwrap_or("").to_string();
					let path = request_line.next().unwrap_or("").to_string();
					let content_length: usize = lines
						.filter_map(|l| l.split_once(':'))
						.find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
						.and_then(|(_, v)| v.trim().parse().ok())
						.unwrap_or(0);
					let mut body = vec![0u8; content_length];
					if reader.read_exact(&mut body).is_err() {
						continue;
					}
					let body = String::from_utf8_lossy(&body).to_string();
					seen.lock().expect("requests lock").push((method.clone(), path.clone(), body.clone()));
					let (status, resp_body) = handler(&method, &path, &body);
					let reason = match status {
						200 => "OK",
						201 => "Created",
						400 => "Bad Request",
						404 => "Not Found",
						409 => "Conflict",
						410 => "Gone",
						429 => "Too Many Requests",
						500 => "Internal Server Error",
						503 => "Service Unavailable",
						_ => "Status",
					};
					let response = format!(
						"HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp_body}",
						resp_body.len()
					);
					if stream.write_all(response.as_bytes()).is_err() {
						break;
					}
				}
			});
			Self { url: format!("http://127.0.0.1:{port}"), requests }
		}

		fn requests(&self) -> Vec<(String, String, String)> {
			self.requests.lock().expect("requests lock").clone()
		}
	}

	fn ok_session() -> (u16, String) {
		(201, r#"{"id":"s","createdAt":0,"expiresAt":0,"busy":false,"keepPage":true}"#.to_string())
	}

	fn ok_scrape(cookies: &str) -> (u16, String) {
		(
			200,
			format!(
				r#"{{"url":"https://a.example/","html":"","cookies":{cookies},"userAgent":"","statusCode":200,"tier":3,"sessionCached":false,"timings":[],"totalMs":1}}"#
			),
		)
	}

	#[test]
	fn solve_clearance_pairs_the_cookie_with_the_solver_ua() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/scrape") => (
				200,
				r#"{"cookies":[{"name":"cf_clearance","value":"abc"},{"name":"other","value":"x"}],"userAgent":"Camoufox/152"}"#
					.to_string(),
			),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let replay = solve_clearance(&mock.url, "cf.example").expect("solved");
		assert_eq!(
			replay,
			crate::traits::ClearanceReplay {
				user_agent: "Camoufox/152".into(),
				clearance: "abc".into(),
			}
		);
		let (_, _, body) = &mock.requests()[0];
		let body: serde_json::Value = serde_json::from_str(body).expect("json");
		assert_eq!(body["url"], "https://cf.example/");
		assert_eq!(body["sessionId"], "sumidoku-net-cf_example", "host-level session id is deterministic");
		assert_eq!(body["maxTier"], 3, "browser sessions require maxTier >= 3");
		assert!(body["maxTimeout"].is_number());
	}

	#[test]
	fn solve_clearance_failures_are_none() {
		let mock = MockTrawl::new(|_, _, _| (500, r#"{"error":"busy"}"#.to_string()));
		assert_eq!(solve_clearance(&mock.url, "cf.example"), None);
		// A solve without a user agent is no replay at all
		let mock = MockTrawl::new(|_, _, _| {
			(200, r#"{"cookies":[{"name":"cf_clearance","value":"abc"}],"userAgent":""}"#.to_string())
		});
		assert_eq!(solve_clearance(&mock.url, "cf.example"), None);
	}

	fn state_with(mock: &MockTrawl) -> BrowserState {
		BrowserState::new_trawl("test", &mock.url, Duration::from_secs(5))
	}

	/// Body assertion helper: parse the JSON and reconcile by key, so field order/whitespace do not matter
	fn body_json(requests: &[(String, String, String)], idx: usize) -> serde_json::Value {
		serde_json::from_str(&requests[idx].2).expect("请求体是 JSON")
	}

	#[test]
	fn lazy_create_on_first_load_and_scrape_carries_cookies() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("POST", "/scrape") => ok_scrape(
				r#"[{"name":"sid","value":"s3cret","domain":".a.example","path":"/","expires":1800000000,"httpOnly":true,"secure":true}]"#,
			),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		assert!(mock.requests().is_empty(), "未操作前不该有请求（懒创建）");

		let cookies = state
			.load(7, "https://a.example/", &[("x-extra".into(), "v1".into())])
			.expect("load");
		assert_eq!(
			cookies,
			vec![BrowserCookie {
				name: "sid".into(),
				value: "s3cret".into(),
				domain: ".a.example".into(),
				path: "/".into(),
				expires: Some(1_800_000_000),
				secure: true,
				http_only: true,
			}]
		);

		let requests = mock.requests();
		assert_eq!(requests.len(), 2, "首次 load 才建 session");
		assert_eq!((requests[0].0.as_str(), requests[0].1.as_str()), ("POST", "/sessions"));
		let create = body_json(&requests, 0);
		assert_eq!(create, serde_json::json!({"id": "sumidoku-test-7", "keepPage": true}));
		assert_eq!((requests[1].0.as_str(), requests[1].1.as_str()), ("POST", "/scrape"));
		let scrape = body_json(&requests, 1);
		assert_eq!(scrape["url"], "https://a.example/");
		assert_eq!(scrape["sessionId"], "sumidoku-test-7");
		assert_eq!(scrape["method"], "GET");
		assert_eq!(scrape["maxTimeout"], 5000);
		assert_eq!(scrape["headers"], serde_json::json!({"x-extra": "v1"}));

		state.load(7, "https://a.example/2", &[]).expect("再次 load");
		let requests = mock.requests();
		assert_eq!(requests.len(), 3, "同 rid 复用 session 不重建");
		assert_eq!(requests[2].1, "/scrape");
	}

	#[test]
	fn create_conflict_adopts_existing_session() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			// After a restart, the same-id session is still alive on the trawl side: 409 is adopted, no error, no recreate
			("POST", "/sessions") => (409, r#"{"error":"duplicate id"}"#.to_string()),
			("POST", "/scrape") => ok_scrape("[]"),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("409 采纳后照常导航");
		let requests = mock.requests();
		assert_eq!(requests.len(), 2);
		assert_eq!(requests[1].1, "/scrape");
	}

	#[test]
	fn eval_value_contract() {
		let mock = MockTrawl::new(|method, path, body| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("POST", p) if p.ends_with("/eval") => {
				let expr: serde_json::Value = serde_json::from_str(body).expect("eval body");
				let value = match expr["expression"].as_str().expect("expression") {
					"'tok' + 42" => serde_json::json!("tok42"),
					"1 + 2" => serde_json::json!(3),
					"0.1 + 0.2" => serde_json::json!(0.30000000000000004),
					"true" => serde_json::json!(true),
					"undefined" => serde_json::Value::Null,
					"({a: 1})" => serde_json::json!({"a": 1}),
					"[1, 2]" => serde_json::json!([1, 2]),
					other => panic!("未预期的 expression: {other}"),
				};
				(200, serde_json::json!({"value": value}).to_string())
			}
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		assert_eq!(state.eval(7, "'tok' + 42").expect("string 原样"), "tok42");
		assert_eq!(state.eval(7, "1 + 2").expect("number 字面"), "3");
		assert_eq!(state.eval(7, "0.1 + 0.2").expect("浮点字面"), "0.30000000000000004");
		assert_eq!(state.eval(7, "true").expect("bool 字面"), "true");
		assert_eq!(state.eval(7, "undefined").expect("undefined→null"), "null");
		assert_eq!(state.eval(7, "({a: 1})").expect("对象给 JSON"), r#"{"a":1}"#);
		assert_eq!(state.eval(7, "[1, 2]").expect("数组给 JSON"), "[1,2]");
		let requests = mock.requests();
		let eval = body_json(&requests, 1);
		assert_eq!(eval["maxTimeout"], 5000, "browser_timeout 换算成毫秒");
	}

	#[test]
	fn eval_js_exception_maps_to_transient() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("POST", p) if p.ends_with("/eval") => (500, r#"{"error":"ReferenceError: nope is not defined"}"#.to_string()),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let e = state.eval(7, "nope").expect_err("JS 异常是失败");
		assert!(!e.fatal, "HTTP 后端的单次失败不断连");
		assert!(e.message.contains("ReferenceError"), "错误带上游信息: {}", e.message);
	}

	#[test]
	fn cookies_read_includes_http_only() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("GET", "/sessions/sumidoku-test-7/cookies") => (
				200,
				r#"{"cookies":[{"name":"sess","value":"abc","domain":".a.example","path":"/","httpOnly":true,"secure":false}]}"#.to_string(),
			),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let cookies = state.cookies(7).expect("cookies");
		assert_eq!(cookies.len(), 1);
		assert!(cookies[0].http_only, "HttpOnly cookie 一并回流");
		assert_eq!(cookies[0].expires, None, "会话 cookie 无 expires");
		let requests = mock.requests();
		assert_eq!(requests.len(), 2, "先懒建 session 再读 cookies");
		assert_eq!((requests[1].0.as_str(), requests[1].1.as_str()), ("GET", "/sessions/sumidoku-test-7/cookies"));
	}

	#[test]
	fn gone_session_recreates_and_retries_once() {
		let calls = Arc::new(Mutex::new(0usize));
		let calls2 = calls.clone();
		let mock = MockTrawl::new(move |method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("GET", "/sessions/sumidoku-test-7/cookies") => {
				let mut n = calls2.lock().expect("calls lock");
				*n += 1;
				if *n == 1 {
					(410, r#"{"error":"browser was replaced"}"#.to_string())
				} else {
					(200, r#"{"cookies":[{"name":"s","value":"v","domain":"a.example","path":"/","httpOnly":false,"secure":false}]}"#.to_string())
				}
			}
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let cookies = state.cookies(7).expect("410 后重建重放成功");
		assert_eq!(cookies[0].name, "s");
		let requests = mock.requests();
		assert_eq!(
			requests.iter().map(|(m, p, _)| format!("{m} {p}")).collect::<Vec<_>>(),
			vec![
				"POST /sessions",
				"GET /sessions/sumidoku-test-7/cookies",
				"POST /sessions",
				"GET /sessions/sumidoku-test-7/cookies",
			],
			"410 → 同 id 重建 → 重放一次"
		);
	}

	#[test]
	fn gone_retry_failure_does_not_loop() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("GET", "/sessions/sumidoku-test-7/cookies") => (404, r#"{"error":"unknown session"}"#.to_string()),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let e = state.cookies(7).expect_err("重放仍 404 要失败");
		assert!(!e.fatal);
		let requests = mock.requests();
		assert_eq!(requests.len(), 4, "只重建重放一次，不循环: {requests:?}");
	}

	#[test]
	fn busy_maps_to_transient_without_recreate() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("POST", "/scrape") => (409, r#"{"error":"session busy"}"#.to_string()),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let e = state.load(7, "https://a.example/", &[]).expect_err("busy 是失败");
		assert!(!e.fatal, "409 busy 是单次失败");
		assert!(e.message.contains("session busy"), "带上游信息: {}", e.message);
		let requests = mock.requests();
		assert_eq!(requests.len(), 2, "409 不触发重建: {requests:?}");
	}

	#[test]
	fn full_registry_maps_to_transient() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => (429, r#"{"status":"error","message":"session registry full"}"#.to_string()),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		let e = state.load(7, "https://a.example/", &[]).expect_err("注册表满建不了 session");
		assert!(!e.fatal);
		assert!(e.message.contains("registry full"), "FlareSolverr 信封的 message 也认得: {}", e.message);
	}

	#[test]
	fn destroy_deletes_session_eagerly_and_404_is_ok() {
		let mock = MockTrawl::new(|method, path, _| match (method, path) {
			("POST", "/sessions") => ok_session(),
			("POST", "/scrape") => ok_scrape("[]"),
			("DELETE", "/sessions/sumidoku-test-7") => (200, r#"{"status":"ok"}"#.to_string()),
			("DELETE", "/sessions/sumidoku-test-9") => (404, r#"{"error":"unknown session"}"#.to_string()),
			_ => (500, r#"{"error":"unexpected"}"#.to_string()),
		});
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");
		state.load(9, "https://a.example/", &[]).expect("load");
		assert!(state.destroy(7).is_none(), "DELETE 200 无日志行");
		assert!(state.destroy(9).is_none(), "DELETE 404 同样无日志行（已丢与删除同效）");
		let requests = mock.requests();
		let deletes: Vec<_> = requests.iter().filter(|(m, _, _)| m == "DELETE").collect();
		assert_eq!(deletes.len(), 2, "destroy 立即 DELETE，不池化");
	}

	#[test]
	fn network_failure_is_transient_and_retried_next_call() {
		// Bind and immediately release to get a port nobody is listening on
		let port = std::net::TcpListener::bind("127.0.0.1:0")
			.expect("bind")
			.local_addr()
			.expect("addr")
			.port();
		let mut state = BrowserState::new_trawl("test", &format!("http://127.0.0.1:{port}"), Duration::from_secs(5));
		let e = state.load(7, "https://a.example/", &[]).expect_err("连不上是失败");
		assert!(!e.fatal, "HTTP 无连接可断，网络错也是单次失败");
		let e = state.load(7, "https://a.example/", &[]).expect_err("下一次操作独立重试");
		assert!(!e.fatal);
	}

	#[test]
	fn session_id_sanitized_and_bounded() {
		assert_eq!(session_id(&TrawlConnector::new("http://x", "zh.baozimh").id_prefix, "7"), "sumidoku-zh-baozimh-7");
		let long: String = "a".repeat(200);
		let id = session_id(&TrawlConnector::new("http://x", &long).id_prefix, "7");
		assert!(id.len() <= 128, "id 上限 128: {}", id.len());
		assert!(
			id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
			"id 只含合法字符: {id}"
		);
	}
}
