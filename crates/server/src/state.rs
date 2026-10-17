use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::routed::{RoutedClients, SumidokuNetPolicy};
use aidoku_wasm::{
	DefaultValue, DefaultsSink, SourceConfig, SourceFeatures, SourceHandle,
};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, mpsc};

pub struct SourceEntry {
	pub handle: Arc<SourceHandle>,
	pub key: String,
	pub features: SourceFeatures,
}

pub struct AppState {
	pub pool: SqlitePool,
	pub config: Config,
	pub sources: RwLock<HashMap<i64, SourceEntry>>,
	pub call_semaphore: Arc<Semaphore>,
	pub defaults_tx: mpsc::UnboundedSender<DefaultsWrite>,
	pub http: RoutedClients,
	pub policy: Arc<SumidokuNetPolicy>,
	// build_app 装配时一次性注入，晚于 AppState::new（manager 反过来依赖 state）
	pub downloads: tokio::sync::OnceCell<Arc<crate::download::DownloadManager>>,
	pub updater: tokio::sync::OnceCell<Arc<crate::update::UpdateScheduler>>,
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

impl AppState {
	pub fn new(
		pool: SqlitePool,
		config: Config,
		defaults_tx: mpsc::UnboundedSender<DefaultsWrite>,
	) -> Arc<Self> {
		let call_concurrency = config.call_concurrency;
		let policy = SumidokuNetPolicy::from_config(&config);
		let http = RoutedClients::new(policy.clone());
		Arc::new(Self {
			pool,
			config,
			sources: RwLock::new(HashMap::new()),
			call_semaphore: Arc::new(Semaphore::new(call_concurrency)),
			defaults_tx,
			http,
			policy,
			downloads: tokio::sync::OnceCell::new(),
			updater: tokio::sync::OnceCell::new(),
		})
	}

	/// build_app 在路由对外服务前完成装配，此后必然已就位
	pub fn downloads(&self) -> Arc<crate::download::DownloadManager> {
		self.downloads
			.get()
			.expect("download manager installed")
			.clone()
	}

	pub fn updater(&self) -> Arc<crate::update::UpdateScheduler> {
		self.updater
			.get()
			.expect("update scheduler installed")
			.clone()
	}

	/// 懒加载：注册表没有就从 source 行 + source_setting 拉起实例
	pub async fn get_handle(self: &Arc<Self>, source_id: i64) -> ApiResult<Arc<SourceHandle>> {
		if let Some(entry) = self.sources.read().await.get(&source_id) {
			return Ok(entry.handle.clone());
		}
		let row: Option<(String, String, bool)> =
			sqlx::query_as("SELECT key, wasm_path, enabled FROM source WHERE id = ?")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await?;
		let Some((key, wasm_path, enabled)) = row else {
			return Err(ApiError::not_found("source not found"));
		};
		if !enabled {
			return Err(ApiError::source_disabled(&key));
		}
		let defaults = load_defaults(&self.pool, source_id).await?;
		let mut config = SourceConfig::new(&key, &wasm_path);
		config.defaults = defaults;
		config.policy = self.policy.clone();
		config.sink = Arc::new(ChanDefaultsSink {
			tx: self.defaults_tx.clone(),
		});
		let (handle, features) = SourceHandle::spawn(config, self.call_semaphore.clone()).await?;
		let handle = Arc::new(handle);
		let mut registry = self.sources.write().await;
		match registry.entry(source_id) {
			std::collections::hash_map::Entry::Occupied(e) => {
				// 并发懒加载撞车：留先到者，后来的实例直接丢掉（Arc 落下 actor 线程即退出）
				Ok(e.get().handle.clone())
			}
			std::collections::hash_map::Entry::Vacant(v) => {
				v.insert(SourceEntry {
					handle: handle.clone(),
					key,
					features,
				});
				Ok(handle)
			}
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
