use crate::error::{Error, Result};
use crate::features::SourceFeatures;
use crate::imports::HttpMethod;
use crate::runtime::env::{Ctx, WasmEnv};
use crate::runtime::limits::Limits;
use crate::runtime::store::{DefaultValue, GlobalStore, Rid, StoreItem};
use crate::traits::{CookieSink, DefaultsSink, HostLogger, NetPolicy};
use aidoku::{
	Chapter, DeepLinkResult, Filter, FilterValue, HomeLayout, Listing, Manga, MangaPageResult, Page,
	PageContext, Setting,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wasmi::{Instance, Store, TrapCode, Val};

#[derive(Clone)]
pub struct SourceConfig {
	pub key: String,
	pub wasm_path: PathBuf,
	pub limits: Limits,
	pub defaults: HashMap<String, DefaultValue>,
	pub sink: Arc<dyn DefaultsSink>,
	/// Persisted cookies: domain → (name → value); the jar is backfilled from these when an instance starts
	pub cookies: HashMap<String, HashMap<String, String>>,
	pub cookie_sink: Arc<dyn CookieSink>,
	pub policy: Arc<dyn NetPolicy>,
	pub logger: Arc<dyn HostLogger>,
	/// Default Accept-Language derived from the source languages; the net host adds it when the source does not send one
	pub accept_language: String,
	/// trawl session API address (http://…); when None, the webview browser functions are explicitly unsupported
	pub browser_trawl: Option<String>,
}

impl SourceConfig {
	pub fn new(key: impl Into<String>, wasm_path: impl Into<PathBuf>) -> Self {
		Self {
			key: key.into(),
			wasm_path: wasm_path.into(),
			limits: Limits::default(),
			defaults: HashMap::new(),
			sink: Arc::new(crate::traits::NoopDefaultsSink),
			cookies: HashMap::new(),
			cookie_sink: Arc::new(crate::traits::NoopCookieSink),
			policy: Arc::new(crate::traits::DirectPolicy { user_agent: None }),
			logger: Arc::new(crate::traits::StdoutLogger),
			accept_language: crate::traits::DEFAULT_ACCEPT_LANGUAGE.to_string(),
			browser_trawl: None,
		}
	}
}

/// Result of get_image_request: the request spec the source built in the host store
#[derive(Debug, Clone)]
pub struct ImageRequestSpec {
	pub method: HttpMethod,
	pub url: String,
	pub headers: Vec<(String, String)>,
	pub body: Option<Vec<u8>>,
}

/// Input of process_*_image: the fetched image bytes plus the request context needed to build the ImageResponse
#[derive(Debug, Clone)]
pub struct ProcessImageInput {
	pub bytes: Vec<u8>,
	pub url: String,
	pub status: u16,
	pub response_headers: Vec<(String, String)>,
	pub request_headers: Vec<(String, String)>,
}

pub(crate) struct SourceInstance {
	store: Store<WasmEnv>,
	instance: Instance,
	memory: wasmi::Memory,
	pub features: SourceFeatures,
	limits: Limits,
}

/// Channel for the host side to read/write guest memory and the store around an export call (read_result/store_arg)
impl Ctx for SourceInstance {
	fn host(&mut self) -> &mut WasmEnv {
		self.store.data_mut()
	}

	fn mem_read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
		let data = self.memory.data(&self.store);
		let end = ptr.checked_add(len)? as usize;
		Some(data.get(ptr as usize..end)?.to_vec())
	}

	fn mem_write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
		let Some(end) = ptr.checked_add(bytes.len() as u32) else {
			return false;
		};
		let data = self.memory.data_mut(&mut self.store);
		let Some(slot) = data.get_mut(ptr as usize..end as usize) else {
			return false;
		};
		slot.copy_from_slice(bytes);
		true
	}
}

impl SourceInstance {
	pub fn build(config: &SourceConfig) -> Result<Self> {
		let wasm_bytes = crate::package::read_wasm_payload(&config.wasm_path)?;

		let mut engine_config = wasmi::Config::default();
		engine_config.consume_fuel(true);
		let engine = wasmi::Engine::new(&engine_config);
		let module = wasmi::Module::new(&engine, &wasm_bytes)
			.map_err(|e| Error::Instance(format!("translate module: {e}")))?;
		let features = SourceFeatures::from_module(&module);

		let mut net = crate::runtime::env::NetState::new(
			config.policy.clone(),
			crate::imports::CookieJar::new(config.cookies.clone(), config.cookie_sink.clone()),
		);
		net.accept_language = config.accept_language.clone();

		let memory_max = usize::try_from(config.limits.memory_max)
			.map_err(|_| Error::Instance("memory_max does not fit usize".into()))?;
		let env = WasmEnv {
			memory: None,
			limiter: wasmi::StoreLimitsBuilder::new().memory_size(memory_max).build(),
			store: GlobalStore::new(),
			defaults: crate::runtime::env::Defaults::new(
				config.defaults.clone(),
				config.sink.clone(),
				config.key.clone(),
			),
			net,
			browser: match &config.browser_trawl {
				Some(api) => crate::browser::BrowserState::new_trawl(
					&config.key,
					api,
					config.limits.browser_timeout,
				),
				None => crate::browser::BrowserState::unconfigured(config.limits.browser_timeout),
			},
			limits: config.limits.clone(),
			source_key: config.key.clone(),
			logger: config.logger.clone(),
			partial_sink: None,
		};
		let mut store = Store::new(&engine, env);
		store
			.set_fuel(config.limits.fuel)
			.map_err(|e| Error::Instance(format!("set fuel: {e}")))?;
		store.limiter(|data| &mut data.limiter);

		let mut linker = wasmi::Linker::new(&engine);
		crate::imports::register_imports(&mut linker)
			.map_err(|e| Error::Instance(format!("link imports: {e}")))?;
		let instance = linker
			.instantiate_and_start(&mut store, &module)
			.map_err(|e| Error::Instance(format!("instantiate: {e}")))?;
		let memory = instance
			.get_memory(&store, "memory")
			.ok_or_else(|| Error::Instance("source exports no memory".into()))?;
		store.data_mut().memory = Some(memory);

		let mut source = Self {
			store,
			instance,
			memory,
			features,
			limits: config.limits.clone(),
		};
		source.call_unit_export("start")?;
		Ok(source)
	}

	fn call_raw(&mut self, name: &str, args: &[Val]) -> Result<i32> {
		self.store
			.set_fuel(self.limits.fuel)
			.map_err(|e| Error::Instance(format!("set fuel: {e}")))?;
		let function = self
			.instance
			.get_func(&self.store, name)
			.ok_or(Error::Unsupported("missing export"))?;
		let mut results = vec![Val::I32(0); function.ty(&self.store).results().len()];
		match function.call(&mut self.store, args, &mut results) {
			Ok(()) => Ok(results.first().and_then(|v| v.i32()).unwrap_or(0)),
			Err(e) => {
				if e.as_trap_code() == Some(TrapCode::OutOfFuel) {
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
			.store
			.data_mut()
			.store
			.store_encoded(value)
			.map_err(|e| Error::Instance(format!("encode argument: {e}")))?;
		transient.push(rid);
		Ok(rid)
	}

	// Only for parameters the macro reads via read_string (raw UTF-8); parameters of read::<String> must go through store_arg
	fn store_string(&mut self, value: Option<&str>, transient: &mut Vec<Rid>) -> Rid {
		let rid = self
			.store
			.data_mut()
			.store
			.store(StoreItem::String(value.unwrap_or_default().to_string()));
		transient.push(rid);
		rid
	}

	fn cleanup(&mut self, transient: Vec<Rid>) {
		let host = self.store.data_mut();
		for rid in transient {
			host.store.remove(rid);
		}
	}

	fn read_result<T: DeserializeOwned>(&mut self, ret: i32) -> Result<T> {
		if ret < 0 {
			return Err(Error::from_code(ret));
		}
		let ptr = ret as u32;
		let head = self.read_u32(ptr)?;
		let result = if head == u32::MAX {
			let real_len = self.read_u32(ptr + 8)?;
			let msg = self.read_bytes(ptr + 12, real_len.saturating_sub(12))?;
			Err(Error::Message(String::from_utf8_lossy(&msg).into_owned()))
		} else {
			if head < 8 {
				return Err(Error::Instance("invalid result header".into()));
			}
			let payload = self.read_bytes(ptr + 8, head - 8)?;
			postcard::from_bytes(&payload).map_err(|_| Error::Deserialize)
		};
		self.free_result(ret);
		result
	}

	fn free_result(&mut self, ptr: i32) {
		if let Ok(free) = self.instance.get_typed_func::<i32, ()>(&self.store, "free_result")
			&& let Err(e) = free.call(&mut self.store, ptr)
		{
			self.host().log(&format!("free_result failed: {e}"));
		}
	}

	fn check(&self, supported: bool, name: &'static str) -> Result<()> {
		if supported { Ok(()) } else { Err(Error::Unsupported(name)) }
	}

	/// Write a value into the running instance's defaults, persisted via the sink (for submitting credentials after a successful login)
	fn set_default(&mut self, key: String, value: DefaultValue) {
		self.store.data_mut().defaults.set(key, value);
	}

	// The partial-result sink is attached only for the duration of a single export call
	// and detached when the call ends (including on trap), so it never leaks into the next call
	fn with_partials<T>(
		&mut self,
		sink: crate::runtime::env::PartialSink,
		f: impl FnOnce(&mut Self) -> Result<T>,
	) -> Result<T> {
		self.store.data_mut().partial_sink = Some(sink);
		let result = f(self);
		self.store.data_mut().partial_sink = None;
		result
	}

	pub fn get_home_with_partials(&mut self, sink: crate::runtime::env::PartialSink) -> Result<HomeLayout> {
		self.with_partials(sink, |i| i.get_home())
	}

	pub fn get_search_manga_list_with_partials(
		&mut self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
		sink: crate::runtime::env::PartialSink,
	) -> Result<MangaPageResult> {
		self.with_partials(sink, |i| i.get_search_manga_list(query, page, filters))
	}

	pub fn get_manga_update_with_partials(
		&mut self,
		manga: &Manga,
		needs_details: bool,
		needs_chapters: bool,
		sink: crate::runtime::env::PartialSink,
	) -> Result<Manga> {
		self.with_partials(sink, |i| i.get_manga_update(manga, needs_details, needs_chapters))
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

	pub fn handle_basic_login(&mut self, key: &str, username: &str, password: &str) -> Result<bool> {
		self.check(self.features.basic_login, "handle_basic_login")?;
		let mut t = Vec::new();
		let key_rid = self.store_arg(&key, &mut t)?;
		let username_rid = self.store_arg(&username, &mut t)?;
		let password_rid = self.store_arg(&password, &mut t)?;
		let ret = self.call_raw(
			"handle_basic_login",
			&[key_rid.into(), username_rid.into(), password_rid.into()],
		);
		let result = ret.and_then(|r| self.read_result::<bool>(r));
		self.cleanup(t);
		// Aligned with Aidoku: after a successful login, write the credentials and the key's marker into defaults (persisted via the sink)
		if result.as_ref().is_ok_and(|ok| *ok) {
			self.set_default(format!("{key}.username"), DefaultValue::String(username.to_string()));
			self.set_default(format!("{key}.password"), DefaultValue::String(password.to_string()));
			self.set_default(key.to_string(), DefaultValue::String("logged_in".to_string()));
		}
		result
	}

	pub fn handle_web_login(&mut self, key: &str, cookies: &HashMap<String, String>) -> Result<bool> {
		self.check(self.features.web_login, "handle_web_login")?;
		let mut keys: Vec<String> = cookies.keys().cloned().collect();
		keys.sort();
		let values: Vec<String> = keys
			.iter()
			.map(|k| cookies.get(k).cloned().unwrap_or_default())
			.collect();
		// Aligned with Aidoku's store-first-then-call: inside handle_web_login the source
		// reads back `<key>.keys/.values` via defaults_get_map to get the cookies, not via
		// parameters
		self.set_default(format!("{key}.keys"), DefaultValue::StringArray(keys.clone()));
		self.set_default(format!("{key}.values"), DefaultValue::StringArray(values.clone()));
		let mut t = Vec::new();
		let key_rid = self.store_arg(&key, &mut t)?;
		let keys_rid = self.store_arg(&keys, &mut t)?;
		let values_rid = self.store_arg(&values, &mut t)?;
		let ret = self.call_raw(
			"handle_web_login",
			&[key_rid.into(), keys_rid.into(), values_rid.into()],
		);
		let result = ret.and_then(|r| self.read_result::<bool>(r));
		self.cleanup(t);
		// The marker is persisted synchronously by the server: after a web login succeeds
		// the instance is restarted to swap the cookie jar, and an async sink write would
		// race the restart's read of the store (only basic_login, after which there is no
		// restart, may write that way)
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
			let host = self.store.data_mut();
			let request = host
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
			host.store.remove(rid);
			Ok(spec)
		});
		self.cleanup(t);
		result
	}

	fn image_png(&mut self, rid: Rid) -> Result<Vec<u8>> {
		let host = self.store.data_mut();
		let item = host
			.store
			.get(rid)
			.ok_or_else(|| Error::Instance(format!("invalid image rid {rid}")))?;
		match item {
			StoreItem::ImageData(image) => image
				.encode_png()
				.ok_or_else(|| Error::Instance("encode image png failed".into())),
			// The source passed through raw bytes the host could not decode: consistent with Aidoku's fallback to the raw data, hand them back as-is
			StoreItem::ImageBytes(raw) => Ok(raw.clone()),
			_ => Err(Error::Instance(format!("invalid image rid {rid}"))),
		}
	}

	/// Take the image bytes away: the rid is reclaimed whether it succeeds or fails, otherwise the store only grows on every page-list fetch
	pub fn take_image_png(&mut self, rid: Rid) -> Result<Vec<u8>> {
		let result = self.image_png(rid);
		self.store.data_mut().store.remove(rid);
		result
	}

	fn process_image(
		&mut self,
		export: &'static str,
		supported: bool,
		context: Option<Option<&PageContext>>,
		input: ProcessImageInput,
	) -> Result<Vec<u8>> {
		self.check(supported, export)?;
		// AidokuRunner `store(value:)` semantics: decodable inputs become pixels (sources
		// like jmcomic need width/height/canvas to assemble strips); only encrypted bytes
		// that cannot be decoded are left as-is for the source to decrypt itself
		let image_rid = {
			let host = self.store.data_mut();
			let item = match crate::imports::canvas::decode_image(&input.bytes) {
				Some(image) => StoreItem::ImageData(image),
				None => StoreItem::ImageBytes(input.bytes),
			};
			host.store.store(item)
		};
		let response = aidoku::ImageResponse {
			code: input.status,
			headers: input.response_headers.into_iter().collect(),
			request: aidoku::ImageRequest {
				url: Some(input.url),
				headers: input.request_headers.into_iter().collect(),
			},
			// A reference built on the host side must be externally_managed, otherwise the source destroys it on drop
			image: aidoku::imports::canvas::ImageRef {
				rid: image_rid,
				externally_managed: true,
			},
		};
		let mut t = Vec::new();
		let response_rid = self.store_arg(&response, &mut t)?;
		let mut args = vec![response_rid.into()];
		// The outer Option distinguishes the export signatures: a page processor takes a context argument (None passed as -1), a cover processor does not
		if let Some(context) = context {
			let context_rid = match context {
				Some(context) => self.store_arg(context, &mut t)?,
				None => -1,
			};
			args.push(context_rid.into());
		}
		let ret = self.call_raw(export, &args);
		let result = ret.and_then(|r| self.read_result::<i32>(r));
		// The source may pass the input image through unchanged (jmcomic's old-chapter/gif
		// branch): when the result has the same rid as the input, the input already is the
		// result — removing it would make the image unretrievable
		if result.as_ref().ok() != Some(&image_rid) {
			self.store.data_mut().store.remove(image_rid);
		}
		self.cleanup(t);
		let result_rid = result?;
		self.take_image_png(result_rid)
	}

	pub fn process_page_image(
		&mut self,
		input: ProcessImageInput,
		context: Option<&PageContext>,
	) -> Result<Vec<u8>> {
		self.process_image(
			"process_page_image",
			self.features.page_image_processor,
			Some(context),
			input,
		)
	}

	pub fn process_cover_image(&mut self, input: ProcessImageInput) -> Result<Vec<u8>> {
		self.process_image("process_cover_image", self.features.cover_image_processor, None, input)
	}
}

