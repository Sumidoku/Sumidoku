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

/// Global cap on sources updating at once: serial within a source, parallel across sources
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
	/// The user the event belongs to (whose library this manga is in); WS filters per connection user
	#[serde(skip)]
	pub user_id: Option<i64>,
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
	/// Ownership of this fetch: None = scheduled union (global progress visible to all), Some = manually triggered by that user
	#[serde(skip)]
	pub user_id: Option<i64>,
}

impl UpdateSummary {
	/// Trimmed for the requesting user: a fetch someone else triggered is irrelevant to this user and presented as
	/// idle; a union (no owner) is visible to all
	pub fn visible_to(self, user_id: i64) -> Self {
		match self.user_id {
			Some(owner) if owner != user_id => Self::default(),
			_ => self,
		}
	}
}

/// Library update detection: scheduled or manually triggered full fetches, filtered by the category include_in_update tri-state.
pub struct UpdateScheduler {
	// Weak prevents a reference cycle: AppState holds this scheduler through a OnceCell
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

	/// Grabs the fetch mutex; failing means a fetch is already running (the API answers 409 from this)
	pub fn try_acquire(&self) -> Option<OwnedMutexGuard<()>> {
		self.fetch_lock.clone().try_lock_owned().ok()
	}

	pub async fn run(self: Arc<Self>) {
		if self.interval_hours == 0 {
			return;
		}
		let mut interval =
			tokio::time::interval(std::time::Duration::from_secs(self.interval_hours * 3600));
		// interval's first tick completes immediately, so the first run waits a full period
		interval.tick().await;
		loop {
			interval.tick().await;
			if let Some(guard) = self.try_acquire() {
				self.fetch_union(guard).await;
			}
		}
	}

	pub async fn fetch(self: &Arc<Self>, user_id: i64, category_id: Option<i64>, guard: OwnedMutexGuard<()>) {
		let rows = match select_manga(&self.pool, user_id, category_id).await {
			Ok(rows) => rows,
			Err(e) => {
				tracing::warn!("update select failed: {e:?}");
				return;
			}
		};
		let owners = rows.iter().map(|row| (row.id, vec![user_id])).collect();
		self.run_fetch(rows, owners, Some(user_id), guard).await;
	}

	/// Scheduled full sweep: the union of every user's library, with each manga fetched from its source only once; auto-download follows each user's own filtering
	pub async fn fetch_union(self: &Arc<Self>, guard: OwnedMutexGuard<()>) {
		let user_ids: Vec<i64> = match sqlx::query_scalar(
			"SELECT DISTINCT user_id FROM manga_user WHERE in_library = 1 ORDER BY user_id",
		)
		.fetch_all(&self.pool)
		.await
		{
			Ok(ids) => ids,
			Err(e) => {
				tracing::warn!("update union user select failed: {e:?}");
				return;
			}
		};
		let mut rows: Vec<MangaRow> = Vec::new();
		let mut owners: HashMap<i64, Vec<i64>> = HashMap::new();
		for user_id in user_ids {
			// Each user reuses the same tri-state filtering, so the rules never drift from a manual trigger
			let user_rows = match select_manga(&self.pool, user_id, None).await {
				Ok(rows) => rows,
				Err(e) => {
					tracing::warn!("update union select failed: {e:?}");
					continue;
				}
			};
			for row in user_rows {
				match owners.entry(row.id) {
					std::collections::hash_map::Entry::Vacant(v) => {
						v.insert(vec![user_id]);
						rows.push(row);
					}
					std::collections::hash_map::Entry::Occupied(mut o) => o.get_mut().push(user_id),
				}
			}
		}
		self.run_fetch(rows, owners, None, guard).await;
	}

	async fn run_fetch(
		self: &Arc<Self>,
		rows: Vec<MangaRow>,
		owners: HashMap<i64, Vec<i64>>,
		scope: Option<i64>,
		_guard: OwnedMutexGuard<()>,
	) {
		let owners = Arc::new(owners);
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
				user_id: scope,
			};
		}
		self.running.store(true, Ordering::SeqCst);
		for row in &rows {
			self.emit_for(owners.get(&row.id).map(Vec::as_slice).unwrap_or(&[]), "pending", row, 0);
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
			let owners = owners.clone();
			tasks.spawn(async move {
				let _permit = semaphore.acquire_owned().await.expect("semaphore open");
				for manga in manga_rows {
					scheduler.update_one(manga, &owners).await;
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

	async fn update_one(self: &Arc<Self>, manga: MangaRow, owners: &HashMap<i64, Vec<i64>>) {
		let interested = owners.get(&manga.id).map(Vec::as_slice).unwrap_or(&[]);
		self.emit_for(interested, "running", &manga, 0);
		let source: Option<(bool, String)> =
			match sqlx::query_as("SELECT enabled, kind FROM source WHERE id = ?")
				.bind(manga.source_id)
				.fetch_optional(&self.pool)
				.await
			{
				Ok(row) => row,
				Err(e) => {
					tracing::warn!(manga_id = manga.id, "update source query failed: {e}");
					self.account("failed").await;
					self.emit_for(interested, "failed", &manga, 0);
					return;
				}
			};
		let Some((enabled, kind)) = source else {
			self.account("skipped").await;
			self.emit_for(interested, "skipped", &manga, 0);
			return;
		};
		if !enabled {
			self.account("skipped").await;
			self.emit_for(interested, "skipped", &manga, 0);
			return;
		}
		let is_local = kind == crate::model::SOURCE_KIND_LOCAL;
		let is_opds = kind == crate::model::SOURCE_KIND_OPDS;
		let outcome: ApiResult<Vec<i64>> = async {
			let state = self
				.state
				.upgrade()
				.ok_or_else(|| ApiError::bad_request("server is shutting down"))?;
			let (updated, protected) = if is_local {
				let update = crate::local::manga_update(&state.config.local_dir(), &state.local_zip, &manga.key).await?;
				(update.manga, update.protected)
			} else if is_opds {
				let updated = crate::opds_source::manga_update(&state, manga.source_id, manga_from_row(&manga)).await?;
				(updated, Vec::new())
			} else {
				let handle = state.get_handle(manga.source_id).await?;
				let updated = handle
					.get_manga_update(manga_from_row(&manga), false, true)
					.await?;
				(updated, Vec::new())
			};
			let mut conn = self.pool.acquire().await?;
			let new_ids =
				reconcile_chapters(&mut conn, manga.id, &updated.chapters.clone().unwrap_or_default(), &protected)
					.await?;
			sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
				.bind(unix_now())
				.bind(manga.id)
				.execute(&mut *conn)
				.await?;
			Ok(new_ids)
		}
		.await;
		match outcome {
			Ok(new_ids) => {
				let added = new_ids.len() as i64;
				self.account("complete").await;
				self.emit_for(interested, "complete", &manga, added);
				// Local manga never enters download management (the files are already local), so new-chapter auto-download is skipped
				if added > 0 && self.auto_download && !is_local {
					for &user_id in interested {
						self.enqueue_new_chapters(user_id, manga.id, new_ids.clone()).await;
					}
				}
			}
			Err(e) => {
				tracing::warn!(manga_id = manga.id, "update failed: {e:?}");
				self.account("failed").await;
				self.emit_for(interested, "failed", &manga, 0);
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

	/// auto_download_new_chapters linkage: at most limit newest chapters per manga go into the download queue
	async fn enqueue_new_chapters(self: &Arc<Self>, user_id: i64, manga_id: i64, new_ids: Vec<i64>) {
		let Some(state) = self.state.upgrade() else { return };
		match download_allowed(&self.pool, user_id, manga_id).await {
			Ok(false) => return,
			Err(e) => {
				tracing::warn!(manga_id, "download category check failed: {e:?}");
				return;
			}
			_ => {}
		}
		// A smaller source_order marks a new chapter in the source's returned list
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
				state.downloads().enqueue(&ids, user_id).await;
			}
			Ok(_) => {}
			Err(e) => tracing::warn!(manga_id, "auto download select failed: {e}"),
		}
	}

	fn emit_for(&self, user_ids: &[i64], status: &'static str, manga: &MangaRow, added_chapters: i64) {
		for &user_id in user_ids {
			let _ = self.events.send(UpdateEvent {
				kind: "job",
				status,
				manga_id: manga.id,
				manga_title: manga.title.clone(),
				source_id: manga.source_id,
				added_chapters,
				user_id: Some(user_id),
			});
		}
	}
}

/// Update scope: in this user's library and not marked never_update; the category include_in_update tri-state
/// (with any INCLUDE, only entries under INCLUDE categories run; otherwise everything not in an EXCLUDE category runs)
async fn select_manga(pool: &SqlitePool, user_id: i64, category_id: Option<i64>) -> ApiResult<Vec<MangaRow>> {
	const BASE: &str = "SELECT m.* FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ?
	 WHERE mu.in_library = 1 AND m.update_strategy != 1";
	let sql = match category_id {
		Some(id) => {
			let tri: Option<(i64,)> =
				sqlx::query_as("SELECT include_in_update FROM category WHERE id = ? AND user_id = ?")
					.bind(id)
					.bind(user_id)
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
				 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id = {id})"
			)
		}
		None => {
			let has_include: Option<(i64,)> =
				sqlx::query_as("SELECT 1 FROM category WHERE user_id = ? AND include_in_update = 1 LIMIT 1")
					.bind(user_id)
					.fetch_optional(pool)
					.await?;
			if has_include.is_some() {
				format!(
					"{BASE} AND EXISTS (SELECT 1 FROM category_manga cm
					 JOIN category c ON c.id = cm.category_id
					 WHERE cm.manga_id = m.id AND cm.user_id = ? AND c.include_in_update = 1)"
				)
			} else {
				format!(
					"{BASE} AND NOT EXISTS (SELECT 1 FROM category_manga cm
					 JOIN category c ON c.id = cm.category_id
					 WHERE cm.manga_id = m.id AND cm.user_id = ? AND c.include_in_update = 2)"
				)
			}
		}
	};
	let rows = sqlx::query_as::<_, MangaRow>(&sql)
		.bind(user_id)
		.bind(user_id)
		.fetch_all(pool)
		.await?;
	Ok(rows)
}

/// include_in_download tri-state filtering, rules identical to the update scope
async fn download_allowed(pool: &SqlitePool, user_id: i64, manga_id: i64) -> ApiResult<bool> {
	let has_include: Option<(i64,)> =
		sqlx::query_as("SELECT 1 FROM category WHERE user_id = ? AND include_in_download = 1 LIMIT 1")
			.bind(user_id)
			.fetch_optional(pool)
			.await?;
	if has_include.is_some() {
		let hit: Option<(i64,)> = sqlx::query_as(
			"SELECT 1 FROM category_manga cm JOIN category c ON c.id = cm.category_id
			 WHERE cm.manga_id = ? AND cm.user_id = ? AND c.include_in_download = 1 LIMIT 1",
		)
		.bind(manga_id)
		.bind(user_id)
		.fetch_optional(pool)
		.await?;
		Ok(hit.is_some())
	} else {
		let blocked: Option<(i64,)> = sqlx::query_as(
			"SELECT 1 FROM category_manga cm JOIN category c ON c.id = cm.category_id
			 WHERE cm.manga_id = ? AND cm.user_id = ? AND c.include_in_download = 2 LIMIT 1",
		)
		.bind(manga_id)
		.bind(user_id)
		.fetch_optional(pool)
		.await?;
		Ok(blocked.is_none())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::Config;
	use tokio::sync::mpsc;

	fn fixture_wasm() -> String {
		std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("../wasm/tests/fixtures/example/main.wasm")
			.display()
			.to_string()
	}

	async fn union_state(config: Config) -> (Arc<AppState>, i64, tempfile::TempDir) {
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
		let state = AppState::new(pool, config, defaults_tx, cookie_tx);
		assert!(state.downloads.set(crate::download::DownloadManager::new(&state)).is_ok());
		(state, source_id, tmp)
	}

	async fn add_manga(state: &AppState, source_id: i64, key: &str) -> i64 {
		sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, ?, ?) RETURNING id")
			.bind(source_id)
			.bind(key)
			.bind(format!("Manga {key}"))
			.fetch_one(&state.pool)
			.await
			.expect("insert manga")
	}

	async fn add_to_library(state: &AppState, manga_id: i64, user_id: i64) {
		sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, ?, 1)")
			.bind(manga_id)
			.bind(user_id)
			.execute(&state.pool)
			.await
			.expect("add to library");
	}

	async fn insert_bob(state: &AppState) {
		sqlx::query("INSERT INTO user (username, password_hash, role) VALUES ('bob', '', 'reader')")
			.execute(&state.pool)
			.await
			.expect("insert bob");
	}

	#[tokio::test]
	async fn union_fetch_pulls_shared_manga_once() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		insert_bob(&state).await;
		let m1 = add_manga(&state, source_id, "1").await;
		let m2 = add_manga(&state, source_id, "2").await;
		let m3 = add_manga(&state, source_id, "3").await;
		add_to_library(&state, m1, 1).await;
		add_to_library(&state, m2, 2).await;
		add_to_library(&state, m3, 1).await;
		add_to_library(&state, m3, 2).await;

		let scheduler = UpdateScheduler::new(&state);
		let guard = scheduler.try_acquire().expect("fetch guard");
		scheduler.fetch_union(guard).await;

		// The union covers both users' libraries, and m3, shelved by both, is still fetched from the source only once
		let summary = scheduler.summary().await;
		assert_eq!(summary.total, 3);
		assert_eq!(summary.complete, 3);
		assert_eq!(summary.user_id, None);
		for manga_id in [m1, m2, m3] {
			let stamped: Option<i64> =
				sqlx::query_scalar("SELECT chapters_last_fetched_at FROM manga WHERE id = ?")
					.bind(manga_id)
					.fetch_one(&state.pool)
					.await
					.expect("manga row");
			assert!(stamped.is_some(), "manga {manga_id} not fetched");
			let chapters: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("chapter count");
			assert_eq!(chapters, 8);
		}
	}

	#[tokio::test]
	async fn union_fetch_auto_download_filters_per_user() {
		let config = Config {
			auto_download_new_chapters: true,
			..Config::default()
		};
		let (state, source_id, _tmp) = union_state(config).await;
		insert_bob(&state).await;
		let manga_id = add_manga(&state, source_id, "1").await;
		add_to_library(&state, manga_id, 1).await;
		add_to_library(&state, manga_id, 2).await;
		// bob's category marks the same manga as not auto-downloadable
		let category_id: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name, include_in_download) VALUES (2, 'blocked', 2) RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("insert category");
		sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 2)")
			.bind(category_id)
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("link category");

		let scheduler = UpdateScheduler::new(&state);
		let guard = scheduler.try_acquire().expect("fetch guard");
		scheduler.fetch_union(guard).await;
		assert_eq!(scheduler.summary().await.complete, 1);

		let owners: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM download GROUP BY user_id")
			.fetch_all(&state.pool)
			.await
			.expect("download owners");
		assert_eq!(owners, vec![1], "auto download must only enqueue for the allowing user");
	}
}
