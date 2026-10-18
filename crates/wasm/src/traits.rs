use crate::runtime::store::DefaultValue;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use url::Url;

/// CF 挑战指纹：403/503 + CF 系响应头。cf-ray/server 挂在 CF 每个响应上，
/// 不区分挑战与硬拦截——误升级的成本只是 trawl 原样回传一次 403
pub fn is_cf_challenge(status: StatusCode, headers: &HeaderMap) -> bool {
	if status != StatusCode::FORBIDDEN && status != StatusCode::SERVICE_UNAVAILABLE {
		return false;
	}
	headers.contains_key("cf-ray")
		|| headers.contains_key("cf-mitigated")
		|| headers
			.get("server")
			.and_then(|v| v.to_str().ok())
			.is_some_and(|s| s.to_ascii_lowercase().starts_with("cloudflare"))
}

/// defaults.set 的持久化回调。在 actor 线程同步调用，实现里不得阻塞。
pub trait DefaultsSink: Send + Sync {
	fn set(&self, source_key: &str, key: &str, value: &DefaultValue);
}

pub struct NoopDefaultsSink;

impl DefaultsSink for NoopDefaultsSink {
	fn set(&self, _source_key: &str, _key: &str, _value: &DefaultValue) {}
}

pub trait HostLogger: Send + Sync {
	fn log(&self, source_key: &str, message: &str);
}

pub struct StdoutLogger;

impl HostLogger for StdoutLogger {
	fn log(&self, source_key: &str, message: &str) {
		println!("[{source_key}] {message}");
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NetRoute {
	Direct,
	Proxy(String),
}

pub trait NetPolicy: Send + Sync {
	fn route_for(&self, url: &Url) -> NetRoute;
	fn user_agent(&self) -> Option<String>;
	/// TRAWL 代理的 MITM CA（PEM），不走代理则返回 None
	fn ca_certificate(&self) -> Option<Vec<u8>> {
		None
	}
	/// 直连遭遇 CF 挑战时改走的路由（通常是 trawl 代理）；
	/// None 表示不升级——未配 trawl，或该 host 被显式钉死直连
	fn trawl_escalation(&self, url: &Url) -> Option<NetRoute> {
		let _ = url;
		None
	}
}

pub struct DirectPolicy {
	pub user_agent: Option<String>,
}

impl NetPolicy for DirectPolicy {
	fn route_for(&self, _url: &Url) -> NetRoute {
		NetRoute::Direct
	}

	fn user_agent(&self) -> Option<String> {
		self.user_agent.clone()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use reqwest::header::HeaderValue;

	fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
		let mut map = HeaderMap::new();
		for (k, v) in pairs {
			map.insert(*k, HeaderValue::from_str(v).unwrap());
		}
		map
	}

	#[test]
	fn cf_challenge_detected() {
		assert!(is_cf_challenge(
			StatusCode::FORBIDDEN,
			&headers(&[("cf-ray", "9a1b2c-NRT")])
		));
		assert!(is_cf_challenge(
			StatusCode::SERVICE_UNAVAILABLE,
			&headers(&[("server", "cloudflare")])
		));
		assert!(is_cf_challenge(
			StatusCode::FORBIDDEN,
			&headers(&[("cf-mitigated", "challenge")])
		));
	}

	#[test]
	fn cf_challenge_rejected() {
		// 无 CF 头的普通 403 是源站自己的拒绝，不该升级
		assert!(!is_cf_challenge(
			StatusCode::FORBIDDEN,
			&headers(&[("server", "nginx")])
		));
		// cf-ray 挂在 CF 每个响应上，状态不是 403/503 不算挑战
		assert!(!is_cf_challenge(
			StatusCode::OK,
			&headers(&[("cf-ray", "9a1b2c-NRT")])
		));
		assert!(!is_cf_challenge(
			StatusCode::NOT_FOUND,
			&headers(&[("cf-ray", "9a1b2c-NRT")])
		));
	}
}
