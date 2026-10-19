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

pub struct SourceEntry {
	pub handle: Arc<SourceHandle>,
	pub key: String,
	pub features: SourceFeatures,
	/// manifest config.maximumParallelRequests 换算的每源调用准入帽；None 只受全局帽
	pub call_limit: Option<Arc<Semaphore>>,
	/// 最近一次 get_handle 命中的时刻；空闲逐出的判据
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
	// build_app 装配时一次性注入，晚于 AppState::new（manager 反过来依赖 state）
	pub downloads: tokio::sync::OnceCell<Arc<crate::download::DownloadManager>>,
	pub updater: tokio::sync::OnceCell<Arc<crate::update::UpdateScheduler>>,
	pub browser_logins: tokio::sync::OnceCell<crate::browser_login::BrowserLoginManager>,
}

pub struct DefaultsWrite {
	pub source_key: String,
	pub key: String,
	pub value: DefaultValue,
}

/// defaults.set 在 actor 线程同步触发，这里只投 channel，持久化由 writer task 批量落库
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
	/// None 表示删除该 cookie
	pub value: Option<String>,
}

/// CookieJar 在 actor 线程同步触发，这里只投 channel，持久化由 writer task 批量落库
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

/// 回填实例 cookie jar：domain → (name → value)
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

/// source 行里组装实例启动配置所需的列（key, wasm_path, enabled, languages, config）
type SourceConfigRow = (String, String, bool, sqlx::types::Json<Vec<String>>, Option<String>);

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
		let auth = crate::auth::AuthState::new(&config.auth.api_keys);
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
			downloads: tokio::sync::OnceCell::new(),
			updater: tokio::sync::OnceCell::new(),
			browser_logins: tokio::sync::OnceCell::new(),
		})
	}

	/// build_app 在路由对外服务前完成装配，此后必然已就位
	pub fn downloads(&self) -> Arc<crate::download::DownloadManager> {
		self.downloads
			.get()
			.expect("download manager installed")
			.clone()
	}

	/// 卸载/换包后踢掉注册表里的实例；有在途调用持有 Arc 时让位给引用计数归零后自然退出
	pub async fn evict_source(&self, source_id: i64) {
		let entry = self.sources.write().await.remove(&source_id);
		if let Some(entry) = entry
			&& let Ok(handle) = Arc::try_unwrap(entry.handle)
		{
			handle.shutdown().await;
		}
	}

	/// 空闲逐出单趟：last_used 超过 max_idle 的实例移出注册表。写锁内复查空闲才移除
	/// （get_handle 持读锁 touch，复查必见新值），try_unwrap/shutdown 在锁外
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
			// 与 evict_source 同理：在途调用持 Arc 时等引用计数归零自然退出
			if let Ok(handle) = Arc::try_unwrap(entry.handle) {
				handle.shutdown().await;
			}
			evicted += 1;
		}
		evicted
	}

	/// 内存压力逐出：按 last_used 从闲到忙逐个逐出，每逐一个复查 RSS，回到阈值内
	/// 或无可逐实例为止。压力下 idle 阈值视为 0——响应流式/落盘、缓存都在磁盘，
	/// 常驻 wasmer 实例是唯一可还的内存。在途实例（注册表外还有 Arc 引用）跳过不摘：
	/// 摘了还不了内存，下次访问还会另起实例，短时反而双份
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
				// 写锁内复查引用数：读锁检查到摘表之间被 get_handle 命中的实例不摘
				match registry.get(&source_id) {
					Some(entry) if Arc::strong_count(&entry.handle) == 1 => registry.remove(&source_id),
					_ => None,
				}
			};
			let Some(entry) = entry else { continue };
			// 引用数复查过关后注册表引用已移至此处，try_unwrap 必然成功
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

	/// 从 source 行 + source_setting + source_cookie 组装实例启动配置；
	/// 顺带把 manifest config.maximumParallelRequests 换算成每源调用准入帽
	async fn source_config(&self, source_id: i64) -> ApiResult<(SourceConfig, Option<Arc<Semaphore>>)> {
		let row: Option<SourceConfigRow> =
			sqlx::query_as("SELECT key, wasm_path, enabled, languages, config FROM source WHERE id = ?")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		let Some((key, wasm_path, enabled, languages, source_manifest_config)) = row else {
			return Err(ApiError::not_found("source not found"));
		};
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
		config.browser_ws = self.config.net.browser_ws.clone();
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

	/// 懒加载：注册表没有就从 source 行 + source_setting 拉起实例
	pub async fn get_handle(self: &Arc<Self>, source_id: i64) -> ApiResult<Arc<SourceHandle>> {
		if let Some(entry) = self.sources.read().await.get(&source_id) {
			// 命中即刷新 last_used；持读锁打点，清扫线程的写锁复查必然看到新值
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
				// 并发懒加载撞车：留先到者，后来的实例直接丢掉（Arc 落下 actor 线程即退出）
				Ok(e.get().handle.clone())
			}
			std::collections::hash_map::Entry::Vacant(v) => {
				v.insert(SourceEntry::new(handle.clone(), key, features, call_limit));
				Ok(handle)
			}
		}
	}
}

/// 空闲清扫的唤醒粒度；阈值默认半小时，分钟级漂移无感
pub const IDLE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// 周期清扫：把空闲超过 max_idle 的源实例逐出注册表（wasmer 线性内存只涨不消，
/// 逐出是唯一把实例内存还给 OS 的途径）
pub async fn run_idle_eviction(
	state: Arc<AppState>,
	max_idle: std::time::Duration,
	sweep_interval: std::time::Duration,
) {
	let mut ticker = tokio::time::interval(sweep_interval);
	// interval 首 tick 立即完成，首扫等满一个周期
	ticker.tick().await;
	loop {
		ticker.tick().await;
		let evicted = state.evict_idle_sources(max_idle).await;
		if evicted > 0 {
			tracing::info!(evicted, "evicted idle source instances");
		}
	}
}

/// RSS 监控：与空闲清扫同频的独立任务。超阈值按最闲优先逐出，逐完仍超只大声记日志
/// ——重启进程是运维决定，监控不自动做
pub async fn run_rss_monitor(
	state: Arc<AppState>,
	threshold_bytes: u64,
	sweep_interval: std::time::Duration,
	mut rss: impl FnMut() -> Option<u64> + Send,
) {
	const MIB: u64 = 1024 * 1024;
	let mut ticker = tokio::time::interval(sweep_interval);
	// interval 首 tick 立即完成，首扫等满一个周期
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

/// 后台 writer：把 sink 投递的 defaults 变更批量 upsert 进 source_setting
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
			let mut tx = pool.begin().await?;
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

/// Accept-Language 的语言来源：多语言源优先取播种的 `languages`/`language`
/// 源设置（`source/mod.rs:seed_source_defaults` 建的用户可改项），没有才退回
/// manifest 的语言列表。Aidoku 用设备 locale，服务端没有设备态，源语言设置
/// 是最接近的替代。
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

/// maximumParallelRequests 换算每源并发帽：声明即生效、至少 1（Aidoku 取
/// max(1, min(config, 默认))，与全局帽叠加后下限语义一致）；未声明/非法为 None，只受全局帽
fn source_call_limit(manifest_config: Option<&str>) -> Option<usize> {
	let value: serde_json::Value = serde_json::from_str(manifest_config?).ok()?;
	let n = value.get("maximumParallelRequests")?.as_i64()?;
	Some(n.max(1) as usize)
}

/// 后台 writer：把 sink 投递的 cookie 变更批量 upsert/delete 进 source_cookie
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
			let mut tx = pool.begin().await?;
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

	/// 把注册表项的 last_used 拨回过去，免去真实等待
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
		// 命中即刷新 last_used，否则这次空闲 40 分钟的实例会被误逐
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
		// 重建的实例真能跑，不是只挂进注册表
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
		// 在飞调用持有的实例不被 shutdown，注册表已放手，引用计数归零才退出
		assert!(in_flight.is_alive());
		assert_eq!(Arc::strong_count(&in_flight), 1);
		// 下一次访问另起新实例，不等旧实例退出
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
		// 读数序列：超 → 逐一个 → 回落。只逐一个且必须是最闲的 ids[0]
		let mut readings = vec![150_u64, 50].into_iter();
		let evicted = state.evict_for_rss_pressure(100, &mut || readings.next()).await;
		assert_eq!(evicted, 1);
		{
			let registry = state.sources.read().await;
			assert!(!registry.contains_key(&ids[0]));
			assert!(registry.contains_key(&ids[1]));
			assert!(registry.contains_key(&ids[2]));
		}
		// 被逐源再访问另起新实例
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
		// 超 → 超 → 回落：两个最闲的都被逐，最新使用的保留
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
		// 在途的恰好最闲：压力逐出应跳过它去逐次闲的，而不是把它摘出注册表
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
		// RSS 一直超、唯一实例又在途：必须退出循环而不是空转
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
		// 阈值内几个周期不动注册表
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
		// 声明 0/负数按 Aidoku 的 max(1, …) 钳到 1
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
