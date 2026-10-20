use crate::runtime::store::DefaultValue;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use url::Url;

/// CF challenge fingerprint: 403/503 + CF-family response headers. cf-ray/server ride on
/// every CF response and do not distinguish a challenge from a hard block — the cost of a
/// wrong escalation is just trawl relaying the 403 once more
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

/// Persistence callback for defaults.set. Invoked synchronously on the actor thread; implementations must not block.
pub trait DefaultsSink: Send + Sync {
	fn set(&self, source_key: &str, key: &str, value: &DefaultValue);
}

pub struct NoopDefaultsSink;

impl DefaultsSink for NoopDefaultsSink {
	fn set(&self, _source_key: &str, _key: &str, _value: &DefaultValue) {}
}

/// Persistence callback for cookie-jar changes. Invoked synchronously on the actor thread;
/// implementations must not block. A None value means the cookie was deleted (expired or max-age<=0).
pub trait CookieSink: Send + Sync {
	fn set(&self, domain: &str, name: &str, value: Option<&str>);
}

pub struct NoopCookieSink;

impl CookieSink for NoopCookieSink {
	fn set(&self, _domain: &str, _name: &str, _value: Option<&str>) {}
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
	/// Whether requests to this url get BROWSER_HEADERS added. Default on: passive shields
	/// only check the suite is present; turning it off is semantically honest for API/XHR —
	/// the navigate semantics of sec-fetch-* are the distorted part of the suite
	fn browser_headers(&self, url: &Url) -> bool {
		let _ = url;
		true
	}
	/// MITM CA (PEM) of the TRAWL proxy; None when not going through the proxy
	fn ca_certificate(&self) -> Option<Vec<u8>> {
		None
	}
	/// Route to switch to when a direct connection hits a CF challenge (usually the trawl
	/// proxy); None means no escalation — trawl is not configured, or this host is
	/// explicitly pinned to direct
	fn trawl_escalation(&self, url: &Url) -> Option<NetRoute> {
		let _ = url;
		None
	}
}

/// Browser header suite added to direct requests: passive Cloudflare validates the
/// consistency of "protocol (h2) + header suite" with the UA, not the TLS fingerprint —
/// a Chrome UA alone without these headers still gets 403 from some sites. The values
/// pair with the default User-Agent (Chrome 126 / macOS); when the UA changes, these must
/// change with it. A same-named header set by the source on the request always wins
/// (check first, add after — see the net host and the server's default-header path).
pub const BROWSER_HEADERS: &[(&str, &str)] = &[
	("sec-ch-ua", "\"Not/A)Brand\";v=\"8\", \"Chromium\";v=\"126\", \"Google Chrome\";v=\"126\""),
	("sec-ch-ua-mobile", "?0"),
	("sec-ch-ua-platform", "\"macOS\""),
	("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"),
	("sec-fetch-dest", "document"),
	("sec-fetch-mode", "navigate"),
	("sec-fetch-site", "none"),
	("sec-fetch-user", "?1"),
	("upgrade-insecure-requests", "1"),
];

/// Source language (ISO 639 primary tag) → the default Accept-Language the host adds on
/// behalf of URLSession. Aidoku has URLSession implicitly attach this header per device
/// locale; without it, some source sites' WAFs return 403 outright (e.g. zh.bilimanga's
/// image host). The server's direct cover/page fetches and the wasm net host share this
/// one mapping.
pub const DEFAULT_ACCEPT_LANGUAGE: &str = "en-US,en;q=0.9";

pub fn default_accept_language(languages: &[String]) -> String {
	let base = languages
		.first()
		.map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase())
		.unwrap_or_default();
	match base.as_str() {
		"zh" => "zh-CN,zh;q=0.9",
		"en" => "en-US,en;q=0.9",
		"ja" => "ja-JP,ja;q=0.9",
		"ko" => "ko-KR,ko;q=0.9",
		"es" => "es-ES,es;q=0.9",
		"fr" => "fr-FR,fr;q=0.9",
		"de" => "de-DE,de;q=0.9",
		"it" => "it-IT,it;q=0.9",
		"pt" => "pt-BR,pt;q=0.9",
		"ru" => "ru-RU,ru;q=0.9",
		"ar" => "ar-SA,ar;q=0.9",
		"id" => "id-ID,id;q=0.9",
		"th" => "th-TH,th;q=0.9",
		"vi" => "vi-VN,vi;q=0.9",
		"tr" => "tr-TR,tr;q=0.9",
		"pl" => "pl-PL,pl;q=0.9",
		"nl" => "nl-NL,nl;q=0.9",
		_ => DEFAULT_ACCEPT_LANGUAGE,
	}
	.to_string()
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
	fn accept_language_from_language() {
		assert_eq!(default_accept_language(&["zh".into()]), "zh-CN,zh;q=0.9");
		assert_eq!(default_accept_language(&["zh-Hant".into()]), "zh-CN,zh;q=0.9");
		assert_eq!(default_accept_language(&["ja".into()]), "ja-JP,ja;q=0.9");
		// Unknown or missing languages fall back to the generic default
		assert_eq!(default_accept_language(&[]), DEFAULT_ACCEPT_LANGUAGE);
		assert_eq!(default_accept_language(&["xx".into()]), DEFAULT_ACCEPT_LANGUAGE);
	}

	#[test]
	fn cf_challenge_rejected() {
		// A plain 403 without CF headers is the source site's own rejection and must not escalate
		assert!(!is_cf_challenge(
			StatusCode::FORBIDDEN,
			&headers(&[("server", "nginx")])
		));
		// cf-ray rides on every CF response; a non-403/503 status is not a challenge
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
