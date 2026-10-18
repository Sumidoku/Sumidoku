//! webview 的真浏览器背衬：CDP 驱动用户自供的 Chromium sidecar。
//! 每个 webview rid 映射一个隔离 BrowserContext，cookie/会话不跨 rid 串味；
//! rid destroy 时 dispose 对应 context，连接断开（fatal 错误）后下一次操作
//! 重连，浏览器侧可能残留的 context 记进 stale 在重连成功时逐一 dispose。
//! 单次操作失败（导航超时、CDP 报错）不断连：context 里的登录会话不能因
//! 一次超时丢掉。

use crate::runtime::store::Rid;
use std::collections::HashMap;
use std::time::Duration;

/// 浏览器侧 cookie 的内部形状；编码成 ABI 形状在 imports/js.rs 完成
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserCookie {
	pub name: String,
	pub value: String,
	pub domain: String,
	pub path: String,
	pub expires: Option<i64>,
	pub secure: bool,
	pub http_only: bool,
}

#[derive(Debug)]
pub struct BrowserError {
	pub message: String,
	/// true = 连接已死（断连重连）；false = 单次失败（连接与 context 保留）
	pub fatal: bool,
}

impl BrowserError {
	pub fn fatal(message: impl Into<String>) -> Self {
		Self { message: message.into(), fatal: true }
	}

	pub fn transient(message: impl Into<String>) -> Self {
		Self { message: message.into(), fatal: false }
	}
}

impl std::fmt::Display for BrowserError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.message)
	}
}

pub trait CdpClient: Send + Sync {
	fn create_context(&mut self) -> Result<String, BrowserError>;
	fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError>;
	/// 在 context 里 GET 导航（带附加请求头），等 load 事件后回该 context
	/// 的 cookie 快照——导航期间的 Set-Cookie 与脚本写入一并回流，省一次往返
	fn load(
		&mut self,
		context: &str,
		url: &str,
		headers: &[(String, String)],
		timeout: Duration,
	) -> Result<Vec<BrowserCookie>, BrowserError>;
	fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError>;
	fn delete_cookie(
		&mut self,
		context: &str,
		name: &str,
		domain: Option<&str>,
		path: Option<&str>,
	) -> Result<(), BrowserError>;
	fn clear_cookies(&mut self, context: &str) -> Result<(), BrowserError>;
}

pub trait CdpConnector: Send + Sync {
	fn connect(&self) -> Result<Box<dyn CdpClient>, BrowserError>;
}

pub struct BrowserState {
	connector: Option<Box<dyn CdpConnector>>,
	timeout: Duration,
	client: Option<Box<dyn CdpClient>>,
	contexts: HashMap<Rid, String>,
	stale: Vec<String>,
}

impl BrowserState {
	/// ws_url 为 None 即未配置：所有 browser 操作不可达，调用方走显式不支持路径
	pub fn new(ws_url: Option<String>, timeout: Duration) -> Self {
		Self {
			connector: ws_url.map(|ws| Box::new(CdpWsConnector { ws }) as Box<dyn CdpConnector>),
			timeout,
			client: None,
			contexts: HashMap::new(),
			stale: Vec::new(),
		}
	}

	#[cfg(test)]
	pub fn with_connector(connector: Box<dyn CdpConnector>, timeout: Duration) -> Self {
		Self {
			connector: Some(connector),
			timeout,
			client: None,
			contexts: HashMap::new(),
			stale: Vec::new(),
		}
	}

	pub fn configured(&self) -> bool {
		self.connector.is_some()
	}

	/// rid destroy 时回收对应 context；返回要进源日志的行（dispose 失败不致命：
	/// 记录后随连接断开由浏览器侧自行回收）
	pub fn destroy(&mut self, rid: Rid) -> Option<String> {
		let context = self.contexts.remove(&rid)?;
		match self.client.as_mut() {
			Some(client) => {
				if let Err(e) = client.dispose_context(&context) {
					if e.fatal {
						self.disconnect();
					}
					return Some(format!("webview browser context dispose failed: {e}"));
				}
				None
			}
			None => {
				self.stale.push(context);
				None
			}
		}
	}

	fn disconnect(&mut self) {
		self.client = None;
		self.stale.extend(self.contexts.drain().map(|(_, context)| context));
	}

	fn client(&mut self) -> Result<&mut dyn CdpClient, BrowserError> {
		if self.client.is_none() {
			let connector = self.connector.as_ref().expect("configured checked by caller");
			let mut client = connector.connect()?;
			// 重连成功：清掉上一轮连接留下的浏览器侧 context
			for context in self.stale.drain(..) {
				let _ = client.dispose_context(&context);
			}
			self.client = Some(client);
		}
		Ok(self.client.as_mut().expect("client just connected").as_mut())
	}

	fn context(&mut self, rid: Rid) -> Result<String, BrowserError> {
		if let Some(context) = self.contexts.get(&rid) {
			return Ok(context.clone());
		}
		let context = self.client()?.create_context()?;
		self.contexts.insert(rid, context.clone());
		Ok(context)
	}

	/// fatal 错误断连：client 丢弃、context 簿记转入 stale，下次操作重连
	fn run<T>(&mut self, op: impl FnOnce(&mut dyn CdpClient) -> Result<T, BrowserError>) -> Result<T, BrowserError> {
		let result = op(self.client()?);
		if let Err(e) = &result
			&& e.fatal
		{
			self.disconnect();
		}
		result
	}

	pub fn load(&mut self, rid: Rid, url: &str, headers: &[(String, String)]) -> Result<Vec<BrowserCookie>, BrowserError> {
		let timeout = self.timeout;
		let context = self.context(rid)?;
		self.run(|client| client.load(&context, url, headers, timeout))
	}

	pub fn cookies(&mut self, rid: Rid) -> Result<Vec<BrowserCookie>, BrowserError> {
		let context = self.context(rid)?;
		self.run(|client| client.cookies(&context))
	}

	pub fn delete_cookie(
		&mut self,
		rid: Rid,
		name: &str,
		domain: Option<&str>,
		path: Option<&str>,
	) -> Result<(), BrowserError> {
		let context = self.context(rid)?;
		self.run(|client| client.delete_cookie(&context, name, domain, path))
	}

	pub fn clear_cookies(&mut self, rid: Rid) -> Result<(), BrowserError> {
		let context = self.context(rid)?;
		self.run(|client| client.clear_cookies(&context))
	}
}

struct CdpWsConnector {
	ws: String,
}

impl CdpConnector for CdpWsConnector {
	fn connect(&self) -> Result<Box<dyn CdpClient>, BrowserError> {
		Ok(Box::new(cdp::ChromiumoxideClient::connect(&self.ws)?))
	}
}

mod cdp;

#[cfg(test)]
pub(crate) mod mock {
	use super::*;
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	pub struct MockState {
		pub connects: usize,
		pub fail_connect: bool,
		next_context: usize,
		pub contexts: Vec<String>,
		pub disposed: Vec<String>,
		pub loads: Vec<(String, String)>,
		pub load_headers: Vec<Vec<(String, String)>>,
		pub cookies: HashMap<String, Vec<BrowserCookie>>,
		pub cleared: Vec<String>,
		pub deleted: Vec<(String, String, Option<String>, Option<String>)>,
		/// Some(true) = fatal（断连），Some(false) = 单次失败
		pub load_error: Option<bool>,
	}

	pub struct MockConnector(pub Arc<Mutex<MockState>>);

	impl CdpConnector for MockConnector {
		fn connect(&self) -> Result<Box<dyn CdpClient>, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.connects += 1;
			if state.fail_connect {
				return Err(BrowserError::fatal("mock connect failed"));
			}
			Ok(Box::new(MockClient(self.0.clone())))
		}
	}

	struct MockClient(Arc<Mutex<MockState>>);

	impl CdpClient for MockClient {
		fn create_context(&mut self) -> Result<String, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.next_context += 1;
			let id = format!("ctx-{}", state.next_context);
			state.contexts.push(id.clone());
			Ok(id)
		}

		fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.disposed.push(context.to_string());
			state.contexts.retain(|c| c != context);
			Ok(())
		}

		fn load(
			&mut self,
			context: &str,
			url: &str,
			headers: &[(String, String)],
			_timeout: Duration,
		) -> Result<Vec<BrowserCookie>, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			if let Some(fatal) = state.load_error {
				return Err(if fatal {
					BrowserError::fatal("mock connection died")
				} else {
					BrowserError::transient("mock navigation timed out")
				});
			}
			state.loads.push((context.to_string(), url.to_string()));
			state.load_headers.push(headers.to_vec());
			Ok(state.cookies.get(context).cloned().unwrap_or_default())
		}

		fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError> {
			let state = self.0.lock().expect("mock lock");
			Ok(state.cookies.get(context).cloned().unwrap_or_default())
		}

		fn delete_cookie(
			&mut self,
			context: &str,
			name: &str,
			domain: Option<&str>,
			path: Option<&str>,
		) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.deleted.push((
				context.to_string(),
				name.to_string(),
				domain.map(str::to_string),
				path.map(str::to_string),
			));
			if let Some(cookies) = state.cookies.get_mut(context) {
				cookies.retain(|c| {
					!(c.name == name
						&& domain.is_none_or(|d| c.domain == d)
						&& path.is_none_or(|p| c.path == p))
				});
			}
			Ok(())
		}

		fn clear_cookies(&mut self, context: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.cleared.push(context.to_string());
			state.cookies.remove(context);
			Ok(())
		}
	}

	pub fn cookie(name: &str, value: &str, domain: &str) -> BrowserCookie {
		BrowserCookie {
			name: name.into(),
			value: value.into(),
			domain: domain.into(),
			path: "/".into(),
			expires: None,
			secure: false,
			http_only: false,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::mock::*;
	use super::*;
	use std::sync::{Arc, Mutex};

	fn state_with(mock: &Arc<Mutex<MockState>>) -> BrowserState {
		BrowserState::with_connector(Box::new(MockConnector(mock.clone())), Duration::from_secs(5))
	}

	#[test]
	fn context_created_once_per_rid_and_reused() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");
		state.load(7, "https://a.example/2", &[]).expect("load");
		state.load(9, "https://b.example/", &[]).expect("load");
		let mock = mock.lock().expect("mock lock");
		assert_eq!(mock.contexts, vec!["ctx-1", "ctx-2"], "每个 rid 一个 context");
		assert_eq!(
			mock.loads,
			vec![
				("ctx-1".into(), "https://a.example/".into()),
				("ctx-1".into(), "https://a.example/2".into()),
				("ctx-2".into(), "https://b.example/".into()),
			],
			"同 rid 复用同一 context"
		);
	}

	#[test]
	fn destroy_disposes_context_once() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");
		assert!(state.destroy(7).is_none());
		assert!(state.destroy(7).is_none(), "重复 destroy 是无害 no-op");
		assert_eq!(mock.lock().expect("mock lock").disposed, vec!["ctx-1"]);
	}

	#[test]
	fn connect_failure_retries_on_next_call() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		mock.lock().expect("mock lock").fail_connect = true;
		let mut state = state_with(&mock);
		assert!(state.load(7, "https://a.example/", &[]).is_err());
		mock.lock().expect("mock lock").fail_connect = false;
		state.load(7, "https://a.example/", &[]).expect("重连后成功");
		assert_eq!(mock.lock().expect("mock lock").connects, 2, "失败后下一次操作重连");
	}

	#[test]
	fn fatal_error_disconnects_and_stale_contexts_disposed_on_reconnect() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");
		state.load(9, "https://b.example/", &[]).expect("load");

		mock.lock().expect("mock lock").load_error = Some(true);
		assert!(state.load(7, "https://a.example/x", &[]).is_err());

		mock.lock().expect("mock lock").load_error = None;
		state.load(7, "https://a.example/y", &[]).expect("重连后成功");
		let mock = mock.lock().expect("mock lock");
		assert_eq!(mock.connects, 2, "fatal 后重连一次");
		let mut disposed = mock.disposed.clone();
		disposed.sort();
		assert_eq!(disposed, vec!["ctx-1", "ctx-2"], "旧连接的 context 在重连成功时逐一 dispose");
		assert_eq!(mock.loads.last().unwrap().0, "ctx-3", "重连后 rid 拿到新 context");
	}

	#[test]
	fn transient_error_keeps_connection_and_context() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");

		mock.lock().expect("mock lock").load_error = Some(false);
		assert!(state.load(7, "https://a.example/x", &[]).is_err());

		mock.lock().expect("mock lock").load_error = None;
		state.load(7, "https://a.example/y", &[]).expect("同连接恢复");
		let mock = mock.lock().expect("mock lock");
		assert_eq!(mock.connects, 1, "单次失败不重连");
		assert!(mock.disposed.is_empty(), "单次失败不清 context");
		assert_eq!(mock.loads.last().unwrap().0, "ctx-1", "context 保留");
	}

	#[test]
	fn destroy_without_connection_marks_context_stale() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");
		// 模拟连接已死被 disconnect：destroy 只能记账，重连时补 dispose
		mock.lock().expect("mock lock").load_error = Some(true);
		assert!(state.load(9, "https://b.example/", &[]).is_err(), "触发 fatal 断连");
		mock.lock().expect("mock lock").load_error = None;

		assert!(state.destroy(7).is_none());
		state.load(9, "https://b.example/", &[]).expect("重连");
		let disposed = &mock.lock().expect("mock lock").disposed;
		assert!(disposed.contains(&"ctx-1".to_string()), "stale context 重连时补 dispose: {disposed:?}");
	}

	#[test]
	fn unconfigured_state_reports_not_configured() {
		let state = BrowserState::new(None, Duration::from_secs(1));
		assert!(!state.configured());
		assert!(BrowserState::new(Some("ws://x".into()), Duration::from_secs(1)).configured());
	}
}
