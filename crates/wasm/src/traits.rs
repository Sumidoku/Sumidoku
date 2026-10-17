use crate::store::DefaultValue;
use url::Url;

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
