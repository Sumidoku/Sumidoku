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

/// cookie jar 变更的持久化回调。在 actor 线程同步调用，实现里不得阻塞。
/// value 为 None 表示该 cookie 被删除（过期或 max-age<=0）。
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

/// 源语言（ISO 639 主标签）→ 宿主替 URLSession 补的 Accept-Language 默认值。
/// Aidoku 由 URLSession 按设备 locale 隐式带上该头；缺它时部分源站 WAF 直接 403
/// （如 zh.bilimanga 的图床）。server 直拉封面/页图与 wasm net 宿主共用这一份映射。
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
		// 未知或缺失语言回落到通用默认
		assert_eq!(default_accept_language(&[]), DEFAULT_ACCEPT_LANGUAGE);
		assert_eq!(default_accept_language(&["xx".into()]), DEFAULT_ACCEPT_LANGUAGE);
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
