use crate::error::{Error, Result};
use crate::features::SourceFeatures;
use crate::partial::{HomeAccumulator, HomePartial};
use crate::source::{ImageRequestSpec, SourceConfig, SourceInstance};
use crate::runtime::store::DefaultValue;
use aidoku::{
	Chapter, DeepLinkResult, Filter, FilterValue, HomeLayout, HomePartialResult, Listing, Manga,
	MangaPageResult, Page, PageContext, Setting,
};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut SourceInstance) + Send>;

enum Cmd {
	Job(Job),
}

struct Inner {
	tx: mpsc::Sender<Cmd>,
	thread_alive: Arc<AtomicBool>,
}

/// 每源 actor 句柄。所有导出调用经 mpsc 发给独立 OS 线程串行执行，
/// 源内串行、源间并行；实例长生命周期，restart 即换线程重建。
pub struct SourceHandle {
	key: String,
	config: Arc<RwLock<SourceConfig>>,
	// 读写均为短临界区，无跨 await 持锁
	inner: Arc<RwLock<Inner>>,
	features: Arc<RwLock<SourceFeatures>>,
	semaphore: Arc<Semaphore>,
	call_timeout: Duration,
}

impl SourceHandle {
	pub async fn spawn(
		config: SourceConfig,
		semaphore: Arc<Semaphore>,
	) -> Result<(Self, SourceFeatures)> {
		let inner = Self::spawn_actor(&config).await?;
		let features = inner.1?;
		let call_timeout = config.limits.call_timeout;
		let handle = Self {
			key: config.key.clone(),
			config: Arc::new(RwLock::new(config)),
			inner: Arc::new(RwLock::new(inner.0)),
			features: Arc::new(RwLock::new(features.clone())),
			semaphore,
			call_timeout,
		};
		Ok((handle, features))
	}

	async fn spawn_actor(config: &SourceConfig) -> Result<(Inner, Result<SourceFeatures>)> {
		let (tx, mut rx) = mpsc::channel::<Cmd>(64);
		let (init_tx, init_rx) = oneshot::channel::<Result<SourceFeatures>>();
		let thread_alive = Arc::new(AtomicBool::new(true));
		let alive_flag = thread_alive.clone();
		let config = config.clone();
		std::thread::Builder::new()
			.name(format!("source-{}", config.key))
			.spawn(move || {
				let mut instance = match SourceInstance::build(&config) {
					Ok(instance) => {
						let _ = init_tx.send(Ok(instance.features.clone()));
						instance
					}
					Err(e) => {
						let _ = init_tx.send(Err(e));
						return;
					}
				};
				while let Some(cmd) = rx.blocking_recv() {
					match cmd {
						Cmd::Job(job) => {
							if catch_unwind(AssertUnwindSafe(|| job(&mut instance))).is_err() {
								// 宿主 import 恐慌后实例状态不可信，线程退出交给上层重启
								break;
							}
						}
					}
				}
				alive_flag.store(false, Ordering::SeqCst);
			})
			.map_err(|e| Error::Instance(format!("spawn actor thread: {e}")))?;
		let init = tokio::time::timeout(Duration::from_secs(120), init_rx)
			.await
			.map_err(|_| Error::Instance("actor init timeout".into()))?
			.map_err(|_| Error::Instance("actor init channel closed".into()))?;
		Ok((Inner { tx, thread_alive }, init))
	}

	pub fn key(&self) -> &str {
		&self.key
	}

	pub fn features(&self) -> SourceFeatures {
		self.features.read().expect("features lock").clone()
	}

	pub fn is_alive(&self) -> bool {
		self.inner
			.read()
			.expect("inner lock")
			.thread_alive
			.load(Ordering::SeqCst)
	}

	async fn call<T: Send + 'static>(
		&self,
		job: impl FnOnce(&mut SourceInstance) -> Result<T> + Send + 'static,
	) -> Result<T> {
		let _permit = self
			.semaphore
			.clone()
			.acquire_owned()
			.await
			.map_err(|_| Error::Instance("call semaphore closed".into()))?;
		let (reply_tx, reply_rx) = oneshot::channel::<Result<T>>();
		let tx = self.inner.read().expect("inner lock").tx.clone();
		tx.send(Cmd::Job(Box::new(move |instance| {
			let _ = reply_tx.send(job(instance));
		})))
		.await
		.map_err(|_| Error::Instance("actor thread dead".into()))?;
		match tokio::time::timeout(self.call_timeout, reply_rx).await {
			Ok(Ok(result)) => result,
			Ok(Err(_closed)) => Err(Error::Trap("source call panicked".into())),
			Err(_elapsed) => {
				// 超时熔断：旧线程可能卡在宿主调用里，换线程重建实例
				let defaults = self.config.read().expect("config lock").defaults.clone();
				let _ = self.restart(defaults).await;
				Err(Error::Timeout)
			}
		}
	}

	pub async fn restart(&self, defaults: HashMap<String, DefaultValue>) -> Result<SourceFeatures> {
		let cookies = self.config.read().expect("config lock").cookies.clone();
		self.restart_with(defaults, cookies).await
	}

	/// 换 defaults 与 cookie jar 重建实例；clear-cache / 设置变更走这里
	pub async fn restart_with(
		&self,
		defaults: HashMap<String, DefaultValue>,
		cookies: HashMap<String, HashMap<String, String>>,
	) -> Result<SourceFeatures> {
		let config = {
			let mut guard = self.config.write().expect("config lock");
			guard.defaults = defaults;
			guard.cookies = cookies;
			guard.clone()
		};
		let (inner, init) = Self::spawn_actor(&config).await?;
		let features = init?;
		// 旧 tx 随 Inner 替换而断开，旧线程跑完当前 job（受 fuel 约束）即退出
		*self.inner.write().expect("inner lock") = inner;
		*self.features.write().expect("features lock") = features.clone();
		Ok(features)
	}

	pub async fn shutdown(self) {
		let mut guard = self.inner.write().expect("inner lock");
		let (tx, _) = mpsc::channel::<Cmd>(1);
		guard.tx = tx;
	}

	pub async fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		self.call(move |i| i.get_search_manga_list(query, page, filters)).await
	}

	pub async fn get_manga_update(
		&self,
		manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		self.call(move |i| i.get_manga_update(&manga, needs_details, needs_chapters))
			.await
	}

	pub async fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		self.call(move |i| i.get_page_list(&manga, &chapter)).await
	}

	pub async fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		self.call(move |i| i.get_manga_list(&listing, page)).await
	}

	pub async fn get_home(&self) -> Result<HomeLayout> {
		self.call(|i| i.get_home()).await
	}

	/// 流式版 get_home：源在调用中途 send_partial_result 的部分结果按到达顺序
	/// 推进 partials。接收端掉线（客户端断开）时发送失败静默丢弃，源继续跑完
	pub async fn get_home_streaming(
		&self,
		partials: mpsc::UnboundedSender<HomePartial>,
	) -> Result<HomeLayout> {
		self.call(move |i| {
			let acc = Arc::new(Mutex::new(HomeAccumulator::default()));
			let sink_acc = acc.clone();
			let sink: crate::runtime::env::PartialSink = Box::new(move |bytes| {
				match postcard::from_bytes::<HomePartialResult>(bytes) {
					Ok(partial) => {
						let event = sink_acc.lock().expect("accumulator lock").apply(partial);
						let _ = partials.send(event);
					}
					// 部分结果只是渐进预览，解码失败不值得让整个请求失败
					Err(_) => tracing::warn!("failed to decode home partial result"),
				}
			});
			let layout = i.get_home_with_partials(sink)?;
			let acc = match Arc::try_unwrap(acc) {
				Ok(mutex) => mutex.into_inner().expect("accumulator lock"),
				Err(arc) => arc.lock().expect("accumulator lock").clone(),
			};
			Ok(acc.finish(layout))
		})
		.await
	}

	/// 流式版 get_search_manga_list：每个 partial 载荷是一条完整 Manga，按到达顺序追加
	pub async fn get_search_manga_list_streaming(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
		partials: mpsc::UnboundedSender<Manga>,
	) -> Result<MangaPageResult> {
		self.call(move |i| {
			let sink: crate::runtime::env::PartialSink = Box::new(move |bytes| {
				match postcard::from_bytes::<Manga>(bytes) {
					Ok(manga) => {
						let _ = partials.send(manga);
					}
					Err(_) => tracing::warn!("failed to decode search partial result"),
				}
			});
			i.get_search_manga_list_with_partials(query, page, filters, sink)
		})
		.await
	}

	pub async fn get_filters(&self) -> Result<Vec<Filter>> {
		self.call(|i| i.get_filters()).await
	}

	pub async fn get_settings(&self) -> Result<Vec<Setting>> {
		self.call(|i| i.get_settings()).await
	}

	pub async fn get_listings(&self) -> Result<Vec<Listing>> {
		self.call(|i| i.get_listings()).await
	}

	pub async fn get_page_description(&self, page: Page) -> Result<String> {
		self.call(move |i| i.get_page_description(&page)).await
	}

	pub async fn get_alternate_covers(&self, manga: Manga) -> Result<Vec<String>> {
		self.call(move |i| i.get_alternate_covers(&manga)).await
	}

	pub async fn get_base_url(&self) -> Result<String> {
		self.call(|i| i.get_base_url()).await
	}

	pub async fn handle_notification(&self, key: String) -> Result<()> {
		self.call(move |i| i.handle_notification(&key)).await
	}

	pub async fn handle_basic_login(
		&self,
		key: String,
		username: String,
		password: String,
	) -> Result<bool> {
		self.call(move |i| i.handle_basic_login(&key, &username, &password))
			.await
	}

	pub async fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		self.call(move |i| i.handle_deep_link(&url)).await
	}

	pub async fn handle_key_migration(
		&self,
		kind: i32,
		manga_key: String,
		chapter_key: Option<String>,
	) -> Result<String> {
		self.call(move |i| i.handle_key_migration(kind, &manga_key, chapter_key.as_deref()))
			.await
	}

	pub async fn get_image_request(
		&self,
		url: String,
		context: Option<PageContext>,
	) -> Result<ImageRequestSpec> {
		self.call(move |i| i.get_image_request(&url, context.as_ref())).await
	}

	/// 把 PageContent::Image 的源内句柄还原成 PNG 字节并回收 rid
	pub async fn take_page_image(&self, rid: i32) -> Result<Vec<u8>> {
		self.call(move |i| i.take_image_png(rid)).await
	}

	pub async fn process_page_image(
		&self,
		input: crate::source::ProcessImageInput,
		context: Option<PageContext>,
	) -> Result<Vec<u8>> {
		self.call(move |i| i.process_page_image(input, context.as_ref())).await
	}

	pub async fn process_cover_image(&self, input: crate::source::ProcessImageInput) -> Result<Vec<u8>> {
		self.call(move |i| i.process_cover_image(input)).await
	}
}
