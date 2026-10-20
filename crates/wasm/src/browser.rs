//! Real-browser backing for webviews: the trawl session API (browser/trawl.rs, an external HTTP sidecar).
//! Each webview rid maps to an isolated browser session; cookies and session state never leak
//! across rids, and destroying a rid disposes its session. After a fatal (connection-level) error
//! the next operation reconnects; sessions possibly left behind on the browser side are recorded
//! as stale and disposed one by one once reconnected. A single operation failure (navigation
//! timeout, backend error) does not disconnect: a login session must not be lost over one timeout.

use crate::runtime::store::Rid;
use std::collections::HashMap;
use std::time::Duration;

/// Internal shape of a browser-side cookie; encoding into the ABI shape happens in imports/js.rs
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
	/// true = connection is dead (disconnect and reconnect); false = single-op failure (connection and context kept)
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

pub trait BrowserClient: Send + Sync {
	/// label is a session-naming hint for the backend (trawl uses it to build the session id)
	fn create_context(&mut self, label: &str) -> Result<String, BrowserError>;
	fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError>;
	/// GET-navigate inside the context (with extra request headers); after the load event,
	/// returns a cookie snapshot of that context — Set-Cookie and script writes during
	/// navigation flow back together, saving a round trip
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
	/// Evaluate in the page's main JS realm and return the string result (the backend awaits
	/// the returned promise); whether this method is used is decided by the connector's supports_eval
	fn eval(&mut self, _context: &str, _source: &str, _timeout: Duration) -> Result<String, BrowserError> {
		Err(BrowserError::transient("eval not supported by this browser backend"))
	}
}

pub trait BrowserConnector: Send + Sync {
	fn connect(&self) -> Result<Box<dyn BrowserClient>, BrowserError>;
	/// Whether webview_eval is routed to the browser (trawl's page eval)
	fn supports_eval(&self) -> bool {
		false
	}
	/// Whether cookie editing (delete/clear) is available; the trawl session API has no such endpoints
	fn supports_cookie_edit(&self) -> bool {
		true
	}
}

pub struct BrowserState {
	connector: Option<Box<dyn BrowserConnector>>,
	timeout: Duration,
	client: Option<Box<dyn BrowserClient>>,
	contexts: HashMap<Rid, String>,
	stale: Vec<String>,
}

impl BrowserState {
	/// No browser backend configured: all browser operations are unreachable and callers take the explicit unsupported path
	pub fn unconfigured(timeout: Duration) -> Self {
		Self {
			connector: None,
			timeout,
			client: None,
			contexts: HashMap::new(),
			stale: Vec::new(),
		}
	}

	#[cfg(test)]
	pub fn with_connector(connector: Box<dyn BrowserConnector>, timeout: Duration) -> Self {
		Self {
			connector: Some(connector),
			timeout,
			client: None,
			contexts: HashMap::new(),
			stale: Vec::new(),
		}
	}

	/// trawl session backend: HTTP has no persistent connection, so the connection-state
	/// fields are moot for it — disconnect/reconnect semantics come naturally from "every
	/// operation is an independent request"
	pub fn new_trawl(source_key: &str, api_url: &str, timeout: Duration) -> Self {
		Self {
			connector: Some(Box::new(trawl::TrawlConnector::new(api_url, source_key))),
			timeout,
			client: None,
			contexts: HashMap::new(),
			stale: Vec::new(),
		}
	}

	pub fn configured(&self) -> bool {
		self.connector.is_some()
	}

	/// Whether webview_eval is routed to the browser backend (trawl); when false the FFI keeps the Boa page model
	pub fn supports_eval(&self) -> bool {
		self.connector.as_ref().is_some_and(|c| c.supports_eval())
	}

	/// Whether cookie editing (webview_delete_cookie) is available; the trawl session API has no such endpoints
	pub fn supports_cookie_edit(&self) -> bool {
		self.connector.as_ref().is_some_and(|c| c.supports_cookie_edit())
	}

	/// Reclaim the context of a rid on destroy; returns the line for the source log (a
	/// failed dispose is not fatal: it is logged, and the browser side reclaims it on its
	/// own when the connection drops)
	pub fn destroy(&mut self, rid: Rid) -> Option<String> {
		let context = self.contexts.remove(&rid)?;
		match self.client.as_mut() {
			Some(client) => {
				if let Err(e) = client.dispose_context(&context) {
					if e.fatal {
						self.disconnect();
						// The dispose may not have reached the browser; finish it on reconnect
						self.stale.push(context);
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

	fn client(&mut self) -> Result<&mut dyn BrowserClient, BrowserError> {
		if self.client.is_none() {
			let connector = self.connector.as_ref().expect("configured checked by caller");
			let mut client = connector.connect()?;
			// Reconnected: clear out the browser-side contexts left by the previous connection
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
		// create_context does not go through run(); fatal handling (disconnect) is covered here itself
		match self.client()?.create_context(&rid.to_string()) {
			Ok(context) => {
				self.contexts.insert(rid, context.clone());
				Ok(context)
			}
			Err(e) => {
				if e.fatal {
					self.disconnect();
				}
				Err(e)
			}
		}
	}

	/// A fatal error disconnects: the client is dropped, the context bookkeeping moves to stale, and the next operation reconnects
	fn run<T>(&mut self, op: impl FnOnce(&mut dyn BrowserClient) -> Result<T, BrowserError>) -> Result<T, BrowserError> {
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

	pub fn eval(&mut self, rid: Rid, source: &str) -> Result<String, BrowserError> {
		let timeout = self.timeout;
		let context = self.context(rid)?;
		self.run(|client| client.eval(&context, source, timeout))
	}
}

mod trawl;

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
		/// Some(true) = fatal (disconnect), Some(false) = single-op failure
		pub load_error: Option<bool>,
		pub create_context_error: Option<bool>,
		pub dispose_error: Option<bool>,
		/// Capability bits of the simulated backend: the eval routing switch and absent cookie editing
		pub supports_eval: bool,
		pub cookie_edit_unsupported: bool,
		pub evals: Vec<(String, String)>,
		pub eval_value: Option<String>,
		pub eval_error: Option<bool>,
	}

	pub struct MockConnector(pub Arc<Mutex<MockState>>);

	impl BrowserConnector for MockConnector {
		fn connect(&self) -> Result<Box<dyn BrowserClient>, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			state.connects += 1;
			if state.fail_connect {
				return Err(BrowserError::fatal("mock connect failed"));
			}
			Ok(Box::new(MockClient(self.0.clone())))
		}

		fn supports_eval(&self) -> bool {
			self.0.lock().expect("mock lock").supports_eval
		}

		fn supports_cookie_edit(&self) -> bool {
			!self.0.lock().expect("mock lock").cookie_edit_unsupported
		}
	}

	struct MockClient(Arc<Mutex<MockState>>);

	impl BrowserClient for MockClient {
		fn create_context(&mut self, _label: &str) -> Result<String, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			if let Some(fatal) = state.create_context_error {
				return Err(if fatal {
					BrowserError::fatal("mock connection died")
				} else {
					BrowserError::transient("mock context rejected")
				});
			}
			state.next_context += 1;
			let id = format!("ctx-{}", state.next_context);
			state.contexts.push(id.clone());
			Ok(id)
		}

		fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			if let Some(fatal) = state.dispose_error {
				return Err(if fatal {
					BrowserError::fatal("mock connection died")
				} else {
					BrowserError::transient("mock dispose rejected")
				});
			}
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

		fn eval(&mut self, context: &str, source: &str, _timeout: Duration) -> Result<String, BrowserError> {
			let mut state = self.0.lock().expect("mock lock");
			if let Some(fatal) = state.eval_error {
				return Err(if fatal {
					BrowserError::fatal("mock connection died")
				} else {
					BrowserError::transient("mock eval failed")
				});
			}
			state.evals.push((context.to_string(), source.to_string()));
			Ok(state.eval_value.clone().unwrap_or_default())
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
	fn fatal_dispose_failure_marks_context_stale() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");

		mock.lock().expect("mock lock").dispose_error = Some(true);
		assert!(state.destroy(7).is_some(), "dispose 失败有日志行");

		mock.lock().expect("mock lock").dispose_error = None;
		state.load(8, "https://c.example/", &[]).expect("重连");
		assert!(
			mock.lock().expect("mock lock").disposed.contains(&"ctx-1".to_string()),
			"没到达浏览器的 dispose 重连时补上"
		);
	}

	#[test]
	fn fatal_create_context_disconnects() {
		let mock = Arc::new(Mutex::new(MockState::default()));
		let mut state = state_with(&mock);
		state.load(7, "https://a.example/", &[]).expect("load");

		mock.lock().expect("mock lock").create_context_error = Some(true);
		assert!(state.load(9, "https://b.example/", &[]).is_err(), "新 rid 建 context 撞断线");

		mock.lock().expect("mock lock").create_context_error = None;
		state.load(9, "https://b.example/", &[]).expect("重连后成功");
		let mock = mock.lock().expect("mock lock");
		assert_eq!(mock.connects, 2, "create_context 的 fatal 也要断连重连");
		assert!(mock.disposed.contains(&"ctx-1".to_string()), "旧 context 重连时回收");
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
		// Simulate the connection having died and been disconnected: destroy can only bookkeep; the dispose is made up on reconnect
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
		let state = BrowserState::unconfigured(Duration::from_secs(1));
		assert!(!state.configured());
		assert!(BrowserState::new_trawl("test", "http://trawl:8191", Duration::from_secs(1)).configured());
	}

	#[test]
	fn backend_capabilities_reported_per_backend() {
		let trawl = BrowserState::new_trawl("test", "http://trawl:8191", Duration::from_secs(1));
		assert!(trawl.supports_eval());
		assert!(!trawl.supports_cookie_edit(), "trawl 没有 cookie 编辑端点");
		let none = BrowserState::unconfigured(Duration::from_secs(1));
		assert!(!none.supports_eval() && !none.supports_cookie_edit());
	}
}
