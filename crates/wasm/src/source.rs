use crate::error::{Error, Result};
use crate::features::SourceFeatures;
use crate::imports::HttpMethod;
use crate::runtime::env::WasmEnv;
use crate::runtime::limits::{Limits, LimitedTunables};
use crate::runtime::store::{DefaultValue, GlobalStore, ImageData, Rid, StoreItem};
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
	/// 持久化的 cookie：domain → (name → value)，实例启动时回填 jar
	pub cookies: HashMap<String, HashMap<String, String>>,
	pub cookie_sink: Arc<dyn CookieSink>,
	pub policy: Arc<dyn NetPolicy>,
	pub logger: Arc<dyn HostLogger>,
	/// 源语言推导出的 Accept-Language 默认值；源没自带时由 net 宿主补上
	pub accept_language: String,
	/// CDP websocket 地址（ws://…）；None 时 webview 浏览器函数显式不支持
	pub browser_ws: Option<String>,
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
			browser_ws: None,
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

/// process_*_image 的入参：已取回的图像字节与构造 ImageResponse 所需的请求上下文
#[derive(Debug, Clone)]
pub struct ProcessImageInput {
	pub bytes: Vec<u8>,
	pub url: String,
	pub status: u16,
	pub response_headers: Vec<(String, String)>,
	pub request_headers: Vec<(String, String)>,
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

		let mut net = crate::runtime::env::NetState::new(
			config.policy.clone(),
			crate::imports::CookieJar::new(config.cookies.clone(), config.cookie_sink.clone()),
		);
		net.accept_language = config.accept_language.clone();

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
				net,
				browser: crate::browser::BrowserState::new(
					config.browser_ws.clone(),
					config.limits.browser_timeout,
				),
				limits: config.limits.clone(),
				source_key: config.key.clone(),
				logger: config.logger.clone(),
				partial_sink: None,
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

	/// 把值写进运行中实例的 defaults，经 sink 持久化（供登录成功后提交凭据）
	fn set_default(&mut self, key: String, value: DefaultValue) {
		self.env.as_mut(&mut self.store).defaults.set(key, value);
	}

	// 部分结果 sink 只在单个导出调用期间挂上，调用结束（含 trap）即摘，
	// 不泄漏到下一个调用
	fn with_partials<T>(
		&mut self,
		sink: crate::runtime::env::PartialSink,
		f: impl FnOnce(&mut Self) -> Result<T>,
	) -> Result<T> {
		self.env.as_mut(&mut self.store).partial_sink = Some(sink);
		let result = f(self);
		self.env.as_mut(&mut self.store).partial_sink = None;
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
		// 对齐 Aidoku：登录成功后把凭据与该键的 marker 写进 defaults（经 sink 落库）
		if result.as_ref().is_ok_and(|ok| *ok) {
			self.set_default(format!("{key}.username"), DefaultValue::String(username.to_string()));
			self.set_default(format!("{key}.password"), DefaultValue::String(password.to_string()));
			self.set_default(key.to_string(), DefaultValue::String("logged_in".to_string()));
		}
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

	fn image_png(&mut self, rid: Rid) -> Result<Vec<u8>> {
		let env = self.env.as_mut(&mut self.store);
		let item = env
			.store
			.get(rid)
			.ok_or_else(|| Error::Instance(format!("invalid image rid {rid}")))?;
		match item {
			// 源 export 返回的 ImageRef（new_image 解码过）→ 编码 PNG
			StoreItem::ImageData(image) => image
				.encode_png()
				.ok_or_else(|| Error::Instance("encode image png failed".into())),
			// 源把宿主透传的原始字节直接回传（pass-through 处理器）→ 原样返回
			StoreItem::Encoded(bytes) => Ok(bytes.clone()),
			_ => Err(Error::Instance(format!("invalid image rid {rid}"))),
		}
	}

	/// 取走图像字节：无论成败都回收 rid，否则每拉一次 page list store 只涨不消
	pub fn take_image_png(&mut self, rid: Rid) -> Result<Vec<u8>> {
		let result = self.image_png(rid);
		self.env.as_mut(&mut self.store).store.remove(rid);
		result
	}

	fn decode_into_store(&mut self, bytes: &[u8]) -> Result<Rid> {
		let Some(rgba) = image::ImageReader::new(std::io::Cursor::new(bytes))
			.with_guessed_format()
			.ok()
			.and_then(|reader| reader.decode().ok())
			.map(|img| img.to_rgba8())
		else {
			// 源声明了图片处理器却喂来宿主侧不可解码的字节（favcomic 这类加密图床
			// 返回 IV||AES-128-CBC 密文）时，不在这里失败：把原始字节以 Encoded 入
			// store，data() 原样取回，让源的 export 自行解密/处理。与 Aidoku 契约
			// 一致——声明处理器的源收到的是原始响应体（见 canvas 的 get_image_data）。
			let env = self.env.as_mut(&mut self.store);
			return Ok(env.store.store(StoreItem::Encoded(bytes.to_vec())));
		};
		let (width, height) = rgba.dimensions();
		let env = self.env.as_mut(&mut self.store);
		Ok(env
			.store
			.store(StoreItem::ImageData(ImageData::from_rgba8(rgba.into_raw(), width, height))))
	}

	fn process_image(
		&mut self,
		export: &'static str,
		supported: bool,
		context: Option<Option<&PageContext>>,
		input: ProcessImageInput,
	) -> Result<Vec<u8>> {
		self.check(supported, export)?;
		let image_rid = self.decode_into_store(&input.bytes)?;
		let response = aidoku::ImageResponse {
			code: input.status,
			headers: input.response_headers.into_iter().collect(),
			request: aidoku::ImageRequest {
				url: Some(input.url),
				headers: input.request_headers.into_iter().collect(),
			},
			// 宿主侧构造的引用必须 externally_managed，否则源 drop 时会 destroy
			image: aidoku::imports::canvas::ImageRef {
				rid: image_rid,
				externally_managed: true,
			},
		};
		let mut t = Vec::new();
		let response_rid = self.store_arg(&response, &mut t)?;
		let mut args = vec![response_rid.into()];
		// 外层 Option 区分导出签名：page 处理器带 context 参（None 传 -1），cover 处理器没有
		if let Some(context) = context {
			let context_rid = match context {
				Some(context) => self.store_arg(context, &mut t)?,
				None => -1,
			};
			args.push(context_rid.into());
		}
		let ret = self.call_raw(export, &args);
		let result = ret.and_then(|r| self.read_result::<i32>(r));
		self.env.as_mut(&mut self.store).store.remove(image_rid);
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

