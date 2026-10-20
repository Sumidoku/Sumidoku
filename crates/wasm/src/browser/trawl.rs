//! trawl session API 后端：每 webview rid 一个 keepPage session，
//! id 取 sumidoku-<源 key 清洗>-<rid>，在 trawl 的 sessions 列表里能认出归属。
//! HTTP 无持久连接，错误全部按单次失败（transient）处理——没有连接可断，
//! 下一次操作自然重试：
//! - 404/410（session 未知/过期、底层浏览器被换、eval 超时被 retire）→ 同 id
//!   重建后重放一次；重建撞 409 说明并发侧已建好，直接采纳。session 丢失意味着
//!   浏览器态（含登录 cookie）已不可恢复，重放得到的是全新 session 的诚实结果
//! - 409 busy / 429 注册表满 / 503 sessions 不可用 → 单次失败交上层记日志
//! - create 撞 409 → 采纳：Sumidoku 重启后 rid 计数归零，同 id session 可能还活着
//!
//! eval 值契约（与 Boa 路径的 JS ToString 对齐到可对齐的部分）：string 原样、
//! number/bool 字面、null（trawl 把 undefined 也映成 null）→ "null"；对象/数组
//! 给紧凑 JSON——源 eval 是为提标量令牌，JSON 比 "[object Object]" 有用且可再 parse
use super::{BrowserClient, BrowserConnector, BrowserCookie, BrowserError};
use std::time::Duration;

/// trawl 建 session 服务端最多等 30s 浏览器容量，客户端留出余量
const SESSION_CREATE_TIMEOUT: Duration = Duration::from_secs(40);
/// cookies/delete 是秒级操作
const SESSION_OP_TIMEOUT: Duration = Duration::from_secs(30);
/// 服务端 maxTimeout 先到期才能回干净错误（超时的 scrape/eval 会 retire session），
/// 客户端超时必须宽于它，否则拿到的是无信息的中断
const CLIENT_GRACE: Duration = Duration::from_secs(10);

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

/// trawl session id 只允许字母/数字/下划线/连字符（1-128），源 key 里的
/// 点、空格等一律压成连字符
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

/// 单次操作的失败：session_gone 触发同 id 重建重放，其余原样上报
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
	/// session 丢失类错误（404/410）用同 id 重建并重放一次；重放再失败原样上报
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
			// 409 = 同 id 旧 session 还活着：重启后幂等采纳（keepPage 同样由旧实例置位）
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
			// 404 = 已过期/已丢，与删除同效
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

	// FFI 按 connector 能力位拦截后不会走到这里；直连防御
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

/// trawl 错误体两种形状：原生 {"error": …} 与 FlareSolverr 信封 {"message": …}，
/// 都认不出来回原文（截断防爆日志）
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

/// /scrape 响应只消费 cookies：导航后的 context 快照随响应一并回来，省一次往返
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

	/// 裸 HTTP/1.1 mock：每连接一个请求，记 (method, path, body) 后按 handler 应答
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

	fn state_with(mock: &MockTrawl) -> BrowserState {
		BrowserState::new_trawl("test", &mock.url, Duration::from_secs(5))
	}

	/// body 断言辅助：JSON 解析后按键对账，字段顺序/空白不影响
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
			// 重启后 trawl 侧同 id session 还活着：409 采纳，不报错不重建
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
		// 绑定后立即释放拿到一个确定没人监听的端口
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
