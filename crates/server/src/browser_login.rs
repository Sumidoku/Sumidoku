//! Sidecar 交互登录的会话管理：一次「在浏览器里登录」从开会话到收割 cookie 之间，
//! 浏览器 context 由服务端钉住（随机句柄 + TTL reaper），帧经 watch 通道流向 WS，
//! 用户输入经 WS 回灌 CDP。会话是内存态：server 重启即丢，浏览器侧 context 随
//! sidecar 自身回收（browserless 进程级回收），不落库。

use crate::error::{ApiError, ApiResult};
use aidoku_wasm::browser::{CdpClient, CdpConnector};
use rand_core::{OsRng, RngCore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// 会话闲置上限：交互登录是人肉操作（验证码/2FA），10 分钟足够；
/// sidecar 的 CONNECTION_TIMEOUT 必须不小于它（docs/sidecars.md 的部署口径）
pub const SESSION_TTL: Duration = Duration::from_secs(600);

/// 初始导航等 load 事件的上限；超时按单次失败处理（页面继续加载，不杀会话）
const NAVIGATE_TIMEOUT: Duration = Duration::from_secs(20);

/// 交互视图的远端视口：webui 按它换算输入坐标
pub const VIEWPORT_WIDTH: i64 = 1280;
pub const VIEWPORT_HEIGHT: i64 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
	Timeout,
	Completed,
	Cancelled,
}

impl SessionEnd {
	pub fn reason(self) -> &'static str {
		match self {
			Self::Timeout => "timeout",
			Self::Completed => "completed",
			Self::Cancelled => "cancelled",
		}
	}
}

struct LoginSession {
	source_id: i64,
	key: String,
	host: String,
	client: Arc<Mutex<Box<dyn CdpClient>>>,
	context: String,
	frames_tx: watch::Sender<String>,
	end_tx: watch::Sender<Option<SessionEnd>>,
	// 会话存续期间必须各保一个接收者：watch::Sender::send 在零接收者时失败且不存值，
	// CDP 泵会因此自停，discard/reaper 的结束信号也会丢
	_frames_rx: watch::Receiver<String>,
	_end_rx: watch::Receiver<Option<SessionEnd>>,
	reaper: tokio::task::AbortHandle,
}

/// WS 与 complete/cancel 共享的会话引用（克隆成本低，全部 Arc/Receiver）
pub struct SessionRef {
	pub source_id: i64,
	pub key: String,
	pub host: String,
	client: Arc<Mutex<Box<dyn CdpClient>>>,
	context: String,
	end_tx: watch::Sender<Option<SessionEnd>>,
	pub frames: watch::Receiver<String>,
	pub ends: watch::Receiver<Option<SessionEnd>>,
}

impl SessionRef {
	/// CDP 调用是阻塞签名（内部 block_on），统一在 blocking 线程池执行
	pub async fn call<T: Send + 'static>(
		&self,
		op: impl FnOnce(&mut dyn CdpClient, &str) -> Result<T, aidoku_wasm::browser::BrowserError> + Send + 'static,
	) -> ApiResult<T> {
		let client = Arc::clone(&self.client);
		let context = self.context.clone();
		tokio::task::spawn_blocking(move || {
			let mut guard = client.lock().expect("cdp client lock");
			op(guard.as_mut(), &context)
		})
		.await
		.map_err(|e| ApiError::internal(format!("browser task join: {e}")))?
		.map_err(|e| ApiError::bad_gateway(format!("browser session: {e}")))
	}
}

pub struct BrowserLoginManager {
	connector: Option<Arc<dyn CdpConnector>>,
	sessions: Arc<Mutex<HashMap<String, LoginSession>>>,
	ttl: Duration,
}

impl BrowserLoginManager {
	pub fn new(connector: Option<Box<dyn CdpConnector>>, ttl: Duration) -> Self {
		Self {
			connector: connector.map(Arc::from),
			sessions: Arc::new(Mutex::new(HashMap::new())),
			ttl,
		}
	}

	pub fn configured(&self) -> bool {
		self.connector.is_some()
	}

	pub fn ttl(&self) -> Duration {
		self.ttl
	}

	/// 开会话：连接 sidecar、开隔离 context、定视口、导航登录页、起推流。
	/// 导航超时/失败不杀会话（页面还在加载，用户可在视图里等），fatal 才报错
	pub async fn start(&self, source_id: i64, key: &str, host: &str, login_url: &str) -> ApiResult<String> {
		let connector = self
			.connector
			.as_ref()
			.ok_or_else(|| ApiError::unsupported_501("browser sidecar not configured"))?
			.clone();
		let login_url = login_url.to_string();
		let (frames_tx, frames_rx) = watch::channel(String::new());
		let (end_tx, end_rx) = watch::channel(None);
		let sink = frames_tx.clone();
		let (client, context) = tokio::task::spawn_blocking(move || {
			let mut client = connector
				.connect()
				.map_err(|e| ApiError::bad_gateway(format!("browser connect failed: {e}")))?;
			let context = client
				.create_context("login")
				.map_err(|e| ApiError::bad_gateway(format!("browser context create failed: {e}")))?;
			if let Err(e) = client.set_viewport(&context, VIEWPORT_WIDTH, VIEWPORT_HEIGHT) {
				let _ = client.dispose_context(&context);
				return Err(ApiError::bad_gateway(format!("browser viewport failed: {e}")));
			}
			// 单次失败（导航超时等）保留会话：页面仍在加载，强杀会抢走正在进行的登录
			if let Err(e) = client.load(&context, &login_url, &[], NAVIGATE_TIMEOUT)
				&& e.fatal
			{
				let _ = client.dispose_context(&context);
				return Err(ApiError::bad_gateway(format!("browser navigation failed: {e}")));
			}
			if let Err(e) = client.screencast_start(&context, sink) {
				let _ = client.dispose_context(&context);
				return Err(ApiError::bad_gateway(format!("browser screencast failed: {e}")));
			}
			Ok((client, context))
		})
		.await
		.map_err(|e| ApiError::internal(format!("browser task join: {e}")))??;

		let id = new_session_id();
		let session = LoginSession {
			source_id,
			key: key.to_string(),
			host: host.to_string(),
			client: Arc::new(Mutex::new(client)),
			context,
			frames_tx,
			end_tx,
			_frames_rx: frames_rx,
			_end_rx: end_rx,
			reaper: self.spawn_reaper(&id),
		};
		self.sessions.lock().expect("sessions lock").insert(id.clone(), session);
		Ok(id)
	}

	fn spawn_reaper(&self, id: &str) -> tokio::task::AbortHandle {
		let sessions = Arc::clone(&self.sessions);
		let id = id.to_string();
		let ttl = self.ttl;
		let handle = tokio::spawn(async move {
			tokio::time::sleep(ttl).await;
			let session = sessions.lock().expect("sessions lock").remove(&id);
			if let Some(session) = session {
				dispose(session).await;
			}
		});
		handle.abort_handle()
	}

	fn reference(session: &LoginSession) -> SessionRef {
		SessionRef {
			source_id: session.source_id,
			key: session.key.clone(),
			host: session.host.clone(),
			client: Arc::clone(&session.client),
			context: session.context.clone(),
			end_tx: session.end_tx.clone(),
			frames: session.frames_tx.subscribe(),
			ends: session.end_tx.subscribe(),
		}
	}

	pub fn session(&self, id: &str) -> Option<SessionRef> {
		let sessions = self.sessions.lock().expect("sessions lock");
		sessions.get(id).map(Self::reference)
	}

	/// 取走会话（complete/cancel 的一次性语义）：停 reaper 但不 dispose——
	/// complete 还要从这个 context 收割 cookie，dispose 是 discard 的职责
	pub fn take(&self, id: &str) -> Option<SessionRef> {
		let session = self.sessions.lock().expect("sessions lock").remove(id)?;
		session.reaper.abort();
		Some(Self::reference(&session))
	}

	/// 终结已取走的会话：发结束信号、dispose context（尽力而为）
	pub async fn discard(&self, reference: SessionRef, end: SessionEnd) {
		let _ = reference.end_tx.send(Some(end));
		dispose_parts(reference.client, reference.context).await;
	}

	#[cfg(test)]
	pub fn session_count(&self) -> usize {
		self.sessions.lock().expect("sessions lock").len()
	}
}

/// reaper 的会话终结：发超时信号后 dispose
async fn dispose(session: LoginSession) {
	let _ = session.end_tx.send(Some(SessionEnd::Timeout));
	dispose_parts(session.client, session.context).await;
}

/// dispose 尽力而为：连接已死时 context 由 sidecar 进程级回收
async fn dispose_parts(client: Arc<Mutex<Box<dyn CdpClient>>>, context: String) {
	tokio::task::spawn_blocking(move || {
		let mut guard = client.lock().expect("cdp client lock");
		let _ = guard.screencast_stop(&context);
		let _ = guard.dispose_context(&context);
	})
	.await
	.ok();
}

fn new_session_id() -> String {
	let mut raw = [0u8; 16];
	OsRng.fill_bytes(&mut raw);
	raw.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_wasm::browser::{BrowserCookie, BrowserError, CdpConnector, FrameSink, KeyInput, MouseInput};
	use std::sync::Mutex as StdMutex;

	// aidoku_wasm 的 mock 是 cfg(test) 不外露，这里按测试关注点写一个最小件
	#[derive(Default)]
	struct FakeState {
		contexts: Vec<String>,
		disposed: Vec<String>,
		cookies: HashMap<String, Vec<BrowserCookie>>,
	}

	struct FakeConnector(Arc<StdMutex<FakeState>>);

	impl CdpConnector for FakeConnector {
		fn connect(&self) -> Result<Box<dyn CdpClient>, BrowserError> {
			Ok(Box::new(FakeClient(self.0.clone())))
		}
	}

	struct FakeClient(Arc<StdMutex<FakeState>>);

	impl CdpClient for FakeClient {
		fn create_context(&mut self, _label: &str) -> Result<String, BrowserError> {
			let mut state = self.0.lock().expect("lock");
			let id = format!("ctx-{}", state.contexts.len() + 1);
			state.contexts.push(id.clone());
			Ok(id)
		}
		fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
			self.0.lock().expect("lock").disposed.push(context.to_string());
			Ok(())
		}
		fn load(&mut self, _context: &str, _url: &str, _headers: &[(String, String)], _timeout: Duration) -> Result<Vec<BrowserCookie>, BrowserError> {
			Ok(Vec::new())
		}
		fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError> {
			Ok(self.0.lock().expect("lock").cookies.get(context).cloned().unwrap_or_default())
		}
		fn delete_cookie(&mut self, _context: &str, _name: &str, _domain: Option<&str>, _path: Option<&str>) -> Result<(), BrowserError> {
			Ok(())
		}
		fn clear_cookies(&mut self, _context: &str) -> Result<(), BrowserError> {
			Ok(())
		}
		fn set_viewport(&mut self, _context: &str, _width: i64, _height: i64) -> Result<(), BrowserError> {
			Ok(())
		}
		fn screencast_start(&mut self, _context: &str, _sink: FrameSink) -> Result<(), BrowserError> {
			Ok(())
		}
		fn screencast_stop(&mut self, _context: &str) -> Result<(), BrowserError> {
			Ok(())
		}
		fn dispatch_mouse(&mut self, _context: &str, _input: &MouseInput) -> Result<(), BrowserError> {
			Ok(())
		}
		fn dispatch_key(&mut self, _context: &str, _input: &KeyInput) -> Result<(), BrowserError> {
			Ok(())
		}
		fn insert_text(&mut self, _context: &str, _text: &str) -> Result<(), BrowserError> {
			Ok(())
		}
		fn local_storage_values(&mut self, _context: &str, _keys: &[String]) -> Result<Vec<(String, String)>, BrowserError> {
			Ok(Vec::new())
		}
	}

	fn manager_with(state: &Arc<StdMutex<FakeState>>, ttl: Duration) -> BrowserLoginManager {
		BrowserLoginManager::new(Some(Box::new(FakeConnector(state.clone()))), ttl)
	}

	#[tokio::test]
	async fn unconfigured_manager_reports_unsupported() {
		let manager = BrowserLoginManager::new(None, Duration::from_secs(1));
		assert!(!manager.configured());
		let err = manager.start(1, "login", "h", "https://h/").await.unwrap_err();
		assert_eq!(err.code(), "unsupported");
	}

	#[tokio::test]
	async fn start_then_reaper_reaps_after_ttl() {
		let state = Arc::new(StdMutex::new(FakeState::default()));
		let manager = manager_with(&state, Duration::from_millis(50));
		manager.start(1, "login", "h", "https://h/").await.expect("start");
		assert_eq!(manager.session_count(), 1);
		// CI 满载时 tokio 任务唤醒可能远迟于 TTL 的几倍：轮询等结果，不做固定时长睡眠
		let mut reaped = false;
		for _ in 0..50 {
			if manager.session_count() == 0 {
				reaped = true;
				break;
			}
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
		assert!(reaped, "reaper did not reap within 5s");
		assert_eq!(state.lock().expect("lock").disposed, vec!["ctx-1"]);
	}

	#[tokio::test]
	async fn take_then_discard_disposes_context_once() {
		let state = Arc::new(StdMutex::new(FakeState::default()));
		let manager = manager_with(&state, Duration::from_secs(60));
		let id = manager.start(1, "login", "h", "https://h/").await.expect("start");
		let reference = manager.take(&id).expect("take");
		assert_eq!(reference.host, "h");
		manager.discard(reference, SessionEnd::Completed).await;
		assert!(manager.take(&id).is_none());
		assert_eq!(state.lock().expect("lock").disposed, vec!["ctx-1"]);
		assert_eq!(manager.session_count(), 0);
	}

	#[tokio::test]
	async fn session_call_runs_cdp_ops_against_the_context() {
		let state = Arc::new(StdMutex::new(FakeState::default()));
		state.lock().expect("lock").cookies.insert(
			"ctx-1".into(),
			vec![BrowserCookie {
				name: "session".into(),
				value: "s1".into(),
				domain: ".login.example.com".into(),
				path: "/".into(),
				expires: None,
				secure: false,
				http_only: false,
			}],
		);
		let manager = manager_with(&state, Duration::from_secs(60));
		let id = manager.start(1, "login", "h", "https://h/").await.expect("start");
		let reference = manager.session(&id).expect("session");
		let cookies = reference.call(|client, context| client.cookies(context)).await.expect("cookies");
		assert_eq!(cookies.len(), 1);
		assert_eq!(cookies[0].name, "session");
	}
}
