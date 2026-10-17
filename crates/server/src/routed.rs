use crate::config::Config;
use aidoku_wasm::{NetPolicy, NetRoute};
use reqwest::{Client, StatusCode, Url};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 3;

/// 设计 §6 的 NetPolicy 实装：direct > relay(TRAWL) > 全局 proxy > direct。
/// 引擎（源 net 导入）与服务端直拉（封面/页图/下载）共用同一份路由。
pub struct SumidokuNetPolicy {
	user_agent: String,
	direct_domains: Vec<String>,
	relay_domains: Vec<String>,
	trawl_proxy: Option<String>,
	global_proxy: Option<String>,
	allow_direct_fallback: bool,
	ca_pem: Option<Vec<u8>>,
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
			ca_pem,
		})
	}

	fn domain_match(list: &[String], host: &str) -> bool {
		list.iter().any(|d| host == d || host.ends_with(&format!(".{d}")))
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
		if Self::domain_match(&self.relay_domains, host)
			&& let Some(proxy) = &self.trawl_proxy
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

	fn ca_certificate(&self) -> Option<Vec<u8>> {
		self.ca_pem.clone()
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

/// 服务端 async HTTP：按 policy 路由缓存 client、统一 UA、429/5xx 退避重试。
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
		let route = self.policy.route_for(url);
		if let Some(client) = self.clients.read().expect("clients lock").get(&route) {
			return Ok(client.clone());
		}
		let mut builder = Client::builder().use_rustls_tls();
		if let NetRoute::Proxy(proxy_url) = &route {
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
			.entry(route)
			.or_insert_with(|| client.clone());
		Ok(client)
	}

	/// 直接构造请求（调用方自定 method/body/headers）；UA 缺省补全局值
	pub fn request(&self, method: reqwest::Method, url: &Url) -> Result<reqwest::RequestBuilder, FetchError> {
		let client = self.client_for(url)?;
		let builder = client
			.request(method, url.clone())
			.header(reqwest::header::USER_AGENT, self.policy.user_agent().unwrap_or_default());
		Ok(builder)
	}

	/// 带 429/5xx 退避（尊 Retry-After，上限 3 次）与可选直连回退的 GET
	pub async fn get(&self, url: &Url) -> Result<reqwest::Response, FetchError> {
		let via_proxy = matches!(self.policy.route_for(url), NetRoute::Proxy(_));
		let mut attempt = 0;
		loop {
			attempt += 1;
			let result = self
				.request(reqwest::Method::GET, url)?
				.send()
				.await;
			let response = match result {
				Ok(response) => response,
				Err(e) => {
					if via_proxy && self.policy.allow_direct_fallback() {
						tracing::warn!(url = %url, "proxy fetch failed, direct fallback: {e}");
						return self.direct_get(url).await;
					}
					return Err(FetchError::Network(e));
				}
			};
			let status = response.status();
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
				return self.direct_get(url).await;
			}
			if !status.is_success() {
				return Err(FetchError::Status(status));
			}
			return Ok(response);
		}
	}

	async fn direct_get(&self, url: &Url) -> Result<reqwest::Response, FetchError> {
		let client = Client::builder()
			.use_rustls_tls()
			.build()
			.map_err(FetchError::Network)?;
		let response = client
			.get(url.clone())
			.header(reqwest::header::USER_AGENT, self.policy.user_agent().unwrap_or_default())
			.send()
			.await
			.map_err(FetchError::Network)?;
		if !response.status().is_success() {
			return Err(FetchError::Status(response.status()));
		}
		Ok(response)
	}
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
		// direct 优先于 relay
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
}
