use crate::error::{Error, Result};
use crate::features::SourceFeatures;
use crate::imports::HttpMethod;
use crate::runtime::env::WasmEnv;
use crate::runtime::limits::{Limits, LimitedTunables};
use crate::runtime::store::{DefaultValue, GlobalStore, Rid, StoreItem};
use crate::traits::{DefaultsSink, HostLogger, NetPolicy};
use aidoku::{
	Chapter, DeepLinkResult, Filter, FilterValue, HomeLayout, Listing, Manga, MangaPageResult, Page,
	PageContext, Setting,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wasmer::sys::{CompilerConfig, Cranelift, EngineBuilder};
use wasmer::{FunctionEnv, Instance, Module, Store, Value};
use wasmer_middlewares::Metering;
use wasmer_middlewares::metering::{MeteringPoints, get_remaining_points, set_remaining_points};

#[derive(Clone)]
pub struct SourceConfig {
	pub key: String,
	pub wasm_path: PathBuf,
	pub limits: Limits,
	pub defaults: HashMap<String, DefaultValue>,
	pub sink: Arc<dyn DefaultsSink>,
	pub policy: Arc<dyn NetPolicy>,
	pub logger: Arc<dyn HostLogger>,
}

impl SourceConfig {
	pub fn new(key: impl Into<String>, wasm_path: impl Into<PathBuf>) -> Self {
		Self {
			key: key.into(),
			wasm_path: wasm_path.into(),
			limits: Limits::default(),
			defaults: HashMap::new(),
			sink: Arc::new(crate::traits::NoopDefaultsSink),
			policy: Arc::new(crate::traits::DirectPolicy { user_agent: None }),
			logger: Arc::new(crate::traits::StdoutLogger),
		}
	}
}

/// get_image_request 的结果：源在宿主 store 里构造好的请求规格
#[derive(Debug, Clone)]
pub struct ImageRequestSpec {
	pub method: HttpMethod,
	pub url: String,
	pub headers: Vec<(String, String)>,
	pub body: Option<Vec<u8>>,
}

type CostFn = fn(&wasmer::wasmparser::Operator) -> u64;

fn operator_cost(_operator: &wasmer::wasmparser::Operator) -> u64 {
	1
}

pub(crate) struct SourceInstance {
	store: Store,
	instance: Instance,
	env: FunctionEnv<WasmEnv>,
	// 持有 Arc 让 middleware 与 engine 同生命周期
	_metering: Arc<Metering<CostFn>>,
	pub features: SourceFeatures,
	limits: Limits,
}

impl SourceInstance {
	pub fn build(config: &SourceConfig) -> Result<Self> {
		let wasm_bytes = crate::package::read_wasm_payload(&config.wasm_path)?;

		let metering: Arc<Metering<CostFn>> = Arc::new(Metering::new(config.limits.fuel, operator_cost as CostFn));
		let mut compiler = Cranelift::default();
		compiler.push_middleware(metering.clone());
		let mut engine = EngineBuilder::new(compiler).engine();
		engine.set_tunables(LimitedTunables::new(config.limits.memory_max));
		let mut store = Store::new(engine);

		let module = Module::new(&store, &wasm_bytes)
			.map_err(|e| Error::Instance(format!("compile module: {e}")))?;
		let features = SourceFeatures::from_module(&module);

		let env = FunctionEnv::new(
			&mut store,
			WasmEnv {
				memory: None,
				store: GlobalStore::new(),
				defaults: crate::runtime::env::Defaults::new(
					config.defaults.clone(),
					config.sink.clone(),
					config.key.clone(),
				),
				net: crate::runtime::env::NetState::new(config.policy.clone()),
				limits: config.limits.clone(),
				source_key: config.key.clone(),
				logger: config.logger.clone(),
			},
		);
		let imports = crate::imports::generate_imports(&mut store, &env);
		let instance = Instance::new(&mut store, &module, &imports)
			.map_err(|e| Error::Instance(format!("instantiate: {e}")))?;
		{
			let memory = instance
				.exports
				.get_memory("memory")
				.map_err(|e| Error::Instance(format!("source exports no memory: {e}")))?;
			env.as_mut(&mut store).memory = Some(memory.clone());
		}

		let mut source = Self {
			store,
			instance,
			env,
			_metering: metering,
			features,
			limits: config.limits.clone(),
		};
		source.call_unit_export("start")?;
		Ok(source)
	}

	fn call_raw(&mut self, name: &str, args: &[Value]) -> Result<i32> {
		set_remaining_points(&mut self.store, &self.instance, self.limits.fuel);
		let function = self
			.instance
			.exports
			.get_function(name)
			.map_err(|_| Error::Unsupported("missing export"))?
			.clone();
		match function.call(&mut self.store, args) {
			Ok(values) => Ok(values.first().and_then(|v| v.i32()).unwrap_or(0)),
			Err(e) => {
				if let MeteringPoints::Exhausted = get_remaining_points(&mut self.store, &self.instance) {
					Err(Error::Trap("out of fuel".into()))
				} else {
					Err(Error::Trap(e.to_string()))
				}
			}
		}
	}

	fn call_unit_export(&mut self, name: &str) -> Result<()> {
		self.call_raw(name, &[]).map(|_| ())
	}

	fn store_arg<T: Serialize>(&mut self, value: &T, transient: &mut Vec<Rid>) -> Result<Rid> {
		let rid = self
			.env
			.as_mut(&mut self.store)
			.store
			.store_encoded(value)
			.map_err(|e| Error::Instance(format!("encode argument: {e}")))?;
		transient.push(rid);
		Ok(rid)
	}

	// 仅用于宏里经 read_string 读取的参数（裸 UTF-8）；read::<String> 的参数必须走 store_arg
	fn store_string(&mut self, value: Option<&str>, transient: &mut Vec<Rid>) -> Rid {
		let rid = self
			.env
			.as_mut(&mut self.store)
			.store
			.store(StoreItem::String(value.unwrap_or_default().to_string()));
		transient.push(rid);
		rid
	}

	fn cleanup(&mut self, transient: Vec<Rid>) {
		let env = self.env.as_mut(&mut self.store);
		for rid in transient {
			env.store.remove(rid);
		}
	}

	fn read_result<T: DeserializeOwned>(&mut self, ret: i32) -> Result<T> {
		if ret < 0 {
			return Err(Error::from_code(ret));
		}
		let ptr = ret as u32;
		let head = self.env.as_ref(&self.store).read_u32(&self.store, ptr)?;
		let result = if head == u32::MAX {
			let real_len = self.env.as_ref(&self.store).read_u32(&self.store, ptr + 8)?;
			let msg = self
				.env
				.as_ref(&self.store)
				.read_bytes(&self.store, ptr + 12, real_len.saturating_sub(12))?;
			Err(Error::Message(String::from_utf8_lossy(&msg).into_owned()))
		} else {
			if head < 8 {
				return Err(Error::Instance("invalid result header".into()));
			}
			let payload = self
				.env
				.as_ref(&self.store)
				.read_bytes(&self.store, ptr + 8, head - 8)?;
			postcard::from_bytes(&payload).map_err(|_| Error::Deserialize)
		};
		self.free_result(ret);
		result
	}

	fn free_result(&mut self, ptr: i32) {
		if let Ok(free) = self
			.instance
			.exports
			.get_typed_function::<i32, ()>(&self.store, "free_result")
			&& let Err(e) = free.call(&mut self.store, ptr)
		{
			self.env.as_ref(&self.store).log(&format!("free_result failed: {e}"));
		}
	}

	fn check(&self, supported: bool, name: &'static str) -> Result<()> {
		if supported { Ok(()) } else { Err(Error::Unsupported(name)) }
	}

	pub fn get_search_manga_list(
		&mut self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut t = Vec::new();
		let query_rid = self.store_string(query.as_deref(), &mut t);
		let filters_rid = self.store_arg(&filters, &mut t)?;
		let ret = self.call_raw(
			"get_search_manga_list",
			&[query_rid.into(), page.into(), filters_rid.into()],
		);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_manga_update(
		&mut self,
		manga: &Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let mut t = Vec::new();
		let manga_rid = self.store_arg(manga, &mut t)?;
		let ret = self.call_raw(
			"get_manga_update",
			&[
				manga_rid.into(),
				(needs_details as i32).into(),
				(needs_chapters as i32).into(),
			],
		);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_page_list(&mut self, manga: &Manga, chapter: &Chapter) -> Result<Vec<Page>> {
		let mut t = Vec::new();
		let manga_rid = self.store_arg(manga, &mut t)?;
		let chapter_rid = self.store_arg(chapter, &mut t)?;
		let ret = self.call_raw("get_page_list", &[manga_rid.into(), chapter_rid.into()]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_manga_list(&mut self, listing: &Listing, page: i32) -> Result<MangaPageResult> {
		self.check(self.features.manga_list, "get_manga_list")?;
		let mut t = Vec::new();
		let listing_rid = self.store_arg(listing, &mut t)?;
		let ret = self.call_raw("get_manga_list", &[listing_rid.into(), page.into()]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_home(&mut self) -> Result<HomeLayout> {
		self.check(self.features.home, "get_home")?;
		let ret = self.call_raw("get_home", &[]);
		ret.and_then(|r| self.read_result(r))
	}

	pub fn get_filters(&mut self) -> Result<Vec<Filter>> {
		self.check(self.features.dynamic_filters, "get_filters")?;
		let ret = self.call_raw("get_filters", &[]);
		ret.and_then(|r| self.read_result(r))
	}

	pub fn get_settings(&mut self) -> Result<Vec<Setting>> {
		self.check(self.features.dynamic_settings, "get_settings")?;
		let ret = self.call_raw("get_settings", &[]);
		ret.and_then(|r| self.read_result(r))
	}

	pub fn get_listings(&mut self) -> Result<Vec<Listing>> {
		self.check(self.features.dynamic_listings, "get_listings")?;
		let ret = self.call_raw("get_listings", &[]);
		ret.and_then(|r| self.read_result(r))
	}

	pub fn get_page_description(&mut self, page: &Page) -> Result<String> {
		self.check(self.features.page_description, "get_page_description")?;
		let mut t = Vec::new();
		let page_rid = self.store_arg(page, &mut t)?;
		let ret = self.call_raw("get_page_description", &[page_rid.into()]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_alternate_covers(&mut self, manga: &Manga) -> Result<Vec<String>> {
		self.check(self.features.alternate_covers, "get_alternate_covers")?;
		let mut t = Vec::new();
		let manga_rid = self.store_arg(manga, &mut t)?;
		let ret = self.call_raw("get_alternate_covers", &[manga_rid.into()]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_base_url(&mut self) -> Result<String> {
		self.check(self.features.base_url, "get_base_url")?;
		let ret = self.call_raw("get_base_url", &[]);
		ret.and_then(|r| self.read_result(r))
	}

	pub fn handle_notification(&mut self, key: &str) -> Result<()> {
		self.check(self.features.notification, "handle_notification")?;
		let mut t = Vec::new();
		let key_rid = self.store_arg(&key, &mut t)?;
		let ret = self.call_raw("handle_notification", &[key_rid.into()]);
		let result = ret.map(|_| ());
		self.cleanup(t);
		result
	}

	pub fn handle_deep_link(&mut self, url: &str) -> Result<Option<DeepLinkResult>> {
		self.check(self.features.deep_link, "handle_deep_link")?;
		let mut t = Vec::new();
		let url_rid = self.store_arg(&url, &mut t)?;
		let ret = self.call_raw("handle_deep_link", &[url_rid.into()]);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn handle_key_migration(
		&mut self,
		kind: i32,
		manga_key: &str,
		chapter_key: Option<&str>,
	) -> Result<String> {
		self.check(self.features.migration, "handle_key_migration")?;
		let mut t = Vec::new();
		let manga_rid = self.store_arg(&manga_key, &mut t)?;
		let chapter_rid = self.store_arg(&chapter_key.unwrap_or_default(), &mut t)?;
		let ret = self.call_raw(
			"handle_key_migration",
			&[kind.into(), manga_rid.into(), chapter_rid.into()],
		);
		let result = ret.and_then(|r| self.read_result(r));
		self.cleanup(t);
		result
	}

	pub fn get_image_request(
		&mut self,
		url: &str,
		context: Option<&PageContext>,
	) -> Result<ImageRequestSpec> {
		self.check(self.features.image_request, "get_image_request")?;
		let mut t = Vec::new();
		let url_rid = self.store_arg(&url, &mut t)?;
		let context_rid = match context {
			Some(ctx) => self.store_arg(ctx, &mut t)?,
			None => -1,
		};
		let ret = self.call_raw("get_image_request", &[url_rid.into(), context_rid.into()]);
		let result = ret.and_then(|r| self.read_result::<i32>(r)).and_then(|rid| {
			let env = self.env.as_mut(&mut self.store);
			let request = env
				.store
				.get_mut(rid)
				.and_then(|item| item.as_request())
				.ok_or(Error::Instance("get_image_request returned invalid rid".into()))?;
			let spec = ImageRequestSpec {
				method: request.method,
				url: request
					.url
					.as_ref()
					.map(|u| u.to_string())
					.ok_or(Error::Instance("image request missing url".into()))?,
				headers: request
					.headers
					.iter()
					.map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
					.collect(),
				body: request.body.clone(),
			};
			env.store.remove(rid);
			Ok(spec)
		});
		self.cleanup(t);
		result
	}
}

