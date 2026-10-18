use crate::error::{ApiError, ApiResult};
use crate::model::{MangaRow, manga_from_row, unix_now};
use crate::source::reconcile_chapters;
use crate::state::AppState;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, Semaphore, broadcast};
use tokio::task::JoinSet;

/// 全局同时更新的源数上限：源内串行、源间并行
const UPDATE_SOURCE_CONCURRENCY: usize = 3;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateEvent {
	#[serde(rename = "type")]
	pub kind: &'static str,
	pub status: &'static str,
	#[serde(rename = "mangaId")]
	pub manga_id: i64,
	#[serde(rename = "mangaTitle")]
	pub manga_title: String,
	#[serde(rename = "sourceId")]
	pub source_id: i64,
	#[serde(rename = "addedChapters")]
	pub added_chapters: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSummary {
	pub running: bool,
	pub total: i64,
	pub complete: i64,
	pub failed: i64,
	pub skipped: i64,
	pub pending: i64,
	pub last_run_at: Option<i64>,
}

/// 库更新检测：定时或手动触发全量 fetch，分类 include_in_update 三态过滤。
pub struct UpdateScheduler {
	// Weak 防循环引用：AppState 经 OnceCell 持有本调度器
	state: Weak<AppState>,
	pool: SqlitePool,
	events: broadcast::Sender<UpdateEvent>,
	summary: RwLock<UpdateSummary>,
	running: AtomicBool,
	fetch_lock: Arc<Mutex<()>>,
	auto_download: bool,
	auto_download_limit: i64,
	interval_hours: u64,
}

impl UpdateScheduler {
	pub fn new(state: &Arc<AppState>) -> Arc<Self> {
		Arc::new(Self {
			state: Arc::downgrade(state),
			pool: state.pool.clone(),
			events: broadcast::channel(256).0,
			summary: RwLock::new(UpdateSummary::default()),
			running: AtomicBool::new(false),
			fetch_lock: Arc::new(Mutex::new(())),
			auto_download: state.config.auto_download_new_chapters,
			auto_download_limit: state.config.auto_download_limit as i64,
			interval_hours: state.config.update_interval_hours,
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<UpdateEvent> {
		self.events.subscribe()
	}

	pub async fn summary(&self) -> UpdateSummary {
		self.summary.read().await.clone()
	}

	pub fn is_running(&self) -> bool {
		self.running.load(Ordering::SeqCst)
	}

	/// 抢 fetch 互斥锁；抢不到说明已有 fetch 在跑（API 据此回 409）
	pub fn try_acquire(&self) -> Option<OwnedMutexGuard<()>> {
		self.fetch_lock.clone().try_lock_owned().ok()
	}

	pub async fn run(self: Arc<Self>) {
		if self.interval_hours == 0 {
			return;
		}
		let mut interval =
			tokio::time::interval(std::time::Duration::from_secs(self.interval_hours * 3600));
		// interval 首 tick 立即完成，首跑等满一个周期
		interval.tick().await;
		loop {
			interval.tick().await;
			if let Some(guard) = self.try_acquire() {
				self.fetch(None, guard).await;
			}
		}
	}

	pub async fn fetch(self: &Arc<Self>, category_id: Option<i64>, _guard: OwnedMutexGuard<()>) {
		let rows = match select_manga(&self.pool, category_id).await {
			Ok(rows) => rows,
			Err(e) => {
				tracing::warn!("update select failed: {e:?}");
				return;
			}
		};
		let total = rows.len() as i64;
		{
			let mut summary = self.summary.write().await;
			*summary = UpdateSummary {
				running: true,
				total,
				complete: 0,
				failed: 0,
				skipped: 0,
				pending: total,
				last_run_at: Some(unix_now()),
			};
		}
		self.running.store(true, Ordering::SeqCst);
		for row in &rows {
			self.emit("pending", row, 0);
		}
		let mut by_source: HashMap<i64, Vec<MangaRow>> = HashMap::new();
		for row in rows {
			by_source.entry(row.source_id).or_default().push(row);
		}
		let semaphore = Arc::new(Semaphore::new(UPDATE_SOURCE_CONCURRENCY));
		let mut tasks = JoinSet::new();
		for manga_rows in by_source.into_values() {
			let scheduler = self.clone();
			let semaphore = semaphore.clone();
			tasks.spawn(async move {
				let _permit = semaphore.acquire_owned().await.expect("semaphore open");
				for manga in manga_rows {
					scheduler.update_one(manga).await;
				}
			});
		}
		while tasks.join_next().await.is_some() {}
		{
			let mut summary = self.summary.write().await;
			summary.running = false;
			summary.pending = 0;
			summary.last_run_at = Some(unix_now());
		}
		self.running.store(false, Ordering::SeqCst);
	}

	async fn update_one(self: &Arc<Self>, manga: MangaRow) {
		self.emit("running", &manga, 0);
		let enabled: Option<(bool,)> =
			match sqlx::query_as("SELECT enabled FROM source WHERE id = ?")
				.bind(manga.source_id)
				.fetch_optional(&self.pool)
				.await
			{
				Ok(row) => row,
				Err(e) => {
					tracing::warn!(manga_id = manga.id, "update source query failed: {e}");
					self.account("failed").await;
					self.emit("failed", &manga, 0);
					return;
				}
			};
		if !enabled.map(|(flag,)| flag).unwrap_or(false) {
			self.account("skipped").await;
			self.emit("skipped", &manga, 0);
			return;
		}
		let outcome: ApiResult<Vec<i64>> = async {
			let state = self
				.state
				.upgrade()
				.ok_or_else(|| ApiError::bad_request("server is shutting down"))?;
			let handle = state.get_handle(manga.source_id).await?;
			let updated = handle
				.get_manga_update(manga_from_row(&manga), false, true)
				.await?;
			let new_ids =
				reconcile_chapters(&self.pool, manga.id, &updated.chapters.clone().unwrap_or_default())
					.await?;
			sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
				.bind(unix_now())
				.bind(manga.id)
				.execute(&self.pool)
				.await?;
			Ok(new_ids)
		}
		.await;
		match outcome {
			Ok(new_ids) => {
				let added = new_ids.len() as i64;
				self.account("complete").await;
				self.emit("complete", &manga, added);
				if added > 0 && self.auto_download {
					self.enqueue_new_chapters(manga.id, new_ids).await;
				}
			}
			Err(e) => {
				tracing::warn!(manga_id = manga.id, "update failed: {e:?}");
				self.account("failed").await;
				self.emit("failed", &manga, 0);
			}
		}
	}

	async fn account(&self, field: &str) {
		let mut summary = self.summary.write().await;
		match field {
			"complete" => summary.complete += 1,
			"failed" => summary.failed += 1,
			"skipped" => summary.skipped += 1,
			_ => {}
		}
		summary.pending = (summary.pending - 1).max(0);
	}

	/// auto_download_new_chapters 联动：每漫画最多取 limit 个最新章入下载队
	async fn enqueue_new_chapters(self: &Arc<Self>, manga_id: i64, new_ids: Vec<i64>) {
		let Some(state) = self.state.upgrade() else { return };
		match download_allowed(&self.pool, manga_id).await {
			Ok(false) => return,
			Err(e) => {
				tracing::warn!(manga_id, "download category check failed: {e:?}");
				return;
			}
			_ => {}
		}
		// source_order 小的为源返回列表里的新章
		let placeholders = new_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let sql = format!(
			"SELECT id FROM chapter WHERE id IN ({placeholders}) ORDER BY source_order ASC LIMIT ?"
		);
		let mut query = sqlx::query_scalar::<_, i64>(&sql);
		for id in &new_ids {
			query = query.bind(id);
		}
		match query.bind(self.auto_download_limit).fetch_all(&self.pool).await {
			Ok(ids) if !ids.is_empty() => {
				state.downloads().enqueue(&ids).await;
			}
			Ok(_) => {}
			Err(e) => tracing::warn!(manga_id, "auto download select failed: {e}"),
		}
	}

	fn emit(&self, status: &'static str, manga: &MangaRow, added_chapters: i64) {
		let _ = self.events.send(UpdateEvent {
			kind: "job",
			status,
			manga_id: manga.id,
			manga_title: manga.title.clone(),
			source_id: manga.source_id,
			added_chapters,
		});
	}
}

/// 更新范围：在库且未标记 never_update；分类 include_in_update 三态
/// （有 INCLUDE 只跑 INCLUDE 分类下的，否则跑不在任何 EXCLUDE 分类的）
async fn select_manga(pool: &SqlitePool, category_id: Option<i64>) -> ApiResult<Vec<MangaRow>> {
	const BASE: &str = "SELECT m.* FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = 1
	 WHERE mu.in_library = 1 AND m.update_strategy != 1";
	let sql = match category_id {
		Some(id) => {
			let tri: Option<(i64,)> =
				sqlx::query_as("SELECT include_in_update FROM category WHERE id = ? AND user_id = 1")
					.bind(id)
					.fetch_optional(pool)
					.await?;
			let Some((tri,)) = tri else {
				return Err(ApiError::not_found("category not found"));
			};
			if tri == 2 {
				return Ok(Vec::new());
			}
			format!(
				"{BASE} AND EXISTS (SELECT 1 FROM category_manga cm
				 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND cm.category_id = {id})"
			)
		}
		None => {
			let has_include: Option<(i64,)> =
				sqlx::query_as("SELECT 1 FROM category WHERE user_id = 1 AND include_in_update = 1 LIMIT 1")
					.fetch_optional(pool)
					.await?;
			if has_include.is_some() {
				format!(
					"{BASE} AND EXISTS (SELECT 1 FROM category_manga cm
					 JOIN category c ON c.id = cm.category_id
					 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND c.include_in_update = 1)"
				)
			} else {
				format!(
					"{BASE} AND NOT EXISTS (SELECT 1 FROM category_manga cm
					 JOIN category c ON c.id = cm.category_id
					 WHERE cm.manga_id = m.id AND cm.user_id = 1 AND c.include_in_update = 2)"
				)
			}
		}
	};
	let rows = sqlx::query_as::<_, MangaRow>(&sql).fetch_all(pool).await?;
	Ok(rows)
}

/// include_in_download 三态过滤，规则与更新范围一致
async fn download_allowed(pool: &SqlitePool, manga_id: i64) -> ApiResult<bool> {
	let has_include: Option<(i64,)> =
		sqlx::query_as("SELECT 1 FROM category WHERE user_id = 1 AND include_in_download = 1 LIMIT 1")
			.fetch_optional(pool)
			.await?;
	if has_include.is_some() {
		let hit: Option<(i64,)> = sqlx::query_as(
			"SELECT 1 FROM category_manga cm JOIN category c ON c.id = cm.category_id
			 WHERE cm.manga_id = ? AND cm.user_id = 1 AND c.include_in_download = 1 LIMIT 1",
		)
		.bind(manga_id)
		.fetch_optional(pool)
		.await?;
		Ok(hit.is_some())
	} else {
		let blocked: Option<(i64,)> = sqlx::query_as(
			"SELECT 1 FROM category_manga cm JOIN category c ON c.id = cm.category_id
			 WHERE cm.manga_id = ? AND cm.user_id = 1 AND c.include_in_download = 2 LIMIT 1",
		)
		.bind(manga_id)
		.fetch_optional(pool)
		.await?;
		Ok(blocked.is_none())
	}
}
