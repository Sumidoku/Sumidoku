use crate::limits::Limits;
use crate::rate_limit::RateLimiter;
use crate::store::{DefaultValue, GlobalStore};
use crate::traits::{DefaultsSink, HostLogger, NetPolicy, NetRoute};
use crate::{Error, Result};
use reqwest::blocking::Client;
use std::collections::HashMap;
use std::sync::Arc;
use wasmer::{Memory, MemoryView, WasmPtr};

pub struct WasmEnv {
	pub memory: Option<Memory>,
	pub store: GlobalStore,
	pub defaults: Defaults,
	pub net: NetState,
	pub limits: Limits,
	pub source_key: String,
	pub logger: Arc<dyn HostLogger>,
}

impl WasmEnv {
	pub fn log(&self, message: &str) {
		self.logger.log(&self.source_key, message);
	}

	pub fn read_u32(&self, store: &(impl wasmer::AsStoreRef + ?Sized), ptr: u32) -> Result<u32> {
		let memory_view = self.memory_view(store)?;
		let ptr: WasmPtr<u32> = WasmPtr::new(ptr);
		Ok(ptr.read(&memory_view).unwrap_or(0))
	}

	pub fn read_values<T: wasmer::ValueType>(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		len: u32,
	) -> Result<Vec<T>> {
		if len == 0 {
			return Ok(Vec::new());
		}
		let memory_view = self.memory_view(store)?;
		let ptr: WasmPtr<T> = WasmPtr::new(ptr);
		let mut data = Vec::new();
		let slice = ptr.slice(&memory_view, len).map_err(|e| Error::Instance(e.to_string()))?;
		for cell in slice.iter() {
			data.push(cell.read().map_err(|e| Error::Instance(e.to_string()))?);
		}
		Ok(data)
	}

	pub fn read_bytes(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		len: u32,
	) -> Result<Vec<u8>> {
		if len == 0 {
			return Ok(Vec::new());
		}
		let memory_view = self.memory_view(store)?;
		let mut data = vec![0u8; len as usize];
		memory_view
			.read(ptr as u64, &mut data)
			.map_err(|e| Error::Instance(e.to_string()))?;
		Ok(data)
	}

	pub fn read_item_bytes(&self, store: &(impl wasmer::AsStoreRef + ?Sized), ptr: u32) -> Result<Vec<u8>> {
		let len = self.read_u32(store, ptr)?;
		if len < 8 {
			return Err(Error::Instance("invalid item header".into()));
		}
		self.read_bytes(store, ptr + 8, len - 8)
	}

	pub fn read_string(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		len: u32,
	) -> Result<String> {
		let bytes = self.read_bytes(store, ptr, len)?;
		String::from_utf8(bytes).map_err(|_| Error::Utf8)
	}

	pub fn write_buffer(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		bytes: &[u8],
	) -> Result<()> {
		let memory_view = self.memory_view(store)?;
		memory_view
			.write(ptr as u64, bytes)
			.map_err(|e| Error::Instance(e.to_string()))
	}

	pub fn write_string(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		s: &str,
	) -> Result<()> {
		self.write_buffer(store, ptr, s.as_bytes())
	}

	pub fn write_values<T: wasmer::ValueType>(
		&self,
		store: &(impl wasmer::AsStoreRef + ?Sized),
		ptr: u32,
		values: Vec<T>,
	) -> Result<()> {
		let memory_view = self.memory_view(store)?;
		let ptr: WasmPtr<T> = WasmPtr::new(ptr);
		let slice = ptr
			.slice(&memory_view, values.len() as u32)
			.map_err(|e| Error::Instance(e.to_string()))?;
		for (idx, val) in values.into_iter().enumerate() {
			slice.write(idx as u64, val).map_err(|e| Error::Instance(e.to_string()))?;
		}
		Ok(())
	}

	fn memory_view<'a>(
		&self,
		store: &'a (impl wasmer::AsStoreRef + ?Sized),
	) -> Result<MemoryView<'a>> {
		let memory = self
			.memory
			.as_ref()
			.ok_or_else(|| Error::Instance("memory not initialized".into()))?;
		Ok(memory.view(store))
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
	ca: Option<Vec<u8>>,
}

impl NetState {
	pub fn new(policy: Arc<dyn NetPolicy>) -> Self {
		let ca = policy.ca_certificate();
		Self {
			policy,
			clients: HashMap::new(),
			jar: crate::imports::CookieJar::default(),
			rate: RateLimiter::default(),
			ca,
		}
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
