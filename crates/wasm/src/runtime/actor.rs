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

/// What the per-call log line reports as `entries`; the default is None for results
/// where a count is meaningless (login, notification, image bytes, URLs)
trait CallEntries {
	fn entries(&self) -> Option<usize> {
		None
	}
}

impl CallEntries for MangaPageResult {
	fn entries(&self) -> Option<usize> {
		Some(self.entries.len())
	}
}

impl CallEntries for Manga {
	fn entries(&self) -> Option<usize> {
		self.chapters.as_ref().map(Vec::len)
	}
}

impl CallEntries for HomeLayout {
	fn entries(&self) -> Option<usize> {
		Some(self.components.len())
	}
}

impl CallEntries for Vec<Page> {
	fn entries(&self) -> Option<usize> {
		Some(self.len())
	}
}

impl CallEntries for Vec<Filter> {
	fn entries(&self) -> Option<usize> {
		Some(self.len())
	}
}

impl CallEntries for Vec<Setting> {
	fn entries(&self) -> Option<usize> {
		Some(self.len())
	}
}

impl CallEntries for Vec<Listing> {
	fn entries(&self) -> Option<usize> {
		Some(self.len())
	}
}

impl CallEntries for Vec<String> {
	fn entries(&self) -> Option<usize> {
		Some(self.len())
	}
}

impl CallEntries for Vec<u8> {}
impl CallEntries for String {}
impl CallEntries for bool {}
impl CallEntries for () {}
impl CallEntries for ImageRequestSpec {}
impl CallEntries for Option<DeepLinkResult> {}

struct Inner {
	tx: mpsc::Sender<Cmd>,
	thread_alive: Arc<AtomicBool>,
}

/// Per-source actor handle. All export calls go over mpsc to a dedicated OS thread and run
/// serially — serial within a source, parallel across sources; instances are long-lived and
/// restart swaps the thread and rebuilds.
pub struct SourceHandle {
	key: String,
	config: Arc<RwLock<SourceConfig>>,
	// Reads and writes are both short critical sections; the lock is never held across an await
	inner: Arc<RwLock<Inner>>,
	features: Arc<RwLock<SourceFeatures>>,
	semaphore: Arc<Semaphore>,
	/// Per-source call admission cap derived from the manifest's config.maximumParallelRequests; None means only the global cap applies
	source_semaphore: Option<Arc<Semaphore>>,
	call_timeout: Duration,
}

impl SourceHandle {
	pub async fn spawn(
		config: SourceConfig,
		semaphore: Arc<Semaphore>,
	) -> Result<(Self, SourceFeatures)> {
		Self::spawn_with_source_limit(config, semaphore, None).await
	}

	pub async fn spawn_with_source_limit(
		config: SourceConfig,
		semaphore: Arc<Semaphore>,
		source_semaphore: Option<Arc<Semaphore>>,
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
			source_semaphore,
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
			// wasmi expands guest calls into native recursion frames: the default 2 MiB
			// stack can physically overflow within the recursion limit (1000 frames) and
			// abort the whole process (#213). 8 MiB is a virtual reservation, lazily
			// committed — unused memory costs no RSS.
			.stack_size(8 * 1024 * 1024)
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
								// After a host import panics, the instance state is untrustworthy; the thread exits and the upper layer restarts
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

	/// This source's default Accept-Language (derived from the source languages); the server's direct cover/page fetches reuse the same value
	pub fn accept_language(&self) -> String {
		self.config.read().expect("config lock").accept_language.clone()
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

	async fn call<T: Send + 'static + CallEntries>(
		&self,
		op: &'static str,
		job: impl FnOnce(&mut SourceInstance) -> Result<T> + Send + 'static,
	) -> Result<T> {
		// Take the per-source cap before occupying the global one: a source that declares
		// maximumParallelRequests is pushed back at the entrance instead of consuming a
		// global permit while queued
		let _source_permit = match &self.source_semaphore {
			Some(sem) => Some(
				sem.clone()
					.acquire_owned()
					.await
					.map_err(|_| Error::Instance("source call semaphore closed".into()))?,
			),
			None => None,
		};
		let _permit = self
			.semaphore
			.clone()
			.acquire_owned()
			.await
			.map_err(|_| Error::Instance("call semaphore closed".into()))?;
		let (reply_tx, reply_rx) = oneshot::channel::<Result<T>>();
		let tx = self.inner.read().expect("inner lock").tx.clone();
		let start = std::time::Instant::now();
		if tx
			.send(Cmd::Job(Box::new(move |instance| {
				let _ = reply_tx.send(job(instance));
			})))
			.await
			.is_err()
		{
			let result = Err(Error::Instance("actor thread dead".into()));
			self.log_outcome(op, start.elapsed(), &result);
			return result;
		}
		let result = match tokio::time::timeout(self.call_timeout, reply_rx).await {
			Ok(Ok(result)) => result,
			Ok(Err(_closed)) => Err(Error::Trap("source call panicked".into())),
			Err(_elapsed) => {
				let result = Err(Error::Timeout);
				// Logged before the rebuild so the duration reflects the call itself
				self.log_outcome(op, start.elapsed(), &result);
				// Timeout circuit breaker: the old thread may be stuck inside a host call, so swap the thread and rebuild the instance
				let defaults = self.config.read().expect("config lock").defaults.clone();
				let _ = self.restart(defaults).await;
				return result;
			}
		};
		self.log_outcome(op, start.elapsed(), &result);
		result
	}

	/// One structured line per source call, at the chokepoint every op funnels through;
	/// errors go through log_safe because guest/reqwest text can carry full URLs
	fn log_outcome<T: CallEntries>(&self, op: &'static str, dur: Duration, result: &Result<T>) {
		let dur_ms = dur.as_millis() as u64;
		match result {
			Ok(value) => {
				if let Some(entries) = value.entries() {
					tracing::info!(source = %self.key, op = %op, dur_ms, entries, "source call");
				} else {
					tracing::info!(source = %self.key, op = %op, dur_ms, "source call");
				}
			}
			Err(e) => {
				let error = e.log_safe();
				tracing::warn!(source = %self.key, op = %op, dur_ms, error = error.as_str(), "source call");
			}
		}
	}

	pub async fn restart(&self, defaults: HashMap<String, DefaultValue>) -> Result<SourceFeatures> {
		let cookies = self.config.read().expect("config lock").cookies.clone();
		self.restart_with(defaults, cookies).await
	}

	/// Rebuild the instance with new defaults and cookie jar; clear-cache / settings changes go through here
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
		// The old tx disconnects as Inner is swapped out; the old thread exits once it finishes its current job (bounded by fuel)
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
		self.call("search", move |i| i.get_search_manga_list(query, page, filters)).await
	}

	pub async fn get_manga_update(
		&self,
		manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		self.call("manga_update", move |i| i.get_manga_update(&manga, needs_details, needs_chapters))
			.await
	}

	pub async fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		self.call("page_list", move |i| i.get_page_list(&manga, &chapter)).await
	}

	pub async fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		self.call("listing", move |i| i.get_manga_list(&listing, page)).await
	}

	pub async fn get_home(&self) -> Result<HomeLayout> {
		self.call("home", |i| i.get_home()).await
	}

	/// Streaming get_home: partial results the source pushes via send_partial_result
	/// mid-call flow into partials in arrival order. When the receiver is gone (client
	/// disconnected), send failures are silently dropped and the source runs to completion
	pub async fn get_home_streaming(
		&self,
		partials: mpsc::UnboundedSender<HomePartial>,
	) -> Result<HomeLayout> {
		self.call("home", move |i| {
			let acc = Arc::new(Mutex::new(HomeAccumulator::default()));
			let sink_acc = acc.clone();
			let sink: crate::runtime::env::PartialSink = Box::new(move |bytes| {
				match postcard::from_bytes::<HomePartialResult>(bytes) {
					Ok(partial) => {
						let event = sink_acc.lock().expect("accumulator lock").apply(partial);
						let _ = partials.send(event);
					}
					// A partial result is only a progressive preview; a decode failure is not worth failing the whole request
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

	/// Streaming get_search_manga_list: each partial payload is one complete Manga, appended in arrival order
	pub async fn get_search_manga_list_streaming(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
		partials: mpsc::UnboundedSender<Manga>,
	) -> Result<MangaPageResult> {
		self.call("search", move |i| {
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

	/// Streaming get_manga_update: aligned with upstream partialMangaPublisher — each
	/// partial is a full Manga snapshot of that moment (details/chapters filled in as the
	/// fetch progresses); the final state follows the export's return, with no home-style
	/// accumulation merge
	pub async fn get_manga_update_streaming(
		&self,
		manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
		partials: mpsc::UnboundedSender<Manga>,
	) -> Result<Manga> {
		self.call("manga_update", move |i| {
			let sink: crate::runtime::env::PartialSink = Box::new(move |bytes| {
				match postcard::from_bytes::<Manga>(bytes) {
					Ok(manga) => {
						let _ = partials.send(manga);
					}
					Err(_) => tracing::warn!("failed to decode manga update partial result"),
				}
			});
			i.get_manga_update_with_partials(&manga, needs_details, needs_chapters, sink)
		})
		.await
	}

	pub async fn get_filters(&self) -> Result<Vec<Filter>> {
		self.call("filters", |i| i.get_filters()).await
	}

	pub async fn get_settings(&self) -> Result<Vec<Setting>> {
		self.call("settings", |i| i.get_settings()).await
	}

	pub async fn get_listings(&self) -> Result<Vec<Listing>> {
		self.call("listings", |i| i.get_listings()).await
	}

	pub async fn get_page_description(&self, page: Page) -> Result<String> {
		self.call("page_description", move |i| i.get_page_description(&page)).await
	}

	pub async fn get_alternate_covers(&self, manga: Manga) -> Result<Vec<String>> {
		self.call("alternate_covers", move |i| i.get_alternate_covers(&manga)).await
	}

	pub async fn get_base_url(&self) -> Result<String> {
		self.call("base_url", |i| i.get_base_url()).await
	}

	pub async fn handle_notification(&self, key: String) -> Result<()> {
		self.call("notification", move |i| i.handle_notification(&key)).await
	}

	pub async fn handle_basic_login(
		&self,
		key: String,
		username: String,
		password: String,
	) -> Result<bool> {
		self.call("basic_login", move |i| i.handle_basic_login(&key, &username, &password))
			.await
	}

	pub async fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		self.call("web_login", move |i| i.handle_web_login(&key, &cookies)).await
	}

	pub async fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		self.call("deep_link", move |i| i.handle_deep_link(&url)).await
	}

	pub async fn handle_key_migration(
		&self,
		kind: i32,
		manga_key: String,
		chapter_key: Option<String>,
	) -> Result<String> {
		self.call("key_migration", move |i| {
			i.handle_key_migration(kind, &manga_key, chapter_key.as_deref())
		})
		.await
	}

	pub async fn get_image_request(
		&self,
		url: String,
		context: Option<PageContext>,
	) -> Result<ImageRequestSpec> {
		self.call("image_request", move |i| i.get_image_request(&url, context.as_ref()))
			.await
	}

	/// Resolve the in-source handle of a PageContent::Image back into PNG bytes and reclaim the rid
	pub async fn take_page_image(&self, rid: i32) -> Result<Vec<u8>> {
		self.call("take_page_image", move |i| i.take_image_png(rid)).await
	}

	pub async fn process_page_image(
		&self,
		input: crate::source::ProcessImageInput,
		context: Option<PageContext>,
	) -> Result<Vec<u8>> {
		self.call("process_page_image", move |i| i.process_page_image(input, context.as_ref()))
			.await
	}

	pub async fn process_cover_image(&self, input: crate::source::ProcessImageInput) -> Result<Vec<u8>> {
		self.call("process_cover_image", move |i| i.process_cover_image(input)).await
	}
}
