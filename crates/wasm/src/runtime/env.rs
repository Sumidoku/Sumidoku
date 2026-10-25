use crate::runtime::limits::Limits;
use crate::runtime::rate_limit::RateLimiter;
use crate::runtime::store::{DefaultValue, GlobalStore};
use crate::traits::{DefaultsSink, HostLogger, NetPolicy, NetRoute};
use crate::{Error, Result};
use reqwest::blocking::Client;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

/// Consumer of partial results for an in-flight request: the input is the source-encoded
/// postcard payload. Hung on WasmEnv, set/cleared per call; actor serialization guarantees
/// only one call holds it at a time
pub type PartialSink = Box<dyn Fn(&[u8]) + Send + Sync>;

pub struct WasmEnv {
	pub memory: Option<wasmi::Memory>,
	pub limiter: wasmi::StoreLimits,
	pub store: GlobalStore,
	pub defaults: Defaults,
	pub net: NetState,
	pub browser: crate::browser::BrowserState,
	pub limits: Limits,
	pub source_key: String,
	pub logger: Arc<dyn HostLogger>,
	pub partial_sink: Option<PartialSink>,
}

impl WasmEnv {
	pub fn log(&self, message: &str) {
		self.logger.log(&self.source_key, message);
	}
}

/// The only interface between import function bodies and the engine: host state plus guest
/// linear-memory access. host()/mem_* calls inside a function body must not be held across
/// each other (&mut re-entry); take owned data and let go
pub trait Ctx {
	fn host(&mut self) -> &mut WasmEnv;
	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>>;
	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool;

	fn read_u32(&self, ptr: u32) -> Result<u32> {
		let bytes = self.mem_read(ptr, 4);
		Ok(bytes
			.and_then(|b| <[u8; 4]>::try_from(b.as_slice()).ok())
			.map(u32::from_le_bytes)
			.unwrap_or(0))
	}

	fn read_i32s(&self, ptr: u32, len: u32) -> Result<Vec<i32>> {
		if len == 0 {
			return Ok(Vec::new());
		}
		let bytes = self
			.mem_read(ptr, len * 4)
			.ok_or_else(|| Error::Instance("memory read out of bounds".into()))?;
		Ok(bytes.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect())
	}

	fn read_bytes(&self, ptr: u32, len: u32) -> Result<Vec<u8>> {
		if len == 0 {
			return Ok(Vec::new());
		}
		self.mem_read(ptr, len)
			.ok_or_else(|| Error::Instance("memory read out of bounds".into()))
	}

	fn read_item_bytes(&self, ptr: u32) -> Result<Vec<u8>> {
		let len = self.read_u32(ptr)?;
		if len < 8 {
			return Err(Error::Instance("invalid item header".into()));
		}
		self.read_bytes(ptr + 8, len - 8)
	}

	fn read_string(&self, ptr: u32, len: u32) -> Result<String> {
		let bytes = self.read_bytes(ptr, len)?;
		String::from_utf8(bytes).map_err(|_| Error::Utf8)
	}

	fn write_buffer(&mut self, ptr: u32, bytes: &[u8]) -> Result<()> {
		if self.mem_write(ptr, bytes) {
			Ok(())
		} else {
			Err(Error::Instance("memory write out of bounds".into()))
		}
	}

	fn write_i32s(&mut self, ptr: u32, values: &[i32]) -> Result<()> {
		let mut bytes = Vec::with_capacity(values.len() * 4);
		for v in values {
			bytes.extend_from_slice(&v.to_le_bytes());
		}
		self.write_buffer(ptr, &bytes)
	}
}

/// The wasmi-side Ctx implementation: the Caller's memory access and host-state access
/// time-share the same &mut. The memory handle is fetched fresh inside each function
/// body — WasmEnv.memory is still None before instantiation completes
pub(crate) struct WasmiCtx<'a, 'b> {
	caller: &'a mut wasmi::Caller<'b, WasmEnv>,
	memory: wasmi::Memory,
}

impl<'a, 'b> WasmiCtx<'a, 'b> {
	pub(crate) fn new(caller: &'a mut wasmi::Caller<'b, WasmEnv>) -> Self {
		let memory = caller
			.data()
			.memory
			.or_else(|| caller.get_export("memory").and_then(wasmi::Extern::into_memory))
			.expect("source exports memory");
		Self { caller, memory }
	}
}

impl Ctx for WasmiCtx<'_, '_> {
	fn host(&mut self) -> &mut WasmEnv {
		self.caller.data_mut()
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
		let data = self.memory.data(&*self.caller);
		let end = ptr.checked_add(len)? as usize;
		Some(data.get(ptr as usize..end)?.to_vec())
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
		let Some(end) = ptr.checked_add(bytes.len() as u32) else {
			return false;
		};
		let data = self.memory.data_mut(&mut *self.caller);
		let Some(slot) = data.get_mut(ptr as usize..end as usize) else {
			return false;
		};
		slot.copy_from_slice(bytes);
		true
	}
}

pub struct Defaults {
	map: HashMap<String, DefaultValue>,
	sink: Arc<dyn DefaultsSink>,
	source_key: String,
}

impl Defaults {
	pub fn new(
		map: HashMap<String, DefaultValue>,
		sink: Arc<dyn DefaultsSink>,
		source_key: String,
	) -> Self {
		Self { map, sink, source_key }
	}

	pub fn get(&self, key: &str) -> Option<&DefaultValue> {
		self.map.get(key)
	}

	pub fn set(&mut self, key: String, value: DefaultValue) {
		self.sink.set(&self.source_key, &key, &value);
		self.map.insert(key, value);
	}
}

pub struct NetState {
	pub policy: Arc<dyn NetPolicy>,
	pub clients: HashMap<NetRoute, Client>,
	pub jar: crate::imports::CookieJar,
	pub rate: RateLimiter,
	/// Accept-Language added when the source does not send one (derived from the source languages, see default_accept_language)
	pub accept_language: String,
	ca: Option<Vec<u8>>,
	/// trawl API base (net.browser_trawl): enables the login-session path and the
	/// clearance solver. None for sources without trawl configured
	trawl_api: Option<String>,
	/// Set while the source's basic_login() runs: the sequence's requests (warm-up GETs
	/// then the POST) are the only ones eligible for the trawl login session
	login_in_progress: Cell<bool>,
	/// host → trawl session id, established when a login request hits a CF challenge and
	/// escalates to the browser session; the rest of the sequence rides it
	login_sessions: RefCell<HashMap<String, String>>,
}

impl NetState {
	pub fn new(policy: Arc<dyn NetPolicy>, jar: crate::imports::CookieJar) -> Self {
		let ca = policy.ca_certificate();
		Self {
			policy,
			clients: HashMap::new(),
			jar,
			rate: RateLimiter::default(),
			accept_language: crate::traits::DEFAULT_ACCEPT_LANGUAGE.to_string(),
			ca,
			trawl_api: None,
			login_in_progress: Cell::new(false),
			login_sessions: RefCell::new(HashMap::new()),
		}
	}

	/// Attach the trawl API base so login sequences and clearance solves can use sessions.
	pub fn set_trawl(&mut self, api: &str) {
		self.trawl_api = Some(api.to_string());
	}

	pub fn trawl_api(&self) -> Option<&str> {
		self.trawl_api.as_deref()
	}

	/// Marks whether the source is currently inside basic_login(): requests issued while
	/// set may escalate to a trawl browser session on a CF challenge
	pub fn set_login_in_progress(&self, in_progress: bool) {
		self.login_in_progress.set(in_progress);
	}

	pub fn login_in_progress(&self) -> bool {
		self.login_in_progress.get()
	}

	pub fn login_session(&self, host: &str) -> Option<String> {
		self.login_sessions.borrow().get(host).cloned()
	}

	pub fn set_login_session(&self, host: String, session: String) {
		self.login_sessions.borrow_mut().insert(host, session);
	}

	pub fn client_for(&mut self, route: &NetRoute) -> Result<&Client> {
		if !self.clients.contains_key(route) {
			let mut builder = Client::builder().use_rustls_tls();
			if let NetRoute::Proxy(proxy_url) = route {
				let proxy = reqwest::Proxy::all(proxy_url)
					.map_err(|e| Error::Instance(format!("invalid proxy url: {e}")))?;
				builder = builder.proxy(proxy);
				if let Some(ca) = &self.ca {
					let cert = reqwest::Certificate::from_pem(ca)
						.map_err(|e| Error::Instance(format!("invalid trawl ca: {e}")))?;
					builder = builder.add_root_certificate(cert);
				}
			}
			let client = builder
				.build()
				.map_err(|e| Error::Instance(format!("failed to build http client: {e}")))?;
			self.clients.insert(route.clone(), client);
		}
		Ok(self.clients.get(route).expect("client just inserted"))
	}
}

#[cfg(test)]
pub(crate) mod test_ctx {
	use super::*;
	use crate::runtime::limits::Limits;
	use crate::runtime::store::GlobalStore;
	use crate::traits::{DirectPolicy, NoopDefaultsSink, StdoutLogger};
	use std::collections::HashMap;
	use std::sync::Arc;

	impl WasmEnv {
		pub(crate) fn for_test(limits: Limits, browser: crate::browser::BrowserState) -> Self {
			Self {
				memory: None,
				limiter: wasmi::StoreLimitsBuilder::new().build(),
				store: GlobalStore::new(),
				defaults: Defaults::new(HashMap::new(), Arc::new(NoopDefaultsSink), "test".into()),
				net: NetState::new(
					Arc::new(DirectPolicy { user_agent: None }),
					crate::imports::CookieJar::default(),
				),
				browser,
				limits,
				source_key: "test".into(),
				logger: Arc::new(StdoutLogger),
				partial_sink: None,
			}
		}
	}

	/// Engine-less Ctx implementation: a flat Vec acts as linear memory, so import unit
	/// tests drive function bodies directly. Pointer 0 is legal memory, so test data is
	/// bump-written from 1024 to avoid hitting the null-pointer path
	pub(crate) struct TestCtx {
		pub env: WasmEnv,
		memory: Vec<u8>,
		bump: u32,
	}

	impl TestCtx {
		pub(crate) fn new(env: WasmEnv) -> Self {
			Self {
				env,
				memory: vec![0; 4 * 64 * 1024],
				bump: 1024,
			}
		}

		pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> (u32, u32) {
			let ptr = self.bump;
			self.bump += bytes.len() as u32 + 64;
			assert!(self.mem_write(ptr, bytes), "测试写入越界");
			(ptr, bytes.len() as u32)
		}

		pub(crate) fn write_str(&mut self, s: &str) -> (u32, u32) {
			self.write_bytes(s.as_bytes())
		}
	}

	impl Ctx for TestCtx {
		fn host(&mut self) -> &mut WasmEnv {
			&mut self.env
		}

		fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
			let end = ptr.checked_add(len)? as usize;
			Some(self.memory.get(ptr as usize..end)?.to_vec())
		}

		fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
			let Some(end) = ptr.checked_add(bytes.len() as u32) else {
				return false;
			};
			let Some(slot) = self.memory.get_mut(ptr as usize..end as usize) else {
				return false;
			};
			slot.copy_from_slice(bytes);
			true
		}
	}
}
