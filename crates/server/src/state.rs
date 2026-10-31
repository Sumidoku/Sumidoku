use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::source::routed::{RoutedClients, SumidokuNetPolicy};
use aidoku_wasm::{
	CookieSink, DefaultValue, DefaultsSink, SourceConfig, SourceFeatures, SourceHandle,
};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, mpsc};

type ChapterVariantKey = (i64, i64, u32);
type ChapterVariantLocks =
	HashMap<ChapterVariantKey, std::sync::Weak<tokio::sync::Mutex<()>>>;
type ChapterOriginalKey = (i64, i64);
type OriginalPageResponse = Arc<(Vec<u8>, String)>;
type OriginalPageFlightResult = Option<(Vec<u8>, OriginalPageResponse)>;

pub struct OriginalPageFlight {
	pub lock: tokio::sync::Mutex<()>,
	result: std::sync::Mutex<OriginalPageFlightResult>,
}

impl OriginalPageFlight {
	pub fn cached(&self, identity: &[u8]) -> Option<OriginalPageResponse> {
		self.result.lock().ok()?.as_ref().filter(|(key, _)| key.as_slice() == identity).map(|(_, value)| value.clone())
	}
	pub fn publish(&self, identity: Vec<u8>, bytes: Vec<u8>, mime: String) {
		if let Ok(mut slot) = self.result.lock() { *slot = Some((identity, Arc::new((bytes, mime)))); }
	}
}


pub struct SourceEntry {
	pub handle: Arc<SourceHandle>,
	pub key: String,
	pub features: SourceFeatures,
	/// Per-source call admission cap derived from manifest config.maximumParallelRequests; None = only the global cap applies
	pub call_limit: Option<Arc<Semaphore>>,
	/// Time of the last get_handle hit; the criterion for idle eviction
	last_used: std::sync::Mutex<std::time::Instant>,
}

impl SourceEntry {
	fn new(
		handle: Arc<SourceHandle>,
		key: String,
		features: SourceFeatures,
		call_limit: Option<Arc<Semaphore>>,
	) -> Self {
		Self {
			handle,
			key,
			features,
			call_limit,
			last_used: std::sync::Mutex::new(std::time::Instant::now()),
		}
	}

	fn touch(&self) {
		*self.last_used.lock().expect("last_used lock") = std::time::Instant::now();
	}

	fn idle_for(&self, now: std::time::Instant) -> std::time::Duration {
		now.saturating_duration_since(*self.last_used.lock().expect("last_used lock"))
	}
}

pub struct AppState {
	pub pool: SqlitePool,
	pub config: Config,
	pub auth: crate::auth::AuthState,
	pub sources: RwLock<HashMap<i64, SourceEntry>>,
	pub call_semaphore: Arc<Semaphore>,
	pub defaults_tx: mpsc::UnboundedSender<DefaultsWrite>,
	pub cookie_tx: mpsc::UnboundedSender<CookieWrite>,
	pub http: RoutedClients,
	pub policy: Arc<SumidokuNetPolicy>,
	/// LRU of local-source CBZ handles (pages are extracted on demand; reopening an archive means re-reading the central directory)
	pub local_zip: Arc<crate::internal::local::ZipCache>,
	// Injected once during build_app assembly, after AppState::new (the manager depends back on state)
	pub downloads: tokio::sync::OnceCell<Arc<crate::download::DownloadManager>>,
	pub updater: tokio::sync::OnceCell<Arc<crate::update::UpdateScheduler>>,
	pub events: tokio::sync::OnceCell<Arc<crate::events::EventHub>>,
	pub tasks: tokio::sync::OnceCell<Arc<crate::task::TaskManager>>,
	pub tracker: tokio::sync::OnceCell<Arc<crate::tracker::TrackerServices>>,
	pub tracker_tx: tokio::sync::OnceCell<crate::tracker::sync::TrackerSyncSender>,
	/// Readers, materializers and packagers hold a read lease; cache clearing holds the write lease.
	pub chapter_cache_gate: RwLock<()>,
	/// Per-chapter operations serialize refresh, image reads and whole-chapter packaging without
	/// blocking unrelated chapters behind one slow source.
	chapter_cache_locks: tokio::sync::Mutex<HashMap<i64, std::sync::Weak<RwLock<()>>>>,
	chapter_cache_builds: tokio::sync::Mutex<HashMap<i64, std::sync::Weak<tokio::sync::Mutex<()>>>>,
	/// Only one LRU collector may make decisions against the persistent byte total at a time.
	chapter_cache_collect: tokio::sync::Mutex<()>,
	chapter_variant_locks: tokio::sync::Mutex<ChapterVariantLocks>,
	chapter_original_locks: tokio::sync::Mutex<HashMap<ChapterOriginalKey, std::sync::Weak<OriginalPageFlight>>>,
	chapter_accounting_locks: tokio::sync::Mutex<HashMap<i64, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

pub struct DefaultsWrite {
	pub source_key: String,
	pub key: String,
	pub value: DefaultValue,
}

/// defaults.set fires synchronously on the actor thread; this only posts to the channel, persistence is batched to the DB by the writer task
pub struct ChanDefaultsSink {
	tx: mpsc::UnboundedSender<DefaultsWrite>,
}

impl DefaultsSink for ChanDefaultsSink {
	fn set(&self, source_key: &str, key: &str, value: &DefaultValue) {
		let write = DefaultsWrite {
			source_key: source_key.to_string(),
			key: key.to_string(),
			value: value.clone(),
		};
		if self.tx.send(write).is_err() {
			tracing::warn!(source = source_key, key, "defaults writer channel closed, value dropped");
		}
	}
}

pub struct CookieWrite {
	pub source_key: String,
	pub domain: String,
	pub name: String,
	/// None means delete this cookie
	pub value: Option<String>,
}

/// CookieJar fires synchronously on the actor thread; this only posts to the channel, persistence is batched to the DB by the writer task
pub struct ChanCookieSink {
	tx: mpsc::UnboundedSender<CookieWrite>,
	source_key: String,
}

impl CookieSink for ChanCookieSink {
	fn set(&self, domain: &str, name: &str, value: Option<&str>) {
		let write = CookieWrite {
			source_key: self.source_key.clone(),
			domain: domain.to_string(),
			name: name.to_string(),
			value: value.map(str::to_string),
		};
		if self.tx.send(write).is_err() {
			tracing::warn!(source = %self.source_key, domain, name, "cookie writer channel closed, value dropped");
		}
	}
}

pub async fn load_defaults(pool: &SqlitePool, source_id: i64) -> ApiResult<HashMap<String, DefaultValue>> {
	let rows: Vec<(String, String)> =
		sqlx::query_as("SELECT key, value FROM source_setting WHERE source_id = ?")
			.bind(source_id)
			.fetch_all(pool)
			.await?;
	let mut defaults = HashMap::new();
	for (key, raw) in rows {
		match serde_json::from_str::<DefaultValue>(&raw) {
			Ok(value) => {
				defaults.insert(key, value);
			}
			Err(e) => tracing::warn!(source_id, key, "skip unparsable source_setting: {e}"),
		}
	}
	Ok(defaults)
}

/// Backfills an instance's cookie jar: domain → (name → value)
pub async fn load_cookies(
	pool: &SqlitePool,
	source_id: i64,
) -> ApiResult<HashMap<String, HashMap<String, String>>> {
	let rows: Vec<(String, String, String)> =
		sqlx::query_as("SELECT domain, name, value FROM source_cookie WHERE source_id = ?")
			.bind(source_id)
			.fetch_all(pool)
			.await?;
	let mut cookies: HashMap<String, HashMap<String, String>> = HashMap::new();
	for (domain, name, value) in rows {
		cookies.entry(domain).or_default().insert(name, value);
	}
	Ok(cookies)
}

/// The columns of a source row needed to assemble an instance's startup config (key, wasm_path, kind, enabled, languages, config)
type SourceConfigRow = (
	String,
	String,
	String,
	bool,
	sqlx::types::Json<Vec<String>>,
	Option<String>,
);

impl AppState {
	pub fn new(
		pool: SqlitePool,
		config: Config,
		defaults_tx: mpsc::UnboundedSender<DefaultsWrite>,
		cookie_tx: mpsc::UnboundedSender<CookieWrite>,
	) -> Arc<Self> {
		let call_concurrency = config.call_concurrency;
		let policy = SumidokuNetPolicy::from_config(&config);
		let http = RoutedClients::new(policy.clone());
		let auth = crate::auth::AuthState::new(config.effective_username());
		Arc::new(Self {
			pool,
			config,
			auth,
			sources: RwLock::new(HashMap::new()),
			call_semaphore: Arc::new(Semaphore::new(call_concurrency)),
			defaults_tx,
			cookie_tx,
			http,
			policy,
			local_zip: Arc::new(crate::internal::local::ZipCache::new()),
			downloads: tokio::sync::OnceCell::new(),
			updater: tokio::sync::OnceCell::new(),
			events: tokio::sync::OnceCell::new(),
			tasks: tokio::sync::OnceCell::new(),
			tracker: tokio::sync::OnceCell::new(),
			tracker_tx: tokio::sync::OnceCell::new(),
			chapter_cache_gate: RwLock::new(()),
			chapter_cache_locks: tokio::sync::Mutex::new(HashMap::new()),
			chapter_cache_builds: tokio::sync::Mutex::new(HashMap::new()),
			chapter_cache_collect: tokio::sync::Mutex::new(()),
			chapter_variant_locks: tokio::sync::Mutex::new(HashMap::new()),
			chapter_original_locks: tokio::sync::Mutex::new(HashMap::new()),
			chapter_accounting_locks: tokio::sync::Mutex::new(HashMap::new()),
		})
	}

	/// build_app finishes assembly before the router serves, so it is necessarily in place by then
	pub fn downloads(&self) -> Arc<crate::download::DownloadManager> {
		self.downloads
			.get()
			.expect("download manager installed")
			.clone()
	}

	pub fn events(&self) -> Arc<crate::events::EventHub> {
		self.events.get().expect("event hub installed").clone()
	}

	pub fn tasks(&self) -> Arc<crate::task::TaskManager> {
		self.tasks.get().expect("task manager installed").clone()
	}

	/// Best-effort task kicks (analysis-on-open, rescan-on-settings-change) skip silently in harnesses that
	/// never installed the manager; the API surface uses tasks() instead, where build_app guarantees it
	pub fn maybe_tasks(&self) -> Option<Arc<crate::task::TaskManager>> {
		self.tasks.get().cloned()
	}

	/// Whether the source is the default local instance: its rows predate per-instance settings, so it
	/// keeps the legacy global/config fallback layers that other instances never had
	async fn is_default_local(&self, source_id: i64) -> ApiResult<bool> {
		let row: Option<(String,)> = sqlx::query_as("SELECT key FROM source WHERE id = ?")
			.bind(source_id)
			.fetch_optional(&self.pool)
			.await?;
		Ok(row.is_some_and(|(key,)| key == crate::model::LOCAL_SOURCE_KEY))
	}

	/// Effective scan roots of one local instance: the instance's own `paths` source setting wins; the
	/// default instance additionally falls back to the legacy global local.dirs setting, then the config
	/// layer, so pre-multi-instance deployments keep scanning the same roots until the per-instance
	/// setting is edited. A wrong-shaped stored value counts as unset, same rule as the backup.* settings
	pub async fn local_dirs(&self, source_id: i64) -> ApiResult<Vec<std::path::PathBuf>> {
		let row: Option<(String,)> =
			sqlx::query_as("SELECT value FROM source_setting WHERE source_id = ? AND key = 'paths'")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		if let Some(dirs) = row
			.and_then(|(raw,)| serde_json::from_str::<DefaultValue>(&raw).ok())
			.and_then(|value| match value {
				DefaultValue::StringArray(dirs) => Some(dirs),
				_ => None,
			})
			.map(|dirs| {
				dirs.into_iter()
					.filter(|dir| !dir.trim().is_empty())
					.map(std::path::PathBuf::from)
					.collect::<Vec<_>>()
			})
			&& !dirs.is_empty()
		{
			return Ok(dirs);
		}
		if self.is_default_local(source_id).await? {
			let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'local.dirs'")
				.fetch_optional(&self.pool)
				.await?;
			if let Some(dirs) = row
				.and_then(|(raw,)| serde_json::from_str::<Vec<String>>(&raw).ok())
				.map(|dirs| {
					dirs.into_iter()
						.filter(|dir| !dir.trim().is_empty())
						.map(std::path::PathBuf::from)
						.collect::<Vec<_>>()
				})
				&& !dirs.is_empty()
			{
				return Ok(dirs);
			}
			return Ok(self.config.local_dirs());
		}
		Ok(Vec::new())
	}

	/// Effective oneshots-directory marker of one local instance: the instance's own `oneshotsDir`
	/// source setting wins — including an explicit blank, which turns the rule off for that instance —
	/// and an absent setting falls back to the legacy global local.oneshots_dir (default instance only),
	/// then the config layer. A blank or wrong-shaped legacy value counts as unset, the pre-existing rule
	pub async fn local_oneshots_dir(&self, source_id: i64) -> ApiResult<Option<String>> {
		let row: Option<(String,)> =
			sqlx::query_as("SELECT value FROM source_setting WHERE source_id = ? AND key = 'oneshotsDir'")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		if let Some((raw,)) = row {
			return Ok(match serde_json::from_str::<DefaultValue>(&raw) {
				Ok(DefaultValue::String(name)) => {
					let name = name.trim().to_string();
					(!name.is_empty()).then_some(name)
				}
				_ => None,
			});
		}
		if self.is_default_local(source_id).await? {
			let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'local.oneshots_dir'")
				.fetch_optional(&self.pool)
				.await?;
			if let Some(name) = row
				.and_then(|(raw,)| serde_json::from_str::<String>(&raw).ok())
				.map(|name| name.trim().to_string())
				&& !name.is_empty()
			{
				return Ok(Some(name));
			}
		}
		Ok(self.config.local_oneshots_dir().map(str::to_string))
	}

	/// Effective exclusion substrings of one local instance (kmrs scan_directory_exclusions): the instance's
	/// own `exclusions` source setting wins — an explicit empty array turns the rule off for that instance —
	/// and an absent setting falls back to the config layer. A wrong-shaped stored value counts as unset.
	/// Values are lowercased once here: every consumer matches case-insensitively against a lowered path
	pub async fn local_exclusions(&self, source_id: i64) -> ApiResult<Vec<String>> {
		let row: Option<(String,)> =
			sqlx::query_as("SELECT value FROM source_setting WHERE source_id = ? AND key = 'exclusions'")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		if let Some((raw,)) = row
			&& let Ok(DefaultValue::StringArray(exclusions)) = serde_json::from_str::<DefaultValue>(&raw)
		{
			return Ok(exclusions
				.into_iter()
				.map(|exclusion| exclusion.trim().to_lowercase())
				.filter(|exclusion| !exclusion.is_empty())
				.collect());
		}
		Ok(self
			.config
			.local_scan_exclusions
			.iter()
			.map(|exclusion| exclusion.trim().to_lowercase())
			.filter(|exclusion| !exclusion.is_empty())
			.collect())
	}

	/// One boolean instance setting of the local source (forceModifiedTime / analyzePages / analyzeMetadata):
	/// the instance's own setting wins and an absent one falls back to the config layer. Bool is two-valued,
	/// so unlike text settings there is no explicit-blank third state: clearing the override is deleting the row
	async fn local_bool_setting(&self, source_id: i64, key: &str, fallback: bool) -> ApiResult<bool> {
		let row: Option<(String,)> = sqlx::query_as("SELECT value FROM source_setting WHERE source_id = ? AND key = ?")
			.bind(source_id)
			.bind(key)
			.fetch_optional(&self.pool)
			.await?;
		if let Some((raw,)) = row
			&& let Ok(DefaultValue::Bool(value)) = serde_json::from_str::<DefaultValue>(&raw)
		{
			return Ok(value);
		}
		Ok(fallback)
	}

	pub async fn local_force_modified_time(&self, source_id: i64) -> ApiResult<bool> {
		self.local_bool_setting(source_id, "forceModifiedTime", self.config.local_scan_force_modified_time)
			.await
	}

	pub async fn local_analyze_pages(&self, source_id: i64) -> ApiResult<bool> {
		self.local_bool_setting(source_id, "analyzePages", self.config.local_analyze_pages)
			.await
	}

	pub async fn local_analyze_metadata(&self, source_id: i64) -> ApiResult<bool> {
		self.local_bool_setting(source_id, "analyzeMetadata", self.config.local_analyze_metadata)
			.await
	}

	/// Kicks the instance out of the registry after unload/repackage; while an in-flight call holds an Arc, it exits naturally when the refcount drops to zero
	pub async fn evict_source(&self, source_id: i64) {
		let entry = self.sources.write().await.remove(&source_id);
		if let Some(entry) = entry
			&& let Ok(handle) = Arc::try_unwrap(entry.handle)
		{
			handle.shutdown().await;
		}
	}

	/// One pass of idle eviction: removes instances whose last_used exceeds max_idle from the registry. Idleness
	/// is re-checked under the write lock before removal (get_handle touches under the read lock, so the re-check
	/// always sees the fresh value); try_unwrap/shutdown happen outside the lock
	pub async fn evict_idle_sources(&self, max_idle: std::time::Duration) -> usize {
		let now = std::time::Instant::now();
		let candidates: Vec<i64> = self
			.sources
			.read()
			.await
			.iter()
			.filter(|(_, entry)| entry.idle_for(now) >= max_idle)
			.map(|(source_id, _)| *source_id)
			.collect();
		let mut evicted = 0;
		for source_id in candidates {
			let entry = {
				let mut registry = self.sources.write().await;
				match registry.get(&source_id) {
					Some(entry) if entry.idle_for(now) >= max_idle => registry.remove(&source_id),
					_ => None,
				}
			};
			let Some(entry) = entry else { continue };
			// Same as evict_source: while an in-flight call holds an Arc, it exits naturally when the refcount drops to zero
			if let Ok(handle) = Arc::try_unwrap(entry.handle) {
				handle.shutdown().await;
			}
			evicted += 1;
		}
		evicted
	}

	/// Memory-pressure eviction: evicts idlest-first by last_used, re-checking RSS after each one, until back under
	/// the threshold or nothing evictable remains. Under pressure the idle threshold counts as 0 — responses stream
	/// to disk and caches live on disk, so resident WASM instances are the only memory that can be returned.
	/// In-flight instances (Arc references outside the registry) are skipped: evicting them frees nothing, and the
	/// next access would spawn a fresh instance, briefly doubling the cost
	pub async fn evict_for_rss_pressure(
		&self,
		threshold_bytes: u64,
		rss: &mut (dyn FnMut() -> Option<u64> + Send),
	) -> usize {
		let mut evicted = 0;
		while rss().is_some_and(|bytes| bytes > threshold_bytes) {
			let now = std::time::Instant::now();
			let candidate = {
				let registry = self.sources.read().await;
				registry
					.iter()
					.filter(|(_, entry)| Arc::strong_count(&entry.handle) == 1)
					.max_by_key(|(_, entry)| entry.idle_for(now))
					.map(|(source_id, _)| *source_id)
			};
			let Some(source_id) = candidate else { break };
			let entry = {
				let mut registry = self.sources.write().await;
				// Re-check the refcount under the write lock: an instance hit by get_handle between the read-lock check and removal is not removed
				match registry.get(&source_id) {
					Some(entry) if Arc::strong_count(&entry.handle) == 1 => registry.remove(&source_id),
					_ => None,
				}
			};
			let Some(entry) = entry else { continue };
			// Once the refcount re-check passes, the registry reference has moved here, so try_unwrap must succeed
			if let Ok(handle) = Arc::try_unwrap(entry.handle) {
				handle.shutdown().await;
			}
			evicted += 1;
		}
		evicted
	}

	pub fn updater(&self) -> Arc<crate::update::UpdateScheduler> {
		self.updater
			.get()
			.expect("update scheduler installed")
			.clone()
	}

	/// Assembles an instance's startup config from the source row + source_setting + source_cookie;
	/// also converts manifest config.maximumParallelRequests into the per-source call admission cap
	async fn source_config(&self, source_id: i64) -> ApiResult<(SourceConfig, Option<Arc<Semaphore>>)> {
		let row: Option<SourceConfigRow> =
			sqlx::query_as("SELECT key, wasm_path, kind, enabled, languages, config FROM source WHERE id = ?")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		let Some((key, wasm_path, kind, enabled, languages, source_manifest_config)) = row else {
			return Err(ApiError::not_found("source not found"));
		};
		// Built-in sources (local/opds) have no wasm payload, so any capability that needs an instance gets 501 unsupported
		if kind == crate::model::SOURCE_KIND_LOCAL || kind == crate::model::SOURCE_KIND_OPDS {
			return Err(ApiError::unsupported_501("feature not supported by source"));
		}
		if !enabled {
			return Err(ApiError::source_disabled(&key));
		}
		let call_limit = source_call_limit(source_manifest_config.as_deref()).map(Semaphore::new);
		let defaults = load_defaults(&self.pool, source_id).await?;
		let cookies = load_cookies(&self.pool, source_id).await?;
		let accept_language =
			aidoku_wasm::default_accept_language(&preferred_languages(&defaults, &languages.0));
		let mut config = SourceConfig::new(&key, &wasm_path);
		config.limits = self.config.wasm_limits();
		config.defaults = defaults;
		config.cookies = cookies;
		config.policy = self.policy.clone();
		config.accept_language = accept_language;
		config.browser_trawl = self.config.net.browser_trawl.clone();
		config.sink = Arc::new(ChanDefaultsSink {
			tx: self.defaults_tx.clone(),
		});
		config.cookie_sink = Arc::new(ChanCookieSink {
			tx: self.cookie_tx.clone(),
			source_key: key.clone(),
		});
		Ok((config, call_limit.map(Arc::new)))
	}

	/// Lazy loading: when the registry has no entry, spin up the instance from the source row + source_setting
	pub async fn get_handle(self: &Arc<Self>, source_id: i64) -> ApiResult<Arc<SourceHandle>> {
		if let Some(entry) = self.sources.read().await.get(&source_id) {
			// A hit refreshes last_used; stamped under the read lock, so the sweeper's write-lock re-check always sees the fresh value
			entry.touch();
			return Ok(entry.handle.clone());
		}
		let (config, call_limit) = self.source_config(source_id).await?;
		let key = config.key.clone();
		let (handle, features) =
			SourceHandle::spawn_with_source_limit(config, self.call_semaphore.clone(), call_limit.clone())
				.await?;
		let handle = Arc::new(handle);
		let mut registry = self.sources.write().await;
		match registry.entry(source_id) {
			std::collections::hash_map::Entry::Occupied(e) => {
				// Concurrent lazy-load collision: keep the first arrival and drop the later instance outright (dropping the Arc exits the actor thread)
				Ok(e.get().handle.clone())
			}
			std::collections::hash_map::Entry::Vacant(v) => {
				v.insert(SourceEntry::new(handle.clone(), key, features, call_limit));
				Ok(handle)
			}
		}
	}

	pub async fn chapter_cache_lock(&self, chapter_id: i64) -> Arc<RwLock<()>> {
		let mut locks = self.chapter_cache_locks.lock().await;
		if let Some(lock) = locks.get(&chapter_id).and_then(std::sync::Weak::upgrade) {
			return lock;
		}
		locks.retain(|_, lock| lock.strong_count() > 0);
		let lock = Arc::new(RwLock::new(()));
		locks.insert(chapter_id, Arc::downgrade(&lock));
		lock
	}

	pub async fn chapter_cache_build_lock(&self, chapter_id: i64) -> Arc<tokio::sync::Mutex<()>> {
		let mut locks = self.chapter_cache_builds.lock().await;
		if let Some(lock) = locks.get(&chapter_id).and_then(std::sync::Weak::upgrade) {
			return lock;
		}
		locks.retain(|_, lock| lock.strong_count() > 0);
		let lock = Arc::new(tokio::sync::Mutex::new(()));
		locks.insert(chapter_id, Arc::downgrade(&lock));
		lock
	}

	pub async fn chapter_variant_lock(&self, chapter_id: i64, index: i64, bucket: u32) -> Arc<tokio::sync::Mutex<()>> {
		let key = (chapter_id, index, bucket);
		let mut locks = self.chapter_variant_locks.lock().await;
		if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) { return lock; }
		locks.retain(|_, lock| lock.strong_count() > 0);
		let lock = Arc::new(tokio::sync::Mutex::new(()));
		locks.insert(key, Arc::downgrade(&lock));
		lock
	}

	pub async fn chapter_original_lock(&self, chapter_id: i64, index: i64) -> Arc<OriginalPageFlight> {
		let key = (chapter_id, index);
		let mut locks = self.chapter_original_locks.lock().await;
		if let Some(flight) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
			return flight;
		}
		locks.retain(|_, lock| lock.strong_count() > 0);
		let flight = Arc::new(OriginalPageFlight { lock: tokio::sync::Mutex::new(()), result: std::sync::Mutex::new(None) });
		locks.insert(key, Arc::downgrade(&flight));
		flight
	}

	pub async fn chapter_accounting_lock(&self, chapter_id: i64) -> Arc<tokio::sync::Mutex<()>> {
		let mut locks = self.chapter_accounting_locks.lock().await;
		if let Some(lock) = locks.get(&chapter_id).and_then(std::sync::Weak::upgrade) { return lock; }
		locks.retain(|_, lock| lock.strong_count() > 0);
		let lock = Arc::new(tokio::sync::Mutex::new(()));
		locks.insert(chapter_id, Arc::downgrade(&lock));
		lock
	}

	pub fn try_chapter_cache_collect(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
		self.chapter_cache_collect.try_lock().ok()
	}
}

/// Wake granularity of the idle sweep; thresholds default to half an hour, so minute-level drift is imperceptible
pub const IDLE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Periodic sweep: evicts source instances idle past max_idle from the registry (WASM linear memory only grows,
/// so eviction is the only way to return an instance's memory to the OS)
pub async fn run_idle_eviction(
	state: Arc<AppState>,
	max_idle: std::time::Duration,
	sweep_interval: std::time::Duration,
) {
	let mut ticker = tokio::time::interval(sweep_interval);
	// interval's first tick completes immediately; the first sweep waits a full period
	ticker.tick().await;
	loop {
		ticker.tick().await;
		let evicted = state.evict_idle_sources(max_idle).await;
		if evicted > 0 {
			tracing::info!(evicted, "evicted idle source instances");
		}
	}
}

/// RSS monitor: an independent task on the same cadence as the idle sweep. Over threshold it evicts idlest-first;
/// if still over after eviction it only logs loudly — restarting the process is an operator decision, the monitor never does it
pub async fn run_rss_monitor(
	state: Arc<AppState>,
	threshold_bytes: u64,
	sweep_interval: std::time::Duration,
	mut rss: impl FnMut() -> Option<u64> + Send,
) {
	const MIB: u64 = 1024 * 1024;
	let mut ticker = tokio::time::interval(sweep_interval);
	// interval's first tick completes immediately; the first sweep waits a full period
	ticker.tick().await;
	loop {
		ticker.tick().await;
		let Some(before) = rss() else { continue };
		if before <= threshold_bytes {
			continue;
		}
		let evicted = state.evict_for_rss_pressure(threshold_bytes, &mut rss).await;
		let after = rss();
		let still_over = after.is_some_and(|bytes| bytes > threshold_bytes);
		tracing::warn!(
			rss_before_mb = before / MIB,
			rss_after_mb = after.map(|bytes| bytes / MIB),
			threshold_mb = threshold_bytes / MIB,
			evicted,
			"RSS over threshold, evicted idle source instances by memory pressure"
		);
		if still_over {
			let resident = state.sources.read().await.len();
			tracing::warn!(
				threshold_mb = threshold_bytes / MIB,
				resident,
				"RSS still over threshold with nothing idle to evict; restart left to operator"
			);
		}
	}
}

/// Background writer: batch-upserts defaults changes posted by the sink into source_setting
pub async fn run_defaults_writer(pool: SqlitePool, mut rx: mpsc::UnboundedReceiver<DefaultsWrite>) {
	while let Some(first) = rx.recv().await {
		let mut batch = vec![first];
		while batch.len() < 64 {
			match rx.try_recv() {
				Ok(write) => batch.push(write),
				Err(_) => break,
			}
		}
		let result: Result<(), sqlx::Error> = async {
			let mut tx = crate::db::begin_write(&pool).await?;
			for write in batch {
				let source_id: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
					.bind(&write.source_key)
					.fetch_optional(&mut *tx)
					.await?;
				let Some((source_id,)) = source_id else {
					tracing::warn!(source = write.source_key, "defaults write for unknown source, dropped");
					continue;
				};
				let value = serde_json::to_string(&write.value)
					.map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
				sqlx::query(
					"INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)
					 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
				)
				.bind(source_id)
				.bind(&write.key)
				.bind(value)
				.execute(&mut *tx)
				.await?;
			}
			tx.commit().await
		}
		.await;
		if let Err(e) = result {
			tracing::warn!("defaults writer batch failed: {e}");
		}
	}
}

/// Language source for Accept-Language: for multi-language sources prefer the seeded `languages`/`language`
/// source settings (user-editable entries created by `source/mod.rs:seed_source_defaults`); only fall back to
/// the manifest's language list when absent. Aidoku uses the device locale; the server has no device state,
/// so the source language setting is the closest substitute.
fn preferred_languages(
	defaults: &HashMap<String, DefaultValue>,
	manifest: &[String],
) -> Vec<String> {
	if let Some(DefaultValue::StringArray(langs)) = defaults.get("languages")
		&& !langs.is_empty()
	{
		return langs.to_vec();
	}
	if let Some(DefaultValue::String(lang)) = defaults.get("language")
		&& !lang.is_empty()
	{
		return vec![lang.to_string()];
	}
	manifest.to_vec()
}

/// Converts maximumParallelRequests into the per-source concurrency cap: honored when declared, at least 1
/// (Aidoku takes max(1, min(config, default)), matching the floor semantics after stacking with the global cap);
/// undeclared/invalid → None, only the global cap applies
fn source_call_limit(manifest_config: Option<&str>) -> Option<usize> {
	let value: serde_json::Value = serde_json::from_str(manifest_config?).ok()?;
	let n = value.get("maximumParallelRequests")?.as_i64()?;
	Some(n.max(1) as usize)
}

/// Background writer: batch-upserts/deletes cookie changes posted by the sink into source_cookie
pub async fn run_cookie_writer(pool: SqlitePool, mut rx: mpsc::UnboundedReceiver<CookieWrite>) {
	while let Some(first) = rx.recv().await {
		let mut batch = vec![first];
		while batch.len() < 64 {
			match rx.try_recv() {
				Ok(write) => batch.push(write),
				Err(_) => break,
			}
		}
		let result: Result<(), sqlx::Error> = async {
			let mut tx = crate::db::begin_write(&pool).await?;
			for write in batch {
				let source_id: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
					.bind(&write.source_key)
					.fetch_optional(&mut *tx)
					.await?;
				let Some((source_id,)) = source_id else {
					tracing::warn!(source = write.source_key, "cookie write for unknown source, dropped");
					continue;
				};
				match write.value {
					Some(value) => {
						sqlx::query(
							"INSERT INTO source_cookie (source_id, domain, name, value) VALUES (?, ?, ?, ?)
							 ON CONFLICT (source_id, domain, name) DO UPDATE SET value = excluded.value",
						)
						.bind(source_id)
						.bind(&write.domain)
						.bind(&write.name)
						.bind(value)
						.execute(&mut *tx)
						.await?;
					}
					None => {
						sqlx::query(
							"DELETE FROM source_cookie WHERE source_id = ? AND domain = ? AND name = ?",
						)
						.bind(source_id)
						.bind(&write.domain)
						.bind(&write.name)
						.execute(&mut *tx)
						.await?;
					}
				}
			}
			tx.commit().await
		}
		.await;
		if let Err(e) = result {
			tracing::warn!("cookie writer batch failed: {e}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn strings(values: &[&str]) -> Vec<String> {
		values.iter().map(|v| v.to_string()).collect()
	}

	#[test]
	fn preferred_languages_prefers_the_source_setting() {
		let mut defaults = HashMap::new();
		defaults.insert("languages".into(), DefaultValue::StringArray(strings(&["zh"])));
		assert_eq!(preferred_languages(&defaults, &strings(&["en"])), strings(&["zh"]));
	}

	#[test]
	fn preferred_languages_reads_a_single_language_setting() {
		let mut defaults = HashMap::new();
		defaults.insert("language".into(), DefaultValue::String("ja".into()));
		assert_eq!(preferred_languages(&defaults, &strings(&["en"])), strings(&["ja"]));
	}

	#[test]
	fn preferred_languages_ignores_an_empty_setting() {
		let mut defaults = HashMap::new();
		defaults.insert("languages".into(), DefaultValue::StringArray(Vec::new()));
		assert_eq!(preferred_languages(&defaults, &strings(&["vi"])), strings(&["vi"]));
	}

	#[test]
	fn preferred_languages_falls_back_to_the_manifest() {
		assert_eq!(
			preferred_languages(&HashMap::new(), &strings(&["ko"])),
			strings(&["ko"])
		);
	}

	use std::path::PathBuf;
	use std::time::{Duration, Instant};

	fn fixture_wasm() -> String {
		PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("../wasm/tests/fixtures/example/main.wasm")
			.display()
			.to_string()
	}

	async fn test_state(config: Config) -> (Arc<AppState>, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, wasm_path) VALUES ('en.example-source', 'Example', ?) RETURNING id",
		)
		.bind(fixture_wasm())
		.fetch_one(&pool)
		.await
		.expect("insert source");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		(AppState::new(pool, config, defaults_tx, cookie_tx), source_id, tmp)
	}

	async fn test_state_multi(config: Config, count: usize) -> (Arc<AppState>, Vec<i64>, tempfile::TempDir) {
		let (state, first, tmp) = test_state(config).await;
		let mut ids = vec![first];
		for i in 1..count {
			let source_id: i64 = sqlx::query_scalar(
				"INSERT INTO source (key, name, wasm_path) VALUES (?, 'Example', ?) RETURNING id",
			)
			.bind(format!("en.example-source-{i}"))
			.bind(fixture_wasm())
			.fetch_one(&state.pool)
			.await
			.expect("insert source");
			ids.push(source_id);
		}
		(state, ids, tmp)
	}

	/// Shifts a registry entry's last_used into the past, avoiding a real wait
	async fn make_idle(state: &AppState, source_id: i64, idle: Duration) {
		let registry = state.sources.read().await;
		let entry = registry.get(&source_id).expect("entry");
		*entry.last_used.lock().expect("last_used lock") = Instant::now()
			.checked_sub(idle)
			.expect("host uptime shorter than idle shift");
	}

	#[tokio::test]
	async fn idle_sweep_evicts_instances_past_the_threshold() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		assert_eq!(state.sources.read().await.len(), 1);
		make_idle(&state, source_id, Duration::from_secs(31 * 60)).await;
		let evicted = state.evict_idle_sources(Duration::from_secs(30 * 60)).await;
		assert_eq!(evicted, 1);
		assert!(state.sources.read().await.is_empty());
	}

	#[tokio::test]
	async fn idle_sweep_keeps_instances_used_within_the_threshold() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		make_idle(&state, source_id, Duration::from_secs(20 * 60)).await;
		let evicted = state.evict_idle_sources(Duration::from_secs(30 * 60)).await;
		assert_eq!(evicted, 0);
		assert_eq!(state.sources.read().await.len(), 1);
	}

	#[tokio::test]
	async fn get_handle_hit_refreshes_last_used() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		make_idle(&state, source_id, Duration::from_secs(40 * 60)).await;
		// A hit must refresh last_used, otherwise this instance idle for 40 minutes would be evicted by mistake
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		let evicted = state.evict_idle_sources(Duration::from_secs(30 * 60)).await;
		assert_eq!(evicted, 0);
		assert_eq!(state.sources.read().await.len(), 1);
	}

	#[tokio::test]
	async fn evicted_source_is_rebuilt_on_next_access() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let first = state.get_handle(source_id).await.expect("handle");
		drop(first);
		make_idle(&state, source_id, Duration::from_secs(31 * 60)).await;
		state.evict_idle_sources(Duration::from_secs(30 * 60)).await;
		let second = state.get_handle(source_id).await.expect("handle");
		assert!(second.is_alive());
		assert_eq!(state.sources.read().await.len(), 1);
		// The rebuilt instance genuinely runs, not just sits in the registry
		let listings = second.get_listings().await.expect("listings");
		assert!(!listings.is_empty());
	}

	#[tokio::test]
	async fn idle_sweep_leaves_in_flight_instances_running() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let in_flight = state.get_handle(source_id).await.expect("handle");
		make_idle(&state, source_id, Duration::from_secs(31 * 60)).await;
		let evicted = state.evict_idle_sources(Duration::from_secs(30 * 60)).await;
		assert_eq!(evicted, 1);
		assert!(state.sources.read().await.is_empty());
		// The instance held by the in-flight call is not shutdown; the registry let go, and it exits when the refcount drops to zero
		assert!(in_flight.is_alive());
		assert_eq!(Arc::strong_count(&in_flight), 1);
		// The next access spawns a fresh instance without waiting for the old one to exit
		let second = state.get_handle(source_id).await.expect("handle");
		assert!(!Arc::ptr_eq(&in_flight, &second));
	}

	#[tokio::test]
	async fn sweep_task_evicts_on_its_own_schedule() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		make_idle(&state, source_id, Duration::from_secs(31 * 60)).await;
		let task = tokio::spawn(run_idle_eviction(
			state.clone(),
			Duration::from_secs(30 * 60),
			Duration::from_millis(20),
		));
		let mut swept = false;
		for _ in 0..250 {
			if state.sources.read().await.is_empty() {
				swept = true;
				break;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
		task.abort();
		assert!(swept);
	}

	#[tokio::test]
	async fn rss_pressure_evicts_only_until_rss_recovers() {
		let (state, ids, _tmp) = test_state_multi(Config::default(), 3).await;
		for id in &ids {
			let handle = state.get_handle(*id).await.expect("handle");
			drop(handle);
		}
		make_idle(&state, ids[0], Duration::from_secs(40 * 60)).await;
		make_idle(&state, ids[1], Duration::from_secs(20 * 60)).await;
		make_idle(&state, ids[2], Duration::from_secs(10 * 60)).await;
		// Reading sequence: over → evict one → recovered. Exactly one eviction, and it must be the idlest ids[0]
		let mut readings = vec![150_u64, 50].into_iter();
		let evicted = state.evict_for_rss_pressure(100, &mut || readings.next()).await;
		assert_eq!(evicted, 1);
		{
			let registry = state.sources.read().await;
			assert!(!registry.contains_key(&ids[0]));
			assert!(registry.contains_key(&ids[1]));
			assert!(registry.contains_key(&ids[2]));
		}
		// Re-accessing the evicted source spawns a fresh instance
		let rebuilt = state.get_handle(ids[0]).await.expect("handle");
		assert!(rebuilt.is_alive());
	}

	#[tokio::test]
	async fn rss_pressure_keeps_evicting_while_over_threshold() {
		let (state, ids, _tmp) = test_state_multi(Config::default(), 3).await;
		for id in &ids {
			let handle = state.get_handle(*id).await.expect("handle");
			drop(handle);
		}
		make_idle(&state, ids[0], Duration::from_secs(40 * 60)).await;
		make_idle(&state, ids[1], Duration::from_secs(20 * 60)).await;
		make_idle(&state, ids[2], Duration::from_secs(10 * 60)).await;
		// Over → over → recovered: the two idlest are evicted, the most recently used is kept
		let mut readings = vec![150_u64, 150, 50].into_iter();
		let evicted = state.evict_for_rss_pressure(100, &mut || readings.next()).await;
		assert_eq!(evicted, 2);
		let registry = state.sources.read().await;
		assert!(!registry.contains_key(&ids[0]));
		assert!(!registry.contains_key(&ids[1]));
		assert!(registry.contains_key(&ids[2]));
	}

	#[tokio::test]
	async fn rss_pressure_leaves_in_flight_instances_untouched() {
		let (state, ids, _tmp) = test_state_multi(Config::default(), 2).await;
		let in_flight = state.get_handle(ids[0]).await.expect("handle");
		let other = state.get_handle(ids[1]).await.expect("handle");
		drop(other);
		// The in-flight one happens to be idlest: pressure eviction should skip it and evict the next-idlest, not pull it out of the registry
		make_idle(&state, ids[0], Duration::from_secs(40 * 60)).await;
		make_idle(&state, ids[1], Duration::from_secs(20 * 60)).await;
		let mut readings = vec![150_u64, 50].into_iter();
		let evicted = state.evict_for_rss_pressure(100, &mut || readings.next()).await;
		assert_eq!(evicted, 1);
		let registry = state.sources.read().await;
		assert!(registry.contains_key(&ids[0]));
		assert!(!registry.contains_key(&ids[1]));
		drop(registry);
		assert!(in_flight.is_alive());
	}

	#[tokio::test]
	async fn rss_pressure_noop_under_threshold() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		make_idle(&state, source_id, Duration::from_secs(40 * 60)).await;
		let mut readings = vec![50_u64].into_iter();
		let evicted = state.evict_for_rss_pressure(100, &mut || readings.next()).await;
		assert_eq!(evicted, 0);
		assert_eq!(state.sources.read().await.len(), 1);
	}

	#[tokio::test]
	async fn rss_pressure_stops_when_nothing_is_evictable() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let in_flight = state.get_handle(source_id).await.expect("handle");
		// RSS stays over and the only instance is in flight: the loop must exit rather than spin
		let evicted = state.evict_for_rss_pressure(100, &mut || Some(150_u64)).await;
		assert_eq!(evicted, 0);
		assert!(in_flight.is_alive());
		assert_eq!(state.sources.read().await.len(), 1);
	}

	#[tokio::test]
	async fn rss_monitor_task_evicts_when_threshold_crossed() {
		use std::sync::atomic::{AtomicU64, Ordering};
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		let reading = Arc::new(AtomicU64::new(50));
		let rss = {
			let reading = reading.clone();
			move || Some(reading.load(Ordering::Relaxed))
		};
		let task = tokio::spawn(run_rss_monitor(state.clone(), 100, Duration::from_millis(20), rss));
		// A few cycles under the threshold must not touch the registry
		tokio::time::sleep(Duration::from_millis(110)).await;
		assert_eq!(state.sources.read().await.len(), 1);
		reading.store(150, Ordering::Relaxed);
		let mut swept = false;
		for _ in 0..250 {
			if state.sources.read().await.is_empty() {
				swept = true;
				break;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
		task.abort();
		assert!(swept);
	}

	#[tokio::test]
	async fn source_config_applies_configured_limits() {
		let config = Config {
			wasm_memory_max_mb: 64,
			wasm_fuel: 123_456,
			wasm_call_timeout_secs: 7,
			..Config::default()
		};
		let (state, source_id, _tmp) = test_state(config).await;
		let (source_config, call_limit) = state.source_config(source_id).await.expect("source config");
		assert_eq!(source_config.limits.memory_max, 64 * 1024 * 1024);
		assert_eq!(source_config.limits.fuel, 123_456);
		assert_eq!(source_config.limits.call_timeout, Duration::from_secs(7));
		assert!(call_limit.is_none());
	}

	#[test]
	fn source_call_limit_reads_the_manifest_config() {
		assert_eq!(
			source_call_limit(Some(r#"{"maximumParallelRequests": 3}"#)),
			Some(3)
		);
		// A declared 0/negative is clamped to 1 per Aidoku's max(1, …)
		assert_eq!(
			source_call_limit(Some(r#"{"maximumParallelRequests": 0}"#)),
			Some(1)
		);
		assert_eq!(source_call_limit(Some(r#"{"other": 1}"#)), None);
		assert_eq!(source_call_limit(Some("not json")), None);
		assert_eq!(source_call_limit(None), None);
	}

	#[tokio::test]
	async fn get_handle_wires_the_declared_call_limit() {
		let (state, source_id, _tmp) = test_state(Config::default()).await;
		sqlx::query("UPDATE source SET config = ? WHERE id = ?")
			.bind(r#"{"maximumParallelRequests": 2}"#)
			.bind(source_id)
			.execute(&state.pool)
			.await
			.expect("set config");
		let handle = state.get_handle(source_id).await.expect("handle");
		drop(handle);
		let registry = state.sources.read().await;
		let limit = registry
			.get(&source_id)
			.expect("entry")
			.call_limit
			.as_ref()
			.expect("per-source call limit");
		assert_eq!(limit.available_permits(), 2);
	}
}
