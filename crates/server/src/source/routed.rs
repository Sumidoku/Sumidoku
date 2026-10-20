use crate::config::Config;
use aidoku_wasm::{NetPolicy, NetRoute};
use reqwest::{Client, StatusCode, Url};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 3;

/// NetPolicy implementation: direct > relay(TRAWL) > challenged host(TRAWL) > global proxy > direct.
/// A direct hit on a CF challenge (403/503 + CF headers) escalates through trawl_escalation and marks the host
/// "challenged" — from then on it always goes via trawl: clearance is bound to the solving browser's UA, replaying
/// it over a direct connection is rejected by CF, so trawl itself must hold both clearance and UA (its Tier 1
/// replays over plain HTTP without launching a browser). The engine (source net imports) and server-side direct
/// fetches (covers/page images/downloads) share this one routing.
pub struct SumidokuNetPolicy {
	user_agent: String,
	direct_domains: Vec<String>,
	relay_domains: Vec<String>,
	trawl_proxy: Option<String>,
	global_proxy: Option<String>,
	allow_direct_fallback: bool,
	browser_headers: bool,
	ca_pem: Option<Vec<u8>>,
	/// Hosts that hit a CF challenge and now permanently go via trawl (in-process memory; the first request after a restart relearns)
	challenged: RwLock<HashSet<String>>,
}

impl SumidokuNetPolicy {
	pub fn from_config(config: &Config) -> Arc<Self> {
		let ca_pem = config
			.net
			.trawl_ca
			.as_ref()
			.and_then(|path| match std::fs::read(path) {
				Ok(pem) => Some(pem),
				Err(e) => {
					tracing::warn!("failed to read trawl ca {}: {e}", path.display());
					None
				}
			});
		Arc::new(Self {
			user_agent: config.user_agent.clone(),
			direct_domains: config.net.direct_domains.clone(),
			relay_domains: config.net.relay_domains.clone(),
			trawl_proxy: config.net.trawl_proxy.clone(),
			global_proxy: config.net.proxy.clone(),
			allow_direct_fallback: config.net.allow_direct_fallback,
			browser_headers: config.net.browser_headers,
			ca_pem,
			challenged: RwLock::new(HashSet::new()),
		})
	}

	fn domain_match(list: &[String], host: &str) -> bool {
		list.iter().any(|d| host == d || host.ends_with(&format!(".{d}")))
	}

	/// Whether this host has hit a CF challenge. Exact match: each mirror/subdomain that hits a challenge is
	/// recorded for itself, with no suffix spreading, so unshielded image hosts on the same domain are not dragged into the proxy.
	fn is_challenged(&self, host: &str) -> bool {
		self.challenged.read().expect("challenged lock").contains(host)
	}

	/// Remembers a host that hit a challenge; only called when there really is a trawl to escalate to
	fn mark_challenged(&self, host: &str) {
		if host.is_empty() {
			return;
		}
		let inserted = self
			.challenged
			.write()
			.expect("challenged lock")
			.insert(host.to_string());
		if inserted {
			tracing::info!(host, "cloudflare challenge seen, routing host through trawl");
		}
	}

	pub fn allow_direct_fallback(&self) -> bool {
		self.allow_direct_fallback
	}
}

impl NetPolicy for SumidokuNetPolicy {
	fn route_for(&self, url: &Url) -> NetRoute {
		let host = url.host_str().unwrap_or_default();
		if Self::domain_match(&self.direct_domains, host) {
			return NetRoute::Direct;
		}
		if let Some(proxy) = &self.trawl_proxy
			&& (Self::domain_match(&self.relay_domains, host) || self.is_challenged(host))
		{
			return NetRoute::Proxy(proxy.clone());
		}
		if let Some(proxy) = &self.global_proxy {
			return NetRoute::Proxy(proxy.clone());
		}
		NetRoute::Direct
	}

	fn user_agent(&self) -> Option<String> {
		Some(self.user_agent.clone())
	}

	fn browser_headers(&self, _url: &Url) -> bool {
		self.browser_headers
	}

	fn ca_certificate(&self) -> Option<Vec<u8>> {
		self.ca_pem.clone()
	}

	fn trawl_escalation(&self, url: &Url) -> Option<NetRoute> {
		let host = url.host_str().unwrap_or_default();
		// direct_domains is an explicit "never proxy" pin; CF escalation yields to it too
		if Self::domain_match(&self.direct_domains, host) {
			return None;
		}
		let proxy = self.trawl_proxy.clone()?;
		self.mark_challenged(host);
		Some(NetRoute::Proxy(proxy))
	}
}

#[derive(Debug)]
pub enum FetchError {
	Network(reqwest::Error),
	Status(StatusCode),
}

impl std::fmt::Display for FetchError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			FetchError::Network(e) => write!(f, "network error: {e}"),
			FetchError::Status(s) => write!(f, "unexpected status {s}"),
		}
	}
}

/// Server-side async HTTP: policy-routed cached clients, uniform UA, 429/5xx backoff retries.
pub struct RoutedClients {
	policy: Arc<SumidokuNetPolicy>,
	clients: RwLock<HashMap<NetRoute, Client>>,
}

impl RoutedClients {
	pub fn new(policy: Arc<SumidokuNetPolicy>) -> Self {
		Self {
			policy,
			clients: RwLock::new(HashMap::new()),
		}
	}

	pub fn policy(&self) -> &Arc<SumidokuNetPolicy> {
		&self.policy
	}

	pub fn client_for(&self, url: &Url) -> Result<Client, FetchError> {
		self.client_for_route(&self.policy.route_for(url))
	}

	fn client_for_route(&self, route: &NetRoute) -> Result<Client, FetchError> {
		if let Some(client) = self.clients.read().expect("clients lock").get(route) {
			return Ok(client.clone());
		}
		let mut builder = base_builder();
		if let NetRoute::Proxy(proxy_url) = route {
			let proxy = reqwest::Proxy::all(proxy_url)
				.map_err(FetchError::Network)?;
			builder = builder.proxy(proxy);
			if let Some(pem) = self.policy.ca_certificate() {
				let cert = reqwest::Certificate::from_pem(&pem)
					.map_err(FetchError::Network)?;
				builder = builder.add_root_certificate(cert);
			}
		}
		let client = builder.build().map_err(FetchError::Network)?;
		self.clients
			.write()
			.expect("clients lock")
			.entry(route.clone())
			.or_insert_with(|| client.clone());
		Ok(client)
	}

	/// Builds a request directly (no source-declared headers): UA/Accept-Language use the global defaults
	pub fn request(&self, method: reqwest::Method, url: &Url) -> Result<reqwest::RequestBuilder, FetchError> {
		self.request_with(method, url, &[], None)
	}

	/// A request carrying source-declared headers: the source's same-named headers win, and only the missing
	/// ones get the global UA and Accept-Language. reqwest's header() appends, so inserting the default first
	/// would pair it with the source value as two same-named headers; CF only honors the first (the default)
	/// → 403, hence always "check first, fill after" (same as the wasm net host).
	pub fn request_with(
		&self,
		method: reqwest::Method,
		url: &Url,
		headers: &[(String, String)],
		accept_language: Option<&str>,
	) -> Result<reqwest::RequestBuilder, FetchError> {
		let client = self.client_for(url)?;
		Ok(apply_default_headers(
			client.request(method, url.clone()),
			headers,
			self.policy.user_agent().unwrap_or_default(),
			accept_language,
			self.policy.browser_headers(url),
		))
	}

	/// GET with 429/5xx backoff (honors Retry-After, capped at 3 attempts), CF-challenge trawl escalation, and optional direct fallback
	pub async fn get(&self, url: &Url) -> Result<reqwest::Response, FetchError> {
		self.get_with(url, None).await
	}

	/// Same as get, plus an Accept-Language for the request: the host-side substitute for the device locale
	/// Aidoku's URLSession would add; some source sites' WAFs answer a flat 403 when it is missing
	pub async fn get_with(
		&self,
		url: &Url,
		accept_language: Option<&str>,
	) -> Result<reqwest::Response, FetchError> {
		let via_proxy = matches!(self.policy.route_for(url), NetRoute::Proxy(_));
		let mut attempt = 0;
		loop {
			attempt += 1;
			let result = self
				.request_with(reqwest::Method::GET, url, &[], accept_language)?
				.send()
				.await;
			let response = match result {
				Ok(response) => response,
				Err(e) => {
					if via_proxy && self.policy.allow_direct_fallback() {
						tracing::warn!(url = %url, "proxy fetch failed, direct fallback: {e}");
						return self.direct_get(url, accept_language).await;
					}
					return Err(FetchError::Network(e));
				}
			};
			let status = response.status();
			// CF escalation comes before backoff: a 503 challenge page must not burn the direct path's retry budget
			if !via_proxy
				&& aidoku_wasm::is_cf_challenge(status, response.headers())
				&& let Some(route) = self.policy.trawl_escalation(url)
			{
				tracing::info!(url = %url, status = %status, "cloudflare challenge, retrying via trawl");
				return self.get_via(&route, url, accept_language).await;
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
				tokio::time::sleep(wait).await;
				continue;
			}
			if status.is_client_error() && via_proxy && self.policy.allow_direct_fallback() {
				tracing::warn!(url = %url, status = %status, "proxy fetch rejected, direct fallback");
				return self.direct_get(url, accept_language).await;
			}
			if !status.is_success() {
				return Err(FetchError::Status(status));
			}
			return Ok(response);
		}
	}

	/// Single-shot GET over a given route, for trawl escalation: the solve itself is this request's retry, so looping would just re-solve
	async fn get_via(
		&self,
		route: &NetRoute,
		url: &Url,
		accept_language: Option<&str>,
	) -> Result<reqwest::Response, FetchError> {
		let client = self.client_for_route(route)?;
		let response = apply_default_headers(
			client.get(url.clone()),
			&[],
			self.policy.user_agent().unwrap_or_default(),
			accept_language,
			self.policy.browser_headers(url),
		)
		.send()
		.await
		.map_err(FetchError::Network)?;
		if !response.status().is_success() {
			return Err(FetchError::Status(response.status()));
		}
		Ok(response)
	}

	async fn direct_get(
		&self,
		url: &Url,
		accept_language: Option<&str>,
	) -> Result<reqwest::Response, FetchError> {
		let client = base_builder()
			.build()
			.map_err(FetchError::Network)?;
		let response = apply_default_headers(
			client.get(url.clone()),
			&[],
			self.policy.user_agent().unwrap_or_default(),
			accept_language,
			self.policy.browser_headers(url),
		)
		.send()
		.await
		.map_err(FetchError::Network)?;
		if !response.status().is_success() {
			return Err(FetchError::Status(response.status()));
		}
		Ok(response)
	}
}

/// Client base shared by direct and proxied routes: rustls pinned in one place, each route only adds its own proxy/CA
fn base_builder() -> reqwest::ClientBuilder {
	Client::builder().use_rustls_tls()
}

/// Assembles the default headers: the source's same-named headers win, and only the missing ones get the
/// global User-Agent, the browser header suite, and the host-side Accept-Language.
/// The key is "check first, fill after" — reqwest's header() appends, so inserting a default first would pair it with the source value as two same-named headers.
fn apply_default_headers(
	builder: reqwest::RequestBuilder,
	source_headers: &[(String, String)],
	global_user_agent: String,
	accept_language: Option<&str>,
	browser_headers: bool,
) -> reqwest::RequestBuilder {
	let has = |name: &str| {
		source_headers
			.iter()
			.any(|(key, _)| key.eq_ignore_ascii_case(name))
	};
	let has_user_agent = has("user-agent");
	let has_accept_language = has("accept-language");
	let mut builder = builder;
	if !has_user_agent {
		builder = builder.header(reqwest::header::USER_AGENT, global_user_agent);
	}
	if browser_headers {
		for &(name, value) in aidoku_wasm::BROWSER_HEADERS {
			if !has(name) {
				builder = builder.header(name, value);
			}
		}
	}
	for (name, value) in source_headers {
		builder = builder.header(name, value);
	}
	if !has_accept_language
		&& let Some(value) = accept_language.filter(|value| !value.is_empty())
	{
		builder = builder.header(reqwest::header::ACCEPT_LANGUAGE, value);
	}
	builder
}

#[cfg(test)]
mod tests {
	use super::*;

	fn policy(config: impl FnOnce(&mut Config)) -> Arc<SumidokuNetPolicy> {
		let mut c = Config::default();
		config(&mut c);
		SumidokuNetPolicy::from_config(&c)
	}

	#[test]
	fn route_priority() {
		let p = policy(|c| {
			c.net.direct_domains = vec!["direct.example.com".into()];
			c.net.relay_domains = vec!["cf.example.com".into(), "direct.example.com".into()];
			c.net.trawl_proxy = Some("http://trawl:8192".into());
			c.net.proxy = Some("http://global:8080".into());
		});
		let route = |u: &str| p.route_for(&Url::parse(u).unwrap());
		// direct wins over relay
		assert_eq!(route("https://direct.example.com/x"), NetRoute::Direct);
		assert_eq!(route("https://cdn.direct.example.com/x"), NetRoute::Direct);
		assert_eq!(
			route("https://cf.example.com/x"),
			NetRoute::Proxy("http://trawl:8192".into())
		);
		assert_eq!(
			route("https://img.cf.example.com/x"),
			NetRoute::Proxy("http://trawl:8192".into())
		);
		assert_eq!(
			route("https://other.example.org/x"),
			NetRoute::Proxy("http://global:8080".into())
		);
	}

	#[test]
	fn relay_without_trawl_falls_through() {
		let p = policy(|c| {
			c.net.relay_domains = vec!["cf.example.com".into()];
		});
		assert_eq!(
			p.route_for(&Url::parse("https://cf.example.com/x").unwrap()),
			NetRoute::Direct
		);
	}

	#[test]
	fn trawl_escalation_respects_config() {
		let p = policy(|c| {
			c.net.direct_domains = vec!["pinned.example.com".into()];
			c.net.trawl_proxy = Some("http://trawl:8192".into());
		});
		assert_eq!(
			p.trawl_escalation(&Url::parse("https://cf.example.com/x").unwrap()),
			Some(NetRoute::Proxy("http://trawl:8192".into()))
		);
		// A host pinned direct does not go through the proxy even for CF escalation
		assert_eq!(
			p.trawl_escalation(&Url::parse("https://pinned.example.com/x").unwrap()),
			None
		);

		let bare = policy(|_| {});
		assert_eq!(
			bare.trawl_escalation(&Url::parse("https://cf.example.com/x").unwrap()),
			None
		);
	}

	#[test]
	fn escalation_marks_host_sticky() {
		let p = policy(|c| {
			c.net.trawl_proxy = Some("http://trawl:8192".into());
		});
		let url = Url::parse("https://cf.example.com/x").unwrap();
		assert_eq!(p.route_for(&url), NetRoute::Direct);

		assert_eq!(
			p.trawl_escalation(&url),
			Some(NetRoute::Proxy("http://trawl:8192".into()))
		);
		// Once escalated it is remembered: the same host goes via trawl from then on
		assert_eq!(
			p.route_for(&url),
			NetRoute::Proxy("http://trawl:8192".into())
		);
		// Exact match, no spreading to sibling subdomains
		assert_eq!(
			p.route_for(&Url::parse("https://www.cf.example.com/x").unwrap()),
			NetRoute::Direct
		);
	}

	#[test]
	fn pinned_direct_host_is_not_marked() {
		let p = policy(|c| {
			c.net.direct_domains = vec!["pinned.example.com".into()];
			c.net.trawl_proxy = Some("http://trawl:8192".into());
		});
		let url = Url::parse("https://pinned.example.com/x").unwrap();
		assert_eq!(p.trawl_escalation(&url), None);
		assert_eq!(p.route_for(&url), NetRoute::Direct);
	}

	#[test]
	fn no_trawl_does_not_mark() {
		let p = policy(|_| {});
		let url = Url::parse("https://cf.example.com/x").unwrap();
		assert_eq!(p.trawl_escalation(&url), None);
		assert_eq!(p.route_for(&url), NetRoute::Direct);
	}

	struct MockServer {
		addr: String,
		hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
		heads: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
	}

	/// Bare HTTP/1.1 mock: one request per connection (responses carry connection: close);
	/// the handler receives the request-head text and returns a full response. Shared by the origin site and the trawl proxy.
	fn mock_server(handler: impl Fn(&str) -> String + Send + 'static) -> MockServer {
		use std::io::{Read, Write};
		let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
		let port = listener.local_addr().expect("local addr").port();
		let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let heads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
				hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
				heads2.lock().expect("heads lock").push(head.clone());
				if stream.write_all(handler(&head).as_bytes()).is_err() {
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
			self.hits.load(std::sync::atomic::Ordering::SeqCst)
		}

		fn heads(&self) -> Vec<String> {
			self.heads.lock().expect("heads lock").clone()
		}
	}

	fn respond(status: &str, extra_headers: &[&str], body: &str) -> String {
		let mut out = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
		for header in extra_headers {
			out.push_str(header);
			out.push_str("\r\n");
		}
		out.push_str("\r\n");
		out.push_str(body);
		out
	}

	#[tokio::test]
	async fn get_with_sends_accept_language() {
		let origin = mock_server(|_head| respond("200 OK", &["content-type: image/jpeg"], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let response = clients
			.get_with(&url, Some("zh-CN,zh;q=0.9"))
			.await
			.expect("get");
		assert_eq!(response.status(), StatusCode::OK);
		let head = origin.heads().join("\n").to_ascii_lowercase();
		assert!(
			head.contains("accept-language: zh-cn,zh;q=0.9"),
			"head: {head}"
		);
	}

	#[tokio::test]
	async fn get_without_accept_language_omits_header() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		clients.get(&url).await.expect("get");
		let head = origin.heads().join("\n").to_ascii_lowercase();
		assert!(!head.contains("accept-language:"), "head: {head}");
	}

	#[tokio::test]
	async fn request_with_keeps_source_user_agent_as_the_only_value() {
		let origin = mock_server(|_head| respond("200 OK", &["content-type: image/webp"], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/m1.webp", origin.addr)).unwrap();

		// The UA the source itself sends in get_image_request must beat the global value, and there must be exactly one:
		// reqwest's header() appends, CF only reads the first, and a duplicate means 403.
		let headers = vec![
			("User-Agent".to_string(), "NoyAcg/3.0".to_string()),
			("Referer".to_string(), "https://noymanga.com/".to_string()),
		];
		let response = clients
			.request_with(reqwest::Method::GET, &url, &headers, None)
			.expect("build")
			.send()
			.await
			.expect("send");
		assert_eq!(response.status(), StatusCode::OK);

		let head = origin.heads().join("\n").to_ascii_lowercase();
		let ua: Vec<&str> = head
			.lines()
			.filter(|l| l.starts_with("user-agent:"))
			.collect();
		assert_eq!(ua.len(), 1, "源 UA 不得被追加成两个: {head}");
		assert_eq!(ua[0].trim(), "user-agent: noyacg/3.0", "{head}");
		assert!(head.contains("referer: https://noymanga.com/"), "{head}");
		assert!(!head.contains("mozilla/5.0"), "全局 UA 不该出现: {head}");
	}

	#[tokio::test]
	async fn request_with_falls_back_to_global_user_agent() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let headers = vec![("Referer".to_string(), "https://noymanga.com/".to_string())];
		clients
			.request_with(reqwest::Method::GET, &url, &headers, Some("zh-CN,zh;q=0.9"))
			.expect("build")
			.send()
			.await
			.expect("send");

		let head = origin.heads().join("\n").to_ascii_lowercase();
		let expected = format!(
			"user-agent: {}",
			Config::default().user_agent.to_ascii_lowercase()
		);
		assert!(head.contains(&expected), "缺省应补全局 UA: {head}");
		assert!(
			head.contains("accept-language: zh-cn,zh;q=0.9"),
			"缺省应补 Accept-Language: {head}"
		);
	}

	#[tokio::test]
	async fn request_with_does_not_duplicate_accept_language() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let headers = vec![("Accept-Language".to_string(), "ja-JP".to_string())];
		clients
			.request_with(reqwest::Method::GET, &url, &headers, Some("zh-CN,zh;q=0.9"))
			.expect("build")
			.send()
			.await
			.expect("send");

		let head = origin.heads().join("\n").to_ascii_lowercase();
		let al: Vec<&str> = head
			.lines()
			.filter(|line| line.starts_with("accept-language:"))
			.collect();
		assert_eq!(al.len(), 1, "源 Accept-Language 不得被追加成两个: {head}");
		assert_eq!(al[0].trim(), "accept-language: ja-jp", "{head}");
	}

	#[tokio::test]
	async fn get_with_sends_browser_header_suite() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/manga/1", origin.addr)).unwrap();

		clients.get(&url).await.expect("get");
		let head = origin.heads().join("\n").to_ascii_lowercase();
		for &(name, value) in aidoku_wasm::BROWSER_HEADERS {
			let expected = format!("{}: {}", name, value.to_ascii_lowercase());
			assert!(head.contains(&expected), "缺浏览器头 {name}: {head}");
		}
	}

	#[tokio::test]
	async fn request_with_keeps_source_suite_headers() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/api/list", origin.addr)).unwrap();

		// Suite members the source itself sends (Accept / Sec-Fetch-Site, common on API requests) must beat the
		// suite defaults, and there must be exactly one: inserting the default first would pair into two same-named headers
		let headers = vec![
			("Accept".to_string(), "application/json".to_string()),
			("Sec-Fetch-Site".to_string(), "same-origin".to_string()),
		];
		clients
			.request_with(reqwest::Method::GET, &url, &headers, None)
			.expect("build")
			.send()
			.await
			.expect("send");

		let head = origin.heads().join("\n").to_ascii_lowercase();
		let accepts: Vec<&str> = head.lines().filter(|l| l.starts_with("accept:")).collect();
		assert_eq!(accepts.len(), 1, "Accept 不得被追加成两个: {head}");
		assert_eq!(accepts[0].trim(), "accept: application/json", "{head}");
		let sites: Vec<&str> = head
			.lines()
			.filter(|l| l.starts_with("sec-fetch-site:"))
			.collect();
		assert_eq!(sites.len(), 1, "Sec-Fetch-Site 不得被追加成两个: {head}");
		assert_eq!(sites[0].trim(), "sec-fetch-site: same-origin", "{head}");
		// Suite members the source did not set must still be filled in
		assert!(head.contains("sec-ch-ua:"), "{head}");
		assert!(head.contains("upgrade-insecure-requests: 1"), "{head}");
	}

	#[tokio::test]
	async fn browser_headers_disabled_omits_suite() {
		let origin = mock_server(|_head| respond("200 OK", &[], "img"));
		let clients = RoutedClients::new(policy(|c| c.net.browser_headers = false));
		let url = Url::parse(&format!("http://{}/api/list", origin.addr)).unwrap();

		clients.get(&url).await.expect("get");
		let head = origin.heads().join("\n").to_ascii_lowercase();
		for &(name, _) in aidoku_wasm::BROWSER_HEADERS {
			if name == "accept" {
				// reqwest itself falls back to accept: */*, so we can only pin the suite value as absent
				assert!(!head.contains("accept: text/html"), "{head}");
				continue;
			}
			assert!(!head.contains(&format!("{name}:")), "旋钮关掉后不该出现 {name}: {head}");
		}
		// The toggle only governs the suite: the global UA is still filled
		assert!(head.contains("user-agent:"), "{head}");
	}

	#[tokio::test]
	async fn get_escalates_cf_challenge_via_trawl() {
		let origin = mock_server(|_head| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let trawl = mock_server(|_head| respond("200 OK", &["content-type: text/html"], "solved-via-trawl"));
		let policy = policy(|c| {
			c.net.trawl_proxy = Some(format!("http://{}", trawl.addr));
		});
		let clients = RoutedClients::new(policy);
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let response = clients.get(&url).await.expect("get");
		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(response.text().await.expect("body"), "solved-via-trawl");
		assert_eq!(origin.hits(), 1, "直连一次撞挑战即升级");
		assert_eq!(trawl.hits(), 1);
		assert!(
			trawl.heads()[0].starts_with(&format!("GET http://{}/cover.jpg", origin.addr)),
			"代理收到 absolute-form 请求: {}",
			trawl.heads()[0].lines().next().unwrap_or_default()
		);
	}

	#[tokio::test]
	async fn get_without_trawl_keeps_403() {
		let origin = mock_server(|_head| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let clients = RoutedClients::new(policy(|_| {}));
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let err = clients.get(&url).await.expect_err("no trawl, 403 surfaces");
		assert!(
			matches!(err, FetchError::Status(s) if s == StatusCode::FORBIDDEN),
			"got: {err}"
		);
		assert_eq!(origin.hits(), 1, "403 不是重试面，不应有退避重发");
	}

	#[tokio::test]
	async fn get_non_cf_403_does_not_escalate() {
		let origin = mock_server(|_head| respond("403 Forbidden", &["server: nginx"], "forbidden"));
		let trawl = mock_server(|_head| respond("200 OK", &[], "unexpected"));
		let policy = policy(|c| {
			c.net.trawl_proxy = Some(format!("http://{}", trawl.addr));
		});
		let clients = RoutedClients::new(policy);
		let url = Url::parse(&format!("http://{}/private", origin.addr)).unwrap();

		let err = clients.get(&url).await.expect_err("origin 403 surfaces");
		assert!(
			matches!(err, FetchError::Status(s) if s == StatusCode::FORBIDDEN),
			"got: {err}"
		);
		assert_eq!(trawl.hits(), 0, "源站自己的 403 不该走 trawl");
	}

	#[tokio::test]
	async fn challenged_host_sticks_to_trawl() {
		let origin = mock_server(|_head| {
			respond(
				"403 Forbidden",
				&["cf-ray: 9a1b2c-NRT", "server: cloudflare", "content-type: text/html"],
				"challenged",
			)
		});
		let trawl = mock_server(|_head| respond("200 OK", &["content-type: text/html"], "solved-via-trawl"));
		let policy = policy(|c| {
			c.net.trawl_proxy = Some(format!("http://{}", trawl.addr));
		});
		let clients = RoutedClients::new(policy);
		let url = Url::parse(&format!("http://{}/cover.jpg", origin.addr)).unwrap();

		let first = clients.get(&url).await.expect("first get");
		assert_eq!(first.text().await.expect("body"), "solved-via-trawl");
		assert_eq!(origin.hits(), 1, "首个请求直连撞挑战后升级");
		assert_eq!(trawl.hits(), 1);

		// The host is remembered: later requests go straight via trawl instead of hitting the challenge at the origin
		let second = clients.get(&url).await.expect("second get");
		assert_eq!(second.text().await.expect("body"), "solved-via-trawl");
		assert_eq!(origin.hits(), 1, "常走 trawl 后源站不再被直连");
		assert_eq!(trawl.hits(), 2);
	}

	/// Live smoke test: hits real sites, not part of the fixed suite (run manually with nextest --run-ignored all).
	/// Pins three things: the protocol echo negotiates h2, passive CF sample sites answer 200 direct without a challenge page, and a normal site shows no regression.
	#[tokio::test]
	#[ignore = "打真实站点，只手动跑"]
	async fn smoke_direct_path_fingerprint() {
		let clients = RoutedClients::new(policy(|_| {}));

		let peet = Url::parse("https://tls.peet.ws/api/all").unwrap();
		let response = clients.get(&peet).await.expect("tls.peet.ws");
		assert_eq!(format!("{:?}", response.version()), "HTTP/2.0", "协议回声应协商 h2");
		let body = response.text().await.expect("peet body");
		assert!(body.contains("\"http_version\": \"h2\""), "{body}");
		println!("tls.peet.ws: negotiated HTTP/2.0");

		for site in ["https://toonily.com/", "https://bakamh.com/"] {
			let url = Url::parse(site).unwrap();
			let response = clients.get(&url).await.unwrap_or_else(|e| panic!("{site}: {e}"));
			assert_eq!(response.status(), StatusCode::OK, "{site}");
			let body = response.text().await.unwrap_or_else(|e| panic!("{site} body: {e}"));
			assert!(!body.contains("Just a moment"), "{site} 不应是挑战页");
			assert!(
				body.len() > 20_000,
				"{site} 正文过短（{} 字节），疑似挑战页",
				body.len()
			);
			println!("{site} 200, {} bytes", body.len());
		}

		let mangadex = Url::parse("https://mangadex.org/").unwrap();
		let response = clients.get(&mangadex).await.expect("mangadex");
		assert_eq!(response.status(), StatusCode::OK, "mangadex 无回归");
		println!("https://mangadex.org/ 200");
	}

	/// The direct client base (base_builder + test CA) must negotiate HTTP/2 against an h2-only origin:
	/// the local rustls echo offers only h2 as ALPN — a client without http2 cannot even complete the TLS handshake.
	#[tokio::test]
	async fn direct_client_negotiates_http2() {
		use tokio::io::AsyncReadExt;

		let rcgen::CertifiedKey { cert, key_pair } =
			rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("self-signed");
		let cert_der = cert.der().clone();
		let mut server_config = rustls::ServerConfig::builder()
			.with_no_client_auth()
			.with_single_cert(
				vec![cert_der.clone()],
				rustls::pki_types::PrivateKeyDer::Pkcs8(
					rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
				),
			)
			.expect("server config");
		server_config.alpn_protocols = vec![b"h2".to_vec()];
		let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server_config));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
		let port = listener.local_addr().expect("local addr").port();
		let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
		tokio::spawn(async move {
			let (stream, _) = listener.accept().await.expect("accept");
			let negotiated = match acceptor.accept(stream).await {
				Ok(mut tls) => {
					let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
					let mut preface = vec![0u8; 24];
					let _ = tls.read_exact(&mut preface).await;
					Some((alpn, preface))
				}
				Err(_) => None,
			};
			let _ = seen_tx.send(negotiated);
		});

		let client = base_builder()
			.add_root_certificate(reqwest::Certificate::from_der(cert_der.as_ref()).expect("cert"))
			.build()
			.expect("client");
		let url = Url::parse(&format!("https://localhost:{port}/")).unwrap();
		// The echo never answers any request: it closes as soon as the h2 preface arrives, so send's outcome is irrelevant
		let _ = client.get(url).send().await;

		let negotiated = tokio::time::timeout(Duration::from_secs(10), seen_rx)
			.await
			.expect("echo 应收到一次连接")
			.expect("seen channel");
		let (alpn, preface) = negotiated.expect("客户端不开 http2 时 ALPN 无交集，TLS 握手会失败");
		assert_eq!(alpn.as_deref(), Some(b"h2".as_slice()), "ALPN 应选中 h2");
		assert_eq!(
			preface,
			b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec(),
			"客户端应发出 h2 client preface"
		);
	}
}
