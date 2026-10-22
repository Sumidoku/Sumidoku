use crate::error::{ApiError, ApiResult};
use crate::model::{MangaRow, manga_from_row, unix_now, update_strategy_from_int};
use crate::source::{ReconcileOutcome, reconcile_chapters_detailed, upsert_manga};
use crate::state::AppState;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, Semaphore, broadcast};
use tokio::task::JoinSet;

/// Global cap on sources updating at once: serial within a source, parallel across sources
const UPDATE_SOURCE_CONCURRENCY: usize = 3;
const SCHEDULE_CHECK_INTERVAL: Duration = Duration::from_secs(60);

pub const LIBRARY_SKIP_TITLES: &str = "library.skip_titles";
pub const LIBRARY_REFRESH_METADATA: &str = "library.refresh_metadata";
pub const LIBRARY_UPDATE_INTERVAL_HOURS: &str = "library.update_interval_hours";
pub const LIBRARY_EXCLUDED_UPDATE_CATEGORIES: &str = "library.excluded_update_categories";
pub const SKIP_TITLE_RULES: [&str; 3] = ["hasUnread", "completed", "notStarted"];

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryUpdateSettings {
	pub skip_titles: Vec<String>,
	pub refresh_metadata: bool,
	pub update_interval_hours: u64,
	pub excluded_update_categories: Vec<i64>,
}

impl LibraryUpdateSettings {
	pub async fn load(pool: &SqlitePool, user_id: i64, config_interval_hours: u64) -> ApiResult<Self> {
		let excluded_key = format!("user.{user_id}.{LIBRARY_EXCLUDED_UPDATE_CATEGORIES}");
		let rows: Vec<(String, String)> = sqlx::query_as(
			"SELECT key, value FROM setting WHERE key IN (?, ?, ?, ?)",
		)
		.bind(LIBRARY_SKIP_TITLES)
		.bind(LIBRARY_REFRESH_METADATA)
		.bind(LIBRARY_UPDATE_INTERVAL_HOURS)
		.bind(&excluded_key)
		.fetch_all(pool)
		.await?;
		let values: HashMap<String, Value> = rows
			.into_iter()
			.filter_map(|(key, raw)| serde_json::from_str(&raw).ok().map(|value| (key, value)))
			.collect();
		Ok(Self {
			skip_titles: parse_skip_titles(values.get(LIBRARY_SKIP_TITLES)),
			refresh_metadata: values
				.get(LIBRARY_REFRESH_METADATA)
				.and_then(Value::as_bool)
				.unwrap_or(false),
			update_interval_hours: values
				.get(LIBRARY_UPDATE_INTERVAL_HOURS)
				.and_then(Value::as_u64)
				.unwrap_or(config_interval_hours),
			excluded_update_categories: parse_excluded_categories(values.get(&excluded_key)),
		})
	}
}

fn default_skip_titles() -> Vec<String> {
	SKIP_TITLE_RULES.iter().map(|rule| (*rule).to_string()).collect()
}

fn valid_skip_titles(value: &Value) -> bool {
	value.as_array().is_some_and(|rules| {
		rules.iter().all(|rule| {
			rule.as_str().is_some_and(|rule| SKIP_TITLE_RULES.contains(&rule))
		})
	})
}

fn parse_skip_titles(value: Option<&Value>) -> Vec<String> {
	value
		.filter(|value| valid_skip_titles(value))
		.and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
		.unwrap_or_else(default_skip_titles)
}

fn valid_excluded_categories(value: &Value) -> bool {
	value.as_array().is_some_and(|ids| ids.iter().all(|id| id.as_i64().is_some()))
}

fn parse_excluded_categories(value: Option<&Value>) -> Vec<i64> {
	value
		.filter(|value| valid_excluded_categories(value))
		.and_then(|value| serde_json::from_value::<Vec<i64>>(value.clone()).ok())
		.unwrap_or_default()
}

pub(crate) fn validate_library_setting(key: &str, value: &Value) -> Option<ApiResult<()>> {
	let result = match key {
		LIBRARY_SKIP_TITLES if !valid_skip_titles(value) => Err(ApiError::bad_request(
			"library.skip_titles must be an array containing only hasUnread, completed or notStarted",
		)),
		LIBRARY_REFRESH_METADATA if !value.is_boolean() => {
			Err(ApiError::bad_request("library.refresh_metadata must be a boolean"))
		}
		LIBRARY_UPDATE_INTERVAL_HOURS if value.as_u64().is_none() => Err(ApiError::bad_request(
			"library.update_interval_hours must be a non-negative integer",
		)),
		LIBRARY_EXCLUDED_UPDATE_CATEGORIES if !valid_excluded_categories(value) => Err(ApiError::bad_request(
			"library.excluded_update_categories must be an array of category ids",
		)),
		LIBRARY_SKIP_TITLES | LIBRARY_REFRESH_METADATA | LIBRARY_UPDATE_INTERVAL_HOURS | LIBRARY_EXCLUDED_UPDATE_CATEGORIES => Ok(()),
		_ => return None,
	};
	Some(result)
}

fn interval_seconds(interval_hours: u64) -> i64 {
	i64::try_from(interval_hours)
		.unwrap_or(i64::MAX)
		.checked_mul(3600)
		.unwrap_or(i64::MAX)
}

fn resolved_next_update(
	source_value: Option<i64>,
	previous_value: Option<i64>,
	fetched_at: i64,
	interval_hours: u64,
) -> (Option<i64>, bool) {
	match source_value {
		Some(at) if Some(at) != previous_value => (Some(at), false),
		_ if interval_hours > 0 => (Some(fetched_at.saturating_add(interval_seconds(interval_hours))), true),
		_ => (None, false),
	}
}

fn is_due(started_at: i64, last_run_at: Option<i64>, now: i64, interval_hours: u64) -> bool {
	interval_hours > 0
		&& now.saturating_sub(last_run_at.unwrap_or(started_at)) >= interval_seconds(interval_hours)
}

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

/// Library update detection: scheduled or manual fetches with runtime skip and download settings.
pub struct UpdateScheduler {
	// Weak prevents a reference cycle: AppState holds this scheduler through a OnceCell
	state: Weak<AppState>,
	pool: SqlitePool,
	events: broadcast::Sender<UpdateEvent>,
	summary: RwLock<UpdateSummary>,
	last_scheduled_run_at: RwLock<Option<i64>>,
	running: AtomicBool,
	fetch_lock: Arc<Mutex<()>>,
	config_interval_hours: u64,
	started_at: i64,
}

impl UpdateScheduler {
	pub fn new(state: &Arc<AppState>) -> Arc<Self> {
		Arc::new(Self {
			state: Arc::downgrade(state),
			pool: state.pool.clone(),
			events: broadcast::channel(256).0,
			summary: RwLock::new(UpdateSummary::default()),
			last_scheduled_run_at: RwLock::new(None),
			running: AtomicBool::new(false),
			fetch_lock: Arc::new(Mutex::new(())),
			config_interval_hours: state.config.update_interval_hours,
			started_at: unix_now(),
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
		let mut ticker = tokio::time::interval(SCHEDULE_CHECK_INTERVAL);
		loop {
			ticker.tick().await;
			if let Err(e) = self.run_scheduled_if_due().await {
				tracing::warn!("scheduled library update failed: {e:?}");
			}
		}
	}

	async fn scheduled_due(&self, now: i64) -> ApiResult<bool> {
		// update_interval_hours is global; any user id works for loading it.
		let settings = LibraryUpdateSettings::load(&self.pool, 1, self.config_interval_hours).await?;
		let last_run_at = *self.last_scheduled_run_at.read().await;
		Ok(is_due(self.started_at, last_run_at, now, settings.update_interval_hours))
	}

	async fn run_scheduled_if_due(self: &Arc<Self>) -> ApiResult<bool> {
		if !self.scheduled_due(unix_now()).await? {
			return Ok(false);
		}
		let Some(guard) = self.try_acquire() else {
			return Ok(false);
		};
		self.fetch_union(guard).await;
		Ok(true)
	}

	pub async fn fetch(self: &Arc<Self>, user_id: i64, category_id: Option<i64>, guard: OwnedMutexGuard<()>) {
		let rows = match select_manga(&self.pool, user_id, category_id, self.config_interval_hours).await {
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
		*self.last_scheduled_run_at.write().await = Some(unix_now());
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
			// Each user reuses the same scope filtering, so the rules never drift from a manual trigger
			let user_rows = match select_manga(&self.pool, user_id, None, self.config_interval_hours).await {
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
		*self.last_scheduled_run_at.write().await = Some(unix_now());
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
		let outcome: ApiResult<ReconcileOutcome> = async {
			let state = self
				.state
				.upgrade()
				.ok_or_else(|| ApiError::bad_request("server is shutting down"))?;
			// refresh_metadata and update_interval_hours are global; user id only matters for excluded categories.
			let settings = LibraryUpdateSettings::load(&self.pool, 1, state.config.update_interval_hours).await?;
			let (updated, protected) = if is_local {
				let update = crate::local::manga_update(&state.local_dirs().await?, &state.local_zip, &manga.key).await?;
				(update.manga, update.protected)
			} else if is_opds {
				let updated = crate::opds_source::manga_update(&state, manga.source_id, manga_from_row(&manga)).await?;
				(updated, Vec::new())
			} else {
				let handle = state.get_handle(manga.source_id).await?;
				let updated = handle
					.get_manga_update(manga_from_row(&manga), settings.refresh_metadata, true)
					.await?;
				(updated, Vec::new())
			};
			let mut tx = crate::db::begin_write(&self.pool).await?;
			let reconciled = persist_success(&mut tx, &manga, &updated, &protected, &settings, unix_now()).await?;
			tx.commit().await?;
			Ok(reconciled)
		}
		.await;
		match outcome {
			Ok(reconciled) => {
				let added = reconciled.new_ids.len() as i64;
				self.account("complete").await;
				self.emit_for(interested, "complete", &manga, added);
				// Local manga never enters download management (the files are already local), so new-chapter auto-download is skipped
				if added > 0 && !is_local {
					for &user_id in interested {
						self.enqueue_new_chapters(
							user_id,
							manga.id,
							&reconciled.new_ids,
							reconciled.previous_latest_chapter_number,
						)
						.await;
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

	async fn enqueue_new_chapters(
		self: &Arc<Self>,
		user_id: i64,
		manga_id: i64,
		new_ids: &[i64],
		previous_latest_chapter_number: Option<f64>,
	) {
		if new_ids.is_empty() {
			return;
		}
		let Some(state) = self.state.upgrade() else { return };
		let settings = match crate::download::DownloadSettings::load(&self.pool, user_id, &state.config).await {
			Ok(settings) => settings,
			Err(e) => {
				tracing::warn!(manga_id, "download settings load failed: {e:?}");
				return;
			}
		};
		if !settings.auto_download_new_chapters {
			return;
		}
		match download_allowed(&self.pool, user_id, manga_id, &settings.excluded_download_categories).await {
			Ok(false) => return,
			Err(e) => {
				tracing::warn!(manga_id, "download category check failed: {e:?}");
				return;
			}
			_ => {}
		}
		let placeholders = new_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		if settings.auto_skip_manga_with_unread {
			let sql = format!(
				"SELECT 1 FROM chapter c WHERE c.manga_id = ? AND c.id NOT IN ({placeholders})
				 AND NOT EXISTS (SELECT 1 FROM chapter_user cu
				 WHERE cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_read = 1) LIMIT 1",
			);
			let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(manga_id);
			for id in new_ids {
				query = query.bind(id);
			}
			match query.bind(user_id).fetch_optional(&self.pool).await {
				Ok(Some(_)) => return,
				Ok(None) => {}
				Err(e) => {
					tracing::warn!(manga_id, "auto download unread check failed: {e}");
					return;
				}
			}
		}
		let mut sql = format!("SELECT id FROM chapter WHERE id IN ({placeholders})");
		if settings.auto_ignore_reuploads && previous_latest_chapter_number.is_some() {
			sql.push_str(" AND (number IS NULL OR number >= ?)");
		}
		sql.push_str(" ORDER BY source_order ASC LIMIT ?");
		let mut query = sqlx::query_scalar::<_, i64>(&sql);
		for id in new_ids {
			query = query.bind(id);
		}
		if settings.auto_ignore_reuploads
			&& let Some(previous_latest) = previous_latest_chapter_number
		{
			query = query.bind(previous_latest);
		}
		match query.bind(settings.auto_download_limit).fetch_all(&self.pool).await {
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

async fn persist_success(
	conn: &mut sqlx::SqliteConnection,
	manga: &MangaRow,
	updated: &aidoku::Manga,
	protected: &[String],
	settings: &LibraryUpdateSettings,
	fetched_at: i64,
) -> ApiResult<ReconcileOutcome> {
	let mut updated = updated.clone();
	let (next_update_time, next_update_is_fallback) = resolved_next_update(
		updated.next_update_time,
		manga.next_update_time,
		fetched_at,
		settings.update_interval_hours,
	);
	updated.next_update_time = next_update_time;
	if settings.refresh_metadata {
		let update_strategy: i64 = sqlx::query_scalar("SELECT update_strategy FROM manga WHERE id = ?")
			.bind(manga.id)
			.fetch_one(&mut *conn)
			.await?;
		updated.update_strategy = update_strategy_from_int(update_strategy);
		upsert_manga(conn, manga.source_id, &updated, true, true, false).await?;
	}
	let reconciled = reconcile_chapters_detailed(
		conn,
		manga.id,
		updated.chapters.as_deref().unwrap_or(&[]),
		protected,
	)
	.await?;
	sqlx::query(
		"UPDATE manga SET next_update_time = ?, next_update_is_fallback = ?, chapters_last_fetched_at = ? WHERE id = ?",
	)
	.bind(next_update_time)
	.bind(next_update_is_fallback)
	.bind(fetched_at)
	.bind(manga.id)
	.execute(&mut *conn)
	.await?;
	Ok(reconciled)
}

/// Update scope combines the admin's library, excluded categories, skip settings and per-manga schedule.
/// The deprecated per-category include_in_update tri-state is no longer consulted.
async fn select_manga(
	pool: &SqlitePool,
	user_id: i64,
	category_id: Option<i64>,
	config_interval_hours: u64,
) -> ApiResult<Vec<MangaRow>> {
	const BASE: &str = "SELECT m.* FROM manga m
	 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ?
	 WHERE mu.in_library = 1 AND m.update_strategy != 1";
	let settings = LibraryUpdateSettings::load(pool, user_id, config_interval_hours).await?;
	let seconds = interval_seconds(settings.update_interval_hours);
	// When updating from a specific category, Aidoku ignores that category in the global exclusion list
	// (pulling that category explicitly should still update its titles).
	let excluded_categories: Vec<i64> = category_id
		.map(|id| settings.excluded_update_categories.iter().copied().filter(|c| *c != id).collect())
		.unwrap_or_else(|| settings.excluded_update_categories.clone());
	let mut sql = if let Some(id) = category_id {
		let exists: Option<(i64,)> =
			sqlx::query_as("SELECT 1 FROM category WHERE id = ? AND user_id = ?")
				.bind(id)
				.bind(user_id)
				.fetch_optional(pool)
				.await?;
		if exists.is_none() {
			return Err(ApiError::not_found("category not found"));
		}
		format!(
			"{BASE} AND EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id = {id})"
		)
	} else {
		BASE.to_string()
	};
	if !excluded_categories.is_empty() {
		let ids = excluded_categories
			.iter()
			.map(|id| id.to_string())
			.collect::<Vec<_>>()
			.join(",");
		sql.push_str(&format!(
			" AND NOT EXISTS (SELECT 1 FROM category_manga cm
			 WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id IN ({ids}))"
		));
	}
	for rule in settings.skip_titles {
		let clause = match rule.as_str() {
			"hasUnread" => {
				" AND NOT EXISTS (SELECT 1 FROM chapter c WHERE c.manga_id = m.id
				 AND NOT EXISTS (SELECT 1 FROM chapter_user cu
				 WHERE cu.chapter_id = c.id AND cu.user_id = mu.user_id AND cu.is_read = 1))"
			}
			"completed" => " AND m.status != 2",
			"notStarted" => {
				" AND EXISTS (SELECT 1 FROM chapter c
				 JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = mu.user_id AND cu.is_read = 1
				 WHERE c.manga_id = m.id)"
			}
			_ => continue,
		};
		sql.push_str(clause);
	}
	sql.push_str(
		" AND ((m.next_update_is_fallback = 0
		 AND (m.next_update_time IS NULL OR m.next_update_time <= unixepoch()))
		 OR (m.next_update_is_fallback = 1
		 AND (? = 0 OR m.chapters_last_fetched_at IS NULL
		 OR m.chapters_last_fetched_at <= unixepoch() - ?)))",
	);
	let mut query = sqlx::query_as::<_, MangaRow>(&sql).bind(user_id);
	if category_id.is_some() {
		query = query.bind(user_id);
	}
	if !excluded_categories.is_empty() {
		query = query.bind(user_id);
	}
	let rows = query.bind(seconds).bind(seconds).fetch_all(pool).await?;
	Ok(rows)
}

async fn download_allowed(
	pool: &SqlitePool,
	user_id: i64,
	manga_id: i64,
	excluded_categories: &[i64],
) -> ApiResult<bool> {
	if excluded_categories.is_empty() {
		return Ok(true);
	}
	let ids = excluded_categories
		.iter()
		.map(|id| id.to_string())
		.collect::<Vec<_>>()
		.join(",");
	let blocked: Option<(i64,)> = sqlx::query_as(&format!(
		"SELECT 1 FROM category_manga cm
		 WHERE cm.manga_id = ? AND cm.user_id = ? AND cm.category_id IN ({ids}) LIMIT 1",
	))
	.bind(manga_id)
	.bind(user_id)
	.fetch_optional(pool)
	.await?;
	Ok(blocked.is_none())
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

	fn partial_wasm() -> String {
		std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("../wasm/tests/fixtures/partial/main.wasm")
			.display()
			.to_string()
	}

	async fn union_state(config: Config) -> (Arc<AppState>, i64, tempfile::TempDir) {
		union_state_with_wasm(config, fixture_wasm()).await
	}

	async fn union_state_with_wasm(config: Config, wasm_path: String) -> (Arc<AppState>, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let mut config = config;
		config.config_dir = tmp.path().to_path_buf();
		config.database_url = format!("sqlite://{}/test.db?mode=rwc", tmp.path().display());
		config.covers_dir = tmp.path().join("covers");
		config.downloads_dir = tmp.path().join("downloads");
		let pool = crate::db::create_pool(&config.database_url).await.expect("pool");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, wasm_path) VALUES ('en.example-source', 'Example', ?) RETURNING id",
		)
		.bind(wasm_path)
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

	async fn set_setting(state: &AppState, key: &str, value: serde_json::Value) {
		sqlx::query(
			"INSERT INTO setting (key, value) VALUES (?, ?)
			 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		)
		.bind(key)
		.bind(serde_json::to_string(&value).expect("encode setting"))
		.execute(&state.pool)
		.await
		.expect("set setting");
	}

	async fn add_chapter(state: &AppState, manga_id: i64, key: &str) -> i64 {
		sqlx::query_scalar("INSERT INTO chapter (manga_id, key) VALUES (?, ?) RETURNING id")
			.bind(manga_id)
			.bind(key)
			.fetch_one(&state.pool)
			.await
			.expect("insert chapter")
	}

	async fn set_read(state: &AppState, chapter_id: i64, user_id: i64) {
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_read) VALUES (?, ?, 1)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_read = 1",
		)
		.bind(chapter_id)
		.bind(user_id)
		.execute(&state.pool)
		.await
		.expect("mark chapter read");
	}

	async fn selected_ids(state: &AppState, user_id: i64, category_id: Option<i64>) -> Vec<i64> {
		let mut ids: Vec<i64> = select_manga(&state.pool, user_id, category_id, state.config.update_interval_hours)
			.await
			.expect("select manga")
			.into_iter()
			.map(|row| row.id)
			.collect();
		ids.sort_unstable();
		ids
	}

	async fn update_one_direct(state: &Arc<AppState>, scheduler: &Arc<UpdateScheduler>, manga_id: i64) {
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let owners = HashMap::from([(manga_id, vec![1])]);
		scheduler.update_one(row, &owners).await;
	}

	#[tokio::test]
	async fn union_fetch_pulls_shared_manga_once() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		let m1 = add_manga(&state, source_id, "1").await;
		let m2 = add_manga(&state, source_id, "2").await;
		let m3 = add_manga(&state, source_id, "3").await;
		for manga_id in [m1, m2, m3] {
			add_to_library(&state, manga_id, 1).await;
		}

		let scheduler = UpdateScheduler::new(&state);
		let guard = scheduler.try_acquire().expect("fetch guard");
		scheduler.fetch_union(guard).await;

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
	async fn union_fetch_auto_download_respects_excluded_category() {
		let config = Config {
			auto_download_new_chapters: true,
			..Config::default()
		};
		let (state, source_id, _tmp) = union_state(config).await;
		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		let manga_id = add_manga(&state, source_id, "1").await;
		add_to_library(&state, manga_id, 1).await;
		let category_id: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name) VALUES (1, 'blocked') RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("insert category");
		sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
			.bind(category_id)
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("link category");
		set_setting(
			&state,
			"user.1.library.excluded_download_categories",
			serde_json::json!([category_id]),
		)
		.await;

		let scheduler = UpdateScheduler::new(&state);
		let guard = scheduler.try_acquire().expect("fetch guard");
		scheduler.fetch_union(guard).await;
		assert_eq!(scheduler.summary().await.complete, 1);

		let owners: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM download GROUP BY user_id")
			.fetch_all(&state.pool)
			.await
			.expect("download owners");
		assert_eq!(owners, Vec::<i64>::new());
	}

	#[tokio::test]
	async fn auto_download_excludes_only_preexisting_unread_chapters() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, crate::download::AUTO_DOWNLOAD_NEW_CHAPTERS, serde_json::json!(true)).await;
		set_setting(&state, crate::download::AUTO_SKIP_MANGA_WITH_UNREAD, serde_json::json!(true)).await;
		let scheduler = UpdateScheduler::new(&state);

		let current_manga = add_manga(&state, source_id, "current").await;
		let current_old = add_chapter(&state, current_manga, "old").await;
		let current_new = add_chapter(&state, current_manga, "new").await;
		scheduler
			.enqueue_new_chapters(1, current_manga, &[current_old, current_new], None)
			.await;
		let current_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE chapter_id IN (?, ?)")
			.bind(current_old)
			.bind(current_new)
			.fetch_one(&state.pool)
			.await
			.expect("current new chapters");
		assert_eq!(current_count, 2, "unread rows in new_ids must not block themselves");

		let old_manga = add_manga(&state, source_id, "old-unread").await;
		let old = add_chapter(&state, old_manga, "old").await;
		let new = add_chapter(&state, old_manga, "new").await;
		scheduler.enqueue_new_chapters(1, old_manga, &[new], None).await;
		let blocked_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE chapter_id = ?")
			.bind(new)
			.fetch_one(&state.pool)
			.await
			.expect("blocked download");
		assert_eq!(blocked_count, 0, "an old chapter without progress is unread");
		set_read(&state, old, 1).await;
		scheduler.enqueue_new_chapters(1, old_manga, &[new], None).await;
		let allowed_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE chapter_id = ?")
			.bind(new)
			.fetch_one(&state.pool)
			.await
			.expect("allowed download");
		assert_eq!(allowed_count, 1);
	}

	#[tokio::test]
	async fn auto_download_reupload_filter_is_strict_and_precedes_limit() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, crate::download::AUTO_DOWNLOAD_NEW_CHAPTERS, serde_json::json!(true)).await;
		set_setting(&state, crate::download::AUTO_SKIP_MANGA_WITH_UNREAD, serde_json::json!(false)).await;
		set_setting(&state, crate::download::AUTO_IGNORE_REUPLOADS, serde_json::json!(true)).await;
		set_setting(&state, crate::download::AUTO_DOWNLOAD_LIMIT, serde_json::json!(1)).await;
		let manga_id = add_manga(&state, source_id, "reupload").await;
		let latest = add_chapter(&state, manga_id, "latest").await;
		let low = add_chapter(&state, manga_id, "low").await;
		let equal = add_chapter(&state, manga_id, "equal").await;
		let high = add_chapter(&state, manga_id, "high").await;
		for (id, number, order) in [(latest, 10.0, 0), (low, 9.0, 1), (equal, 10.0, 2), (high, 11.0, 3)] {
			sqlx::query("UPDATE chapter SET number = ?, source_order = ? WHERE id = ?")
				.bind(number)
				.bind(order)
				.bind(id)
				.execute(&state.pool)
				.await
				.expect("chapter number and order");
		}
		let scheduler = UpdateScheduler::new(&state);
		scheduler
			.enqueue_new_chapters(1, manga_id, &[low, equal, high], Some(10.0))
			.await;
		let mut ids: Vec<i64> = sqlx::query_scalar(
			"SELECT d.chapter_id FROM download d JOIN chapter c ON c.id = d.chapter_id ORDER BY c.source_order ASC",
		)
		.fetch_all(&state.pool)
		.await
		.expect("limited candidates");
		assert_eq!(ids, vec![equal], "the boundary survives while the limit keeps the first source-order candidate");

		set_setting(&state, crate::download::AUTO_DOWNLOAD_LIMIT, serde_json::json!(2)).await;
		scheduler
			.enqueue_new_chapters(1, manga_id, &[low, equal, high], Some(10.0))
			.await;
		ids = sqlx::query_scalar(
			"SELECT d.chapter_id FROM download d JOIN chapter c ON c.id = d.chapter_id ORDER BY c.source_order ASC",
		)
		.fetch_all(&state.pool)
		.await
		.expect("all candidates");
		assert_eq!(ids, vec![equal, high]);
		let low_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE chapter_id = ?")
			.bind(low)
			.fetch_one(&state.pool)
			.await
			.expect("reupload candidate");
		assert_eq!(low_count, 0);
	}

	#[tokio::test]
	async fn default_skip_rules_and_each_can_be_disabled() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let unread = add_manga(&state, source_id, "unread").await;
		let not_started = add_manga(&state, source_id, "not-started").await;
		let completed = add_manga(&state, source_id, "completed").await;
		let eligible = add_manga(&state, source_id, "eligible").await;
		for manga_id in [unread, not_started, completed, eligible] {
			add_to_library(&state, manga_id, 1).await;
		}
		let read_chapter = add_chapter(&state, unread, "read").await;
		add_chapter(&state, unread, "unread").await;
		set_read(&state, read_chapter, 1).await;
		let completed_chapter = add_chapter(&state, completed, "read").await;
		set_read(&state, completed_chapter, 1).await;
		let eligible_chapter = add_chapter(&state, eligible, "read").await;
		set_read(&state, eligible_chapter, 1).await;
		sqlx::query("UPDATE manga SET status = 2 WHERE id = ?")
			.bind(completed)
			.execute(&state.pool)
			.await
			.expect("complete manga");

		assert_eq!(selected_ids(&state, 1, None).await, vec![eligible]);

		set_setting(
			&state,
			"library.skip_titles",
			serde_json::json!(["completed", "notStarted"]),
		)
		.await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![unread, eligible]);

		set_setting(
			&state,
			"library.skip_titles",
			serde_json::json!(["hasUnread", "notStarted"]),
		)
		.await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![completed, eligible]);

		set_setting(
			&state,
			"library.skip_titles",
			serde_json::json!(["hasUnread", "completed"]),
		)
		.await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![not_started, eligible]);

		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		assert_eq!(
			selected_ids(&state, 1, None).await,
			vec![unread, not_started, completed, eligible]
		);
	}

	#[tokio::test]
	async fn read_skip_rules_use_admin_progress() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let started = add_manga(&state, source_id, "started").await;
		let unread = add_manga(&state, source_id, "unread").await;
		for manga_id in [started, unread] {
			add_to_library(&state, manga_id, 1).await;
		}
		for key in ["c1", "c2"] {
			let chapter_id = add_chapter(&state, started, key).await;
			set_read(&state, chapter_id, 1).await;
		}
		add_chapter(&state, unread, "c1").await;

		for rule in ["hasUnread", "notStarted"] {
			set_setting(&state, "library.skip_titles", serde_json::json!([rule])).await;
			assert_eq!(selected_ids(&state, 1, None).await, vec![started], "rule {rule}");
		}
	}

	#[tokio::test]
	async fn future_next_update_time_is_skipped() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		let due = add_manga(&state, source_id, "due").await;
		let future = add_manga(&state, source_id, "future").await;
		let unscheduled = add_manga(&state, source_id, "unscheduled").await;
		for manga_id in [due, future, unscheduled] {
			add_to_library(&state, manga_id, 1).await;
		}
		sqlx::query("UPDATE manga SET next_update_time = unixepoch() - 1 WHERE id = ?")
			.bind(due)
			.execute(&state.pool)
			.await
			.expect("set due time");
		sqlx::query("UPDATE manga SET next_update_time = unixepoch() + 3600 WHERE id = ?")
			.bind(future)
			.execute(&state.pool)
			.await
			.expect("set future time");

		assert_eq!(selected_ids(&state, 1, None).await, vec![due, unscheduled]);

		sqlx::query("UPDATE manga SET next_update_time = unixepoch() - 1 WHERE id = ?")
			.bind(future)
			.execute(&state.pool)
			.await
			.expect("make future manga due");
		assert_eq!(selected_ids(&state, 1, None).await, vec![due, future, unscheduled]);
	}

	#[tokio::test]
	async fn excluded_update_categories_filter() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		let in_excluded = add_manga(&state, source_id, "in-excluded").await;
		let in_both = add_manga(&state, source_id, "in-both").await;
		let untouched = add_manga(&state, source_id, "untouched").await;
		for manga_id in [in_excluded, in_both, untouched] {
			add_to_library(&state, manga_id, 1).await;
		}
		let excluded_category: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name) VALUES (1, 'excluded') RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("insert excluded category");
		let other_category: i64 = sqlx::query_scalar(
			"INSERT INTO category (user_id, name) VALUES (1, 'other') RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("insert other category");
		for (category_id, manga_id) in [(excluded_category, in_excluded), (excluded_category, in_both), (other_category, in_both)] {
			sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
				.bind(category_id)
				.bind(manga_id)
				.execute(&state.pool)
				.await
				.expect("link category");
		}

		// No exclusion yet: all three are selected.
		assert_eq!(selected_ids(&state, 1, None).await, vec![in_excluded, in_both, untouched]);

		// Exclude the category: titles linked to it are skipped, even if they also belong to another category.
		// The setting is user-scoped; user 1 is the only user in this test.
		set_setting(
			&state,
			"user.1.library.excluded_update_categories",
			serde_json::json!([excluded_category]),
		)
		.await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![untouched]);

		// Pulling the excluded category explicitly ignores the exclusion for that category.
		assert_eq!(selected_ids(&state, 1, Some(excluded_category)).await, vec![in_excluded, in_both]);

		// Add another category to the exclusion list and pull the first excluded category again.
		// The pulled category is ignored in the exclusion, but the other excluded category still filters.
		set_setting(
			&state,
			"user.1.library.excluded_update_categories",
			serde_json::json!([excluded_category, other_category]),
		)
		.await;
		assert_eq!(selected_ids(&state, 1, Some(excluded_category)).await, vec![in_excluded]);
	}

	#[tokio::test]
	async fn wasm_refresh_metadata_controls_details_and_preserves_never() {
		let (state, source_id, _tmp) = union_state_with_wasm(Config::default(), partial_wasm()).await;
		let manga_id = add_manga(&state, source_id, "partial").await;
		sqlx::query("UPDATE manga SET description = 'old', update_strategy = 1 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set initial details");
		let scheduler = UpdateScheduler::new(&state);

		update_one_direct(&state, &scheduler, manga_id).await;
		let before: (String, Option<String>, i64) =
			sqlx::query_as("SELECT title, description, update_strategy FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("details before refresh");
		assert_eq!(before, ("Manga partial".into(), Some("old".into()), 1));

		set_setting(&state, "library.refresh_metadata", serde_json::json!(true)).await;
		update_one_direct(&state, &scheduler, manga_id).await;
		let after: (String, Option<String>, i64) =
			sqlx::query_as("SELECT title, description, update_strategy FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("details after refresh");
		assert_eq!(after, ("Partial Title".into(), Some("fetched in pieces".into()), 1));
	}

	#[tokio::test]
	async fn local_refresh_metadata_controls_details() {
		let (state, _, _tmp) = union_state(Config::default()).await;
		let root = state.config.config_dir.join("local");
		let chapter_dir = root.join("Series").join("ch 1");
		std::fs::create_dir_all(&chapter_dir).expect("create local chapter");
		std::fs::write(chapter_dir.join("1.png"), b"page").expect("write local page");
		std::fs::write(
			root.join("Series").join("series.json"),
			r#"{"title":"Local Series","description":"fresh metadata"}"#,
		)
		.expect("write local metadata");
		let source_id: i64 = sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
			.fetch_one(&state.pool)
			.await
			.expect("local source");
		let manga_id = add_manga(&state, source_id, "Series").await;
		sqlx::query("UPDATE manga SET title = 'Stale', description = 'old', update_strategy = 1 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set initial details");
		let scheduler = UpdateScheduler::new(&state);

		update_one_direct(&state, &scheduler, manga_id).await;
		let before: (String, Option<String>) = sqlx::query_as("SELECT title, description FROM manga WHERE id = ?")
			.bind(manga_id)
			.fetch_one(&state.pool)
			.await
			.expect("details before refresh");
		assert_eq!(before, ("Stale".into(), Some("old".into())));

		set_setting(&state, "library.refresh_metadata", serde_json::json!(true)).await;
		update_one_direct(&state, &scheduler, manga_id).await;
		let after: (String, Option<String>, i64) =
			sqlx::query_as("SELECT title, description, update_strategy FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("details after refresh");
		assert_eq!(after, ("Local Series".into(), Some("fresh metadata".into()), 1));
	}

	#[tokio::test]
	async fn failed_fetch_does_not_advance_next_update_time() {
		let (state, _, _tmp) = union_state(Config::default()).await;
		std::fs::create_dir_all(state.config.config_dir.join("local")).expect("create local root");
		let source_id: i64 = sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
			.fetch_one(&state.pool)
			.await
			.expect("local source");
		let manga_id = add_manga(&state, source_id, "Missing").await;
		sqlx::query("UPDATE manga SET next_update_time = 777, chapters_last_fetched_at = 555 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set fetch times");
		let scheduler = UpdateScheduler::new(&state);

		update_one_direct(&state, &scheduler, manga_id).await;

		let times: (Option<i64>, Option<i64>) =
			sqlx::query_as("SELECT next_update_time, chapters_last_fetched_at FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("fetch times");
		assert_eq!(times, (Some(777), Some(555)));
		assert_eq!(scheduler.summary().await.failed, 1);
	}

	#[tokio::test]
	async fn scheduler_stays_alive_when_interval_enabled_at_runtime() {
		let (state, _, _tmp) = union_state(Config::default()).await;
		let scheduler = UpdateScheduler::new(&state);
		let task = tokio::spawn(scheduler.run());
		set_setting(&state, "library.update_interval_hours", serde_json::json!(1)).await;
		tokio::time::sleep(std::time::Duration::from_millis(25)).await;
		assert!(!task.is_finished(), "scheduler exited instead of accepting a runtime interval");
		task.abort();
	}

	#[tokio::test]
	async fn library_settings_load_defaults_and_runtime_values() {
		let config = Config {
			update_interval_hours: 36,
			..Config::default()
		};
		let (state, _, _tmp) = union_state(config).await;
		let settings = LibraryUpdateSettings::load(&state.pool, 1, 36).await.expect("default settings");
		assert_eq!(
			settings,
			LibraryUpdateSettings {
				skip_titles: vec!["hasUnread".into(), "completed".into(), "notStarted".into()],
				refresh_metadata: false,
				update_interval_hours: 36,
				excluded_update_categories: Vec::new(),
			}
		);

		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		set_setting(&state, "library.refresh_metadata", serde_json::json!(true)).await;
		set_setting(&state, "library.update_interval_hours", serde_json::json!(12)).await;
		let settings = LibraryUpdateSettings::load(&state.pool, 1, 36).await.expect("runtime settings");
		assert_eq!(
			settings,
			LibraryUpdateSettings {
				skip_titles: Vec::new(),
				refresh_metadata: true,
				update_interval_hours: 12,
				excluded_update_categories: Vec::new(),
			}
		);
	}

	#[tokio::test]
	async fn scheduled_due_reloads_interval_and_uses_last_run() {
		let (state, _, _tmp) = union_state(Config::default()).await;
		let scheduler = UpdateScheduler::new(&state);
		let started_at = scheduler.started_at;
		assert!(!scheduler.scheduled_due(started_at + 7200).await.expect("disabled due"));

		set_setting(&state, "library.update_interval_hours", serde_json::json!(1)).await;
		assert!(!scheduler.scheduled_due(started_at + 3599).await.expect("early due"));
		assert!(scheduler.scheduled_due(started_at + 3600).await.expect("interval due"));
		*scheduler.last_scheduled_run_at.write().await = Some(started_at + 1800);
		assert!(!scheduler.scheduled_due(started_at + 3600).await.expect("last run early"));
		assert!(scheduler.scheduled_due(started_at + 5400).await.expect("last run due"));

		set_setting(&state, "library.update_interval_hours", serde_json::json!(0)).await;
		assert!(!scheduler.scheduled_due(started_at + 100_000).await.expect("disabled again"));
	}

	#[tokio::test]
	async fn successful_fetch_saves_source_next_update_time() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "source-time").await;
		add_to_library(&state, manga_id, 1).await;
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.next_update_time = Some(123_456);
		updated.chapters = Some(Vec::new());
		let before: (i64, i64) = sqlx::query_as(
			"SELECT last_modified_at, (SELECT COUNT(*) FROM sync_change) FROM manga WHERE id = ?",
		)
		.bind(manga_id)
		.fetch_one(&state.pool)
		.await
		.expect("change state before fetch");

		persist_at(&state, manga_id, updated, 24, 1000).await;

		let times: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
			"SELECT next_update_time, chapters_last_fetched_at, next_update_is_fallback FROM manga WHERE id = ?",
		)
		.bind(manga_id)
		.fetch_one(&state.pool)
		.await
		.expect("fetch times");
		assert_eq!(times, (Some(123_456), Some(1000), 0));
		let after: (i64, i64) = sqlx::query_as(
			"SELECT last_modified_at, (SELECT COUNT(*) FROM sync_change) FROM manga WHERE id = ?",
		)
		.bind(manga_id)
		.fetch_one(&state.pool)
		.await
		.expect("change state after fetch");
		assert_eq!(after, before);
	}

	#[tokio::test]
	async fn successful_fetch_derives_or_clears_next_update_time() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "derived-time").await;
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.chapters = Some(Vec::new());
		persist_at(&state, manga_id, updated, 2, 1000).await;
		let derived: (Option<i64>, i64) =
			sqlx::query_as("SELECT next_update_time, next_update_is_fallback FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("derived next update");
		assert_eq!(derived, (Some(8200), 1));

		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("updated manga row");
		let mut updated = manga_from_row(&row);
		updated.next_update_time = None;
		updated.chapters = Some(Vec::new());
		persist_at(&state, manga_id, updated, 0, 2000).await;
		let times: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
			"SELECT next_update_time, chapters_last_fetched_at, next_update_is_fallback FROM manga WHERE id = ?",
		)
		.bind(manga_id)
		.fetch_one(&state.pool)
		.await
		.expect("cleared fetch times");
		assert_eq!(times, (None, Some(2000), 0));
	}

	#[tokio::test]
	async fn next_update_time_saturates_on_overflow() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "overflow-time").await;
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.chapters = Some(Vec::new());

		persist_at(&state, manga_id, updated, u64::MAX, i64::MAX).await;

		let next: (Option<i64>, i64) =
			sqlx::query_as("SELECT next_update_time, next_update_is_fallback FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("saturated next update");
		assert_eq!(next, (Some(i64::MAX), 1));
	}

	#[tokio::test]
	async fn manual_summary_time_does_not_defer_scheduled_due() {
		let (state, _, _tmp) = union_state(Config::default()).await;
		let scheduler = UpdateScheduler::new(&state);
		let started_at = scheduler.started_at;
		set_setting(&state, "library.update_interval_hours", serde_json::json!(1)).await;
		let guard = scheduler.try_acquire().expect("manual fetch guard");
		scheduler.fetch(1, None, guard).await;
		assert!(scheduler.summary().await.last_run_at.is_some());
		scheduler.summary.write().await.last_run_at = Some(started_at + 100_000);

		assert!(scheduler.scheduled_due(started_at + 3600).await.expect("manual time must not defer"));
	}

	#[tokio::test]
	async fn fallback_schedule_uses_current_interval() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		set_setting(&state, "library.skip_titles", serde_json::json!([])).await;
		let manga_id = add_manga(&state, source_id, "current-interval").await;
		add_to_library(&state, manga_id, 1).await;
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.chapters = Some(Vec::new());
		persist_at(&state, manga_id, updated, 24, unix_now() - 7200).await;

		set_setting(&state, "library.update_interval_hours", serde_json::json!(1)).await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![manga_id]);
		set_setting(&state, "library.update_interval_hours", serde_json::json!(24)).await;
		assert_eq!(selected_ids(&state, 1, None).await, Vec::<i64>::new());
		set_setting(&state, "library.update_interval_hours", serde_json::json!(0)).await;
		assert_eq!(selected_ids(&state, 1, None).await, vec![manga_id]);
	}

	#[tokio::test]
	async fn echoed_source_time_derives_fallback_schedule() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "echoed-time").await;
		sqlx::query("UPDATE manga SET next_update_time = 111 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set old source time");
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.chapters = Some(Vec::new());

		persist_at(&state, manga_id, updated, 2, 1000).await;

		let times: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
			"SELECT next_update_time, chapters_last_fetched_at, next_update_is_fallback FROM manga WHERE id = ?",
		)
		.bind(manga_id)
		.fetch_one(&state.pool)
		.await
		.expect("echoed fetch times");
		assert_eq!(times, (Some(8200), Some(1000), 1));
	}

	#[tokio::test]
	async fn upsert_source_details_reset_fallback_provenance() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "upsert-provenance").await;
		sqlx::query("UPDATE manga SET next_update_time = 111, next_update_is_fallback = 1 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set fallback provenance");
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let mut updated = manga_from_row(&row);
		updated.next_update_time = Some(999);

		let mut tx = crate::db::begin_write(&state.pool).await.expect("write transaction");
		upsert_manga(&mut tx, source_id, &updated, true, true, false)
			.await
			.expect("upsert source details");
		tx.commit().await.expect("commit source details");

		let schedule: (Option<i64>, i64) =
			sqlx::query_as("SELECT next_update_time, next_update_is_fallback FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("source schedule");
		assert_eq!(schedule, (Some(999), 0));
	}

	#[tokio::test]
	async fn upsert_echoed_details_preserve_fallback_provenance() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "upsert-echoed-provenance").await;
		sqlx::query("UPDATE manga SET next_update_time = 111, next_update_is_fallback = 1 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("set fallback provenance");
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let updated = manga_from_row(&row);

		let mut tx = crate::db::begin_write(&state.pool).await.expect("write transaction");
		upsert_manga(&mut tx, source_id, &updated, true, true, false)
			.await
			.expect("upsert echoed details");
		tx.commit().await.expect("commit echoed details");

		let schedule: (Option<i64>, i64) =
			sqlx::query_as("SELECT next_update_time, next_update_is_fallback FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("echoed source schedule");
		assert_eq!(schedule, (Some(111), 1));
	}

	#[tokio::test]
	async fn persist_refresh_uses_current_update_strategy() {
		let (state, source_id, _tmp) = union_state(Config::default()).await;
		let manga_id = add_manga(&state, source_id, "current-strategy").await;
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("pre-fetch row");
		sqlx::query("UPDATE manga SET update_strategy = 1 WHERE id = ?")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("mark never during fetch");
		let mut updated = manga_from_row(&row);
		updated.description = Some("fresh".into());
		updated.chapters = Some(Vec::new());

		let settings = LibraryUpdateSettings {
			skip_titles: Vec::new(),
			refresh_metadata: true,
			update_interval_hours: 0,
			excluded_update_categories: Vec::new(),
		};
		let mut tx = crate::db::begin_write(&state.pool).await.expect("write transaction");
		persist_success(&mut tx, &row, &updated, &[], &settings, 1000)
			.await
			.expect("persist successful fetch");
		tx.commit().await.expect("commit successful fetch");

		let details: (i64, Option<String>) =
			sqlx::query_as("SELECT update_strategy, description FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("current strategy");
		assert_eq!(details, (1, Some("fresh".into())));
	}

	async fn persist_at(
		state: &AppState,
		manga_id: i64,
		updated: aidoku::Manga,
		interval_hours: u64,
		fetched_at: i64,
	) {
		persist_at_with_refresh(state, manga_id, updated, interval_hours, fetched_at, false).await;
	}

	async fn persist_at_with_refresh(
		state: &AppState,
		manga_id: i64,
		updated: aidoku::Manga,
		interval_hours: u64,
		fetched_at: i64,
		refresh_metadata: bool,
	) {
		let row = crate::source::manga_row(&state.pool, manga_id).await.expect("manga row");
		let settings = LibraryUpdateSettings {
			skip_titles: Vec::new(),
			refresh_metadata,
			update_interval_hours: interval_hours,
			excluded_update_categories: Vec::new(),
		};
		let mut tx = crate::db::begin_write(&state.pool).await.expect("write transaction");
		persist_success(&mut tx, &row, &updated, &[], &settings, fetched_at)
			.await
			.expect("persist successful fetch");
		tx.commit().await.expect("commit successful fetch");
	}
}
