use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterRow, MangaRow, PageRow, unix_now};
use crate::source::page_dimensions::{DIMENSION_HEAD_BYTES, PageDimensionUpdate, measure_with_fallback, persist_downloaded as persist_page_dimensions};
use crate::source::{chapter_row, fetch_page_image, http_get, manga_row};
use crate::state::AppState;
use aidoku::PageContext;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio::task::JoinSet;

/// Per-chapter retry cap (first attempt not counted), backoff 2/4/8s
const MAX_CHAPTER_RETRIES: u32 = 3;
/// Parallel page-image cap within a chapter
const PAGE_CONCURRENCY: usize = 5;

pub const AUTO_DOWNLOAD_NEW_CHAPTERS: &str = "auto_download_new_chapters";
pub const AUTO_DOWNLOAD_LIMIT: &str = "auto_download_limit";
pub const DOWNLOAD_FORMAT: &str = "download_format";
pub const DOWNLOAD_CONCURRENCY: &str = "download_concurrency";
pub const DELETE_AFTER_MANUAL_READ: &str = "delete_after_manual_read";
pub const DELETE_AFTER_READ: &str = "delete_after_read";
pub const ALLOW_DELETE_BOOKMARKED: &str = "allow_delete_bookmarked";
pub const AUTO_SKIP_MANGA_WITH_UNREAD: &str = "downloads.auto_skip_manga_with_unread";
pub const AUTO_IGNORE_REUPLOADS: &str = "downloads.auto_ignore_reuploads";
pub const DOWNLOAD_AHEAD: &str = "downloads.download_ahead";
pub const DOWNLOAD_AHEAD_COUNT: &str = "downloads.download_ahead_count";
pub const LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES: &str = "library.excluded_download_categories";

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadSettings {
	pub auto_download_new_chapters: bool,
	pub auto_download_limit: i64,
	pub download_format: String,
	pub download_concurrency: usize,
	pub delete_after_manual_read: bool,
	pub delete_after_read: bool,
	pub allow_delete_bookmarked: bool,
	pub auto_skip_manga_with_unread: bool,
	pub auto_ignore_reuploads: bool,
	pub download_ahead: bool,
	pub download_ahead_count: i64,
	pub excluded_download_categories: Vec<i64>,
}

impl DownloadSettings {
	pub async fn load(pool: &SqlitePool, user_id: i64, config: &crate::config::Config) -> ApiResult<Self> {
		let excluded_key = format!("user.{user_id}.{LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES}");
		let rows: Vec<(String, String)> = sqlx::query_as(
			"SELECT key, value FROM setting WHERE key IN (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		)
		.bind(AUTO_DOWNLOAD_NEW_CHAPTERS)
		.bind(AUTO_DOWNLOAD_LIMIT)
		.bind(DOWNLOAD_FORMAT)
		.bind(DOWNLOAD_CONCURRENCY)
		.bind(DELETE_AFTER_MANUAL_READ)
		.bind(DELETE_AFTER_READ)
		.bind(ALLOW_DELETE_BOOKMARKED)
		.bind(AUTO_SKIP_MANGA_WITH_UNREAD)
		.bind(AUTO_IGNORE_REUPLOADS)
		.bind(DOWNLOAD_AHEAD)
		.bind(DOWNLOAD_AHEAD_COUNT)
		.bind(&excluded_key)
		.fetch_all(pool)
		.await?;
		let values: HashMap<String, Value> = rows
			.into_iter()
			.filter_map(|(key, raw)| serde_json::from_str(&raw).ok().map(|value| (key, value)))
			.collect();
		let boolean = |key: &str, fallback: bool| {
			values.get(key).and_then(Value::as_bool).unwrap_or(fallback)
		};
		let format = values
			.get(DOWNLOAD_FORMAT)
			.and_then(Value::as_str)
			.unwrap_or(&config.download_format);
		Ok(Self {
			auto_download_new_chapters: boolean(AUTO_DOWNLOAD_NEW_CHAPTERS, config.auto_download_new_chapters),
			auto_download_limit: values
				.get(AUTO_DOWNLOAD_LIMIT)
				.and_then(Value::as_i64)
				.filter(|value| *value >= 0)
				.unwrap_or(config.auto_download_limit as i64),
			download_format: if format == "folder" { "folder".into() } else { "cbz".into() },
			download_concurrency: values
				.get(DOWNLOAD_CONCURRENCY)
				.and_then(Value::as_u64)
				.and_then(|value| usize::try_from(value).ok())
				.unwrap_or(config.download_concurrency)
				.max(1),
			delete_after_manual_read: boolean(DELETE_AFTER_MANUAL_READ, false),
			delete_after_read: boolean(DELETE_AFTER_READ, false),
			allow_delete_bookmarked: boolean(ALLOW_DELETE_BOOKMARKED, false),
			auto_skip_manga_with_unread: boolean(AUTO_SKIP_MANGA_WITH_UNREAD, true),
			auto_ignore_reuploads: boolean(AUTO_IGNORE_REUPLOADS, false),
			download_ahead: boolean(DOWNLOAD_AHEAD, false),
			download_ahead_count: values
				.get(DOWNLOAD_AHEAD_COUNT)
				.and_then(Value::as_i64)
				.filter(|value| *value >= 0)
				.unwrap_or(2),
			excluded_download_categories: values
				.get(&excluded_key)
				.filter(|value| {
					value.as_array().is_some_and(|ids| ids.iter().all(|id| id.as_i64().is_some()))
				})
				.and_then(|value| serde_json::from_value::<Vec<i64>>(value.clone()).ok())
				.unwrap_or_default(),
		})
	}
}

fn valid_excluded_categories(value: &Value) -> bool {
	value.as_array().is_some_and(|ids| ids.iter().all(|id| id.as_i64().is_some()))
}

pub(crate) fn validate_download_setting(key: &str, value: &Value) -> Option<ApiResult<()>> {
	let result = match key {
		AUTO_DOWNLOAD_NEW_CHAPTERS | DELETE_AFTER_MANUAL_READ | DELETE_AFTER_READ | ALLOW_DELETE_BOOKMARKED
		| AUTO_SKIP_MANGA_WITH_UNREAD | AUTO_IGNORE_REUPLOADS | DOWNLOAD_AHEAD if !value.is_boolean() => {
			Err(crate::error::ApiError::bad_request(format!("{key} must be a boolean")))
		}
		AUTO_DOWNLOAD_LIMIT | DOWNLOAD_AHEAD_COUNT if value.as_u64().is_none() => {
			Err(crate::error::ApiError::bad_request(format!("{key} must be a non-negative integer")))
		}
		DOWNLOAD_CONCURRENCY if !matches!(value.as_u64(), Some(1..)) => {
			Err(crate::error::ApiError::bad_request("download_concurrency must be a positive integer"))
		}
		DOWNLOAD_FORMAT if !matches!(value.as_str(), Some("cbz" | "folder")) => {
			Err(crate::error::ApiError::bad_request("download_format must be cbz or folder"))
		}
		LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES if !valid_excluded_categories(value) => {
			Err(crate::error::ApiError::bad_request("library.excluded_download_categories must be an array of category ids"))
		}
		AUTO_DOWNLOAD_NEW_CHAPTERS | AUTO_DOWNLOAD_LIMIT | DOWNLOAD_FORMAT | DOWNLOAD_CONCURRENCY
		| DELETE_AFTER_MANUAL_READ | DELETE_AFTER_READ | ALLOW_DELETE_BOOKMARKED | AUTO_SKIP_MANGA_WITH_UNREAD
		| AUTO_IGNORE_REUPLOADS | DOWNLOAD_AHEAD | DOWNLOAD_AHEAD_COUNT | LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES => Ok(()),
		_ => return None,
	};
	Some(result)
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadEvent {
	#[serde(rename = "type")]
	pub kind: &'static str,
	#[serde(rename = "chapterId")]
	pub chapter_id: i64,
	pub downloaded: i64,
	pub total: i64,
	/// Carried only by running events (kind="running", chapter_id=0); chapter events always have None, kept off the wire
	#[serde(skip_serializing_if = "Option::is_none")]
	pub running: Option<bool>,
	/// Owning user of the row; event streams filter per connection user. running events are global manager state (None, broadcast to all)
	#[serde(skip)]
	pub user_id: Option<i64>,
}

/// In-memory queue entry: no request context, so the row's ownership travels with the entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueEntry {
	chapter_id: i64,
	user_id: i64,
}

/// Download manager: serial within a source, parallel across sources, with a runtime cap on concurrent sources.
/// The state machine lives in the download table (queued → downloading → (paused) → finished / error), recoverable across restarts.
pub struct DownloadManager {
	// Weak prevents a reference cycle: AppState holds this manager through a OnceCell
	state: Weak<AppState>,
	pool: SqlitePool,
	queue: Mutex<VecDeque<QueueEntry>>,
	notify: Notify,
	events: broadcast::Sender<DownloadEvent>,
	running: AtomicBool,
	active_sources: Mutex<HashSet<i64>>,
	progress: Mutex<HashMap<i64, (i64, i64)>>,
	cancel: Mutex<HashSet<i64>>,
	/// An orphaned old artifact path from an active deletion is removed by its
	/// worker's dispatch owner after the attempt exits, never during packaging.
	cancelled_paths: Mutex<HashMap<i64, String>>,
	/// Artifact paths held by in-flight downloads, keyed to the owning chapter: two sources can run at once, so a
	/// finished row alone cannot tell a running download that another chapter is about to package the same path
	claimed_paths: Mutex<HashMap<String, i64>>,
	downloads_dir: PathBuf,
}

enum FinishMode {
	Update(i64),
	Upsert,
}

async fn persist_finished(
	pool: &SqlitePool,
	chapter_id: i64,
	user_id: i64,
	path: &str,
	format: &str,
	mode: FinishMode,
) -> Result<bool, sqlx::Error> {
	let now = unix_now();
	let mut tx = crate::db::begin_write(pool).await?;
	let applied = match mode {
		FinishMode::Update(attempt_id) => {
			sqlx::query(
				"UPDATE download SET status = 'finished', path = ?, error = NULL, dismissed_at = NULL, updated_at = ?
				 WHERE chapter_id = ? AND user_id = ? AND status = 'downloading' AND updated_at = ?",
			)
			.bind(path)
			.bind(now)
			.bind(chapter_id)
			.bind(user_id)
			.bind(attempt_id)
			.execute(&mut *tx)
			.await?
			.rows_affected() == 1
		}
		FinishMode::Upsert => {
			sqlx::query(
				"INSERT INTO download (chapter_id, user_id, status, format, path, created_at, updated_at, dismissed_at)
				 VALUES (?, ?, 'finished', ?, ?, ?, ?, NULL)
				 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
				 status = 'finished', format = excluded.format, path = excluded.path,
				 error = NULL, retries = 0, dismissed_at = NULL, updated_at = excluded.updated_at",
			)
			.bind(chapter_id)
			.bind(user_id)
			.bind(format)
			.bind(path)
			.bind(now)
			.bind(now)
			.execute(&mut *tx)
			.await?;
			true
		}
	};
	if !applied {
		tx.rollback().await?;
		return Ok(false);
	}
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 1)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 1",
	)
	.bind(chapter_id)
	.bind(user_id)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(true)
}

impl DownloadManager {
	pub fn new(state: &Arc<AppState>) -> Arc<Self> {
		Arc::new(Self {
			state: Arc::downgrade(state),
			pool: state.pool.clone(),
			queue: Mutex::new(VecDeque::new()),
			notify: Notify::new(),
			events: broadcast::channel(256).0,
			running: AtomicBool::new(true),
			active_sources: Mutex::new(HashSet::new()),
			progress: Mutex::new(HashMap::new()),
			cancel: Mutex::new(HashSet::new()),
			cancelled_paths: Mutex::new(HashMap::new()),
			claimed_paths: Mutex::new(HashMap::new()),
			downloads_dir: state.config.downloads_dir.clone(),
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
		self.events.subscribe()
	}

	pub fn downloads_dir(&self) -> &Path {
		&self.downloads_dir
	}

	async fn settings(&self, user_id: i64) -> ApiResult<DownloadSettings> {
		let state = self
			.state
			.upgrade()
			.ok_or_else(|| crate::error::ApiError::bad_request("server is shutting down"))?;
		DownloadSettings::load(&self.pool, user_id, &state.config).await
	}

	pub fn is_paused(&self) -> bool {
		!self.running.load(Ordering::SeqCst)
	}

	pub fn is_running(&self) -> bool {
		self.running.load(Ordering::SeqCst)
	}

	pub async fn progress_of(&self, chapter_id: i64) -> Option<(i64, i64)> {
		self.progress.lock().await.get(&chapter_id).copied()
	}

	fn emit(&self, kind: &'static str, chapter_id: i64, downloaded: i64, total: i64, user_id: i64) {
		let _ = self.events.send(DownloadEvent {
			kind,
			chapter_id,
			downloaded,
			total,
			running: None,
			user_id: Some(user_id),
		});
	}

	fn emit_running(&self, running: bool) {
		let _ = self.events.send(DownloadEvent {
			kind: "running",
			chapter_id: 0,
			downloaded: 0,
			total: 0,
			running: Some(running),
			user_id: None,
		});
	}

	/// Existing artifact for the same chapter: reuse it and settle straight to finished
	async fn shared_artifact(&self, chapter_id: i64) -> Option<(String, String)> {
		let row: Option<(String, String)> = sqlx::query_as(
			"SELECT path, format FROM download
			 WHERE chapter_id = ? AND status = 'finished' AND path IS NOT NULL
			 ORDER BY updated_at DESC LIMIT 1",
		)
		.bind(chapter_id)
		.fetch_optional(&self.pool)
		.await
		.ok()?;
		let (path, format) = row?;
		let exists = tokio::fs::try_exists(product_path(&self.downloads_dir, &path)).await.ok()?;
		exists.then_some((path, format))
	}

	/// Settles a row reusing a shared artifact: no repackaging, the row goes straight to finished and gets the download flag
	async fn settle_finished(&self, chapter_id: i64, user_id: i64, path: &str, format: &str) {
		let result = persist_finished(&self.pool, chapter_id, user_id, path, format, FinishMode::Upsert).await;
		if let Err(e) = result { tracing::warn!(chapter_id, "download shared-artifact settle failed: {e}"); return; }
		self.cleanup_page_cache_after_finish(chapter_id, 0, 0, user_id).await;
	}

	/// A committed product supersedes derived/source page assets. Recheck under the chapter write
	/// lease so a concurrent removal or requeue cannot clean files for a stale completion.
	async fn cleanup_page_cache_after_finish(&self, chapter_id: i64, done: i64, total: i64, user_id: i64) -> bool {
		self.cleanup_page_cache_after_finish_with_failure(chapter_id, done, total, user_id, None).await
	}

	async fn cleanup_page_cache_after_finish_with_failure(
		&self,
		chapter_id: i64,
		done: i64,
		total: i64,
		user_id: i64,
		fail_on: Option<PathBuf>,
	) -> bool {
		let Some(state) = self.state.upgrade() else { return false; };
		let _cache_lease = state.chapter_cache_gate.read().await;
		let chapter_lock = state.chapter_cache_lock(chapter_id).await;
		let _chapter = chapter_lock.write().await;
		let product = match crate::download::finished_product(&state, chapter_id).await {
			Ok(Some(product)) => product,
			Ok(None) => return false,
			Err(error) => { tracing::warn!(chapter_id, "finished download recheck failed: {}", error.message()); return false; }
		};
		if let Err(error) = crate::download::product_entry_names(&state, &product).await {
			tracing::warn!(chapter_id, "finished download artifact is not usable; page cache retained: {}", error.message());
			return false;
		}
		let mut failures = Vec::new();
		for path in [
			state.config.page_blobs_dir().join(chapter_id.to_string()),
			state.config.page_blobs_dir().join(format!("{chapter_id}.previous")),
			state.config.page_blobs_dir().join(format!("{chapter_id}.staging")),
			state.config.opds_cbz_dir().join(chapter_id.to_string()),
			state.config.opds_cbz_dir().join(format!("{chapter_id}.previous")),
			state.config.opds_cbz_dir().join(format!("{chapter_id}.staging")),
		] {
			let removal = if fail_on.as_deref() == Some(path.as_path()) {
				Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected cache cleanup failure"))
			} else {
				match tokio::fs::symlink_metadata(&path).await {
				Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
					tokio::fs::remove_dir_all(&path).await
				}
				Ok(_) => tokio::fs::remove_file(&path).await,
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
				Err(error) => Err(error),
				}
			};
			if let Err(error) = removal
				&& error.kind() != std::io::ErrorKind::NotFound
			{
				failures.push(format!("{}: {error}", path.display()));
			}
		}
		let publish = crate::cache::publish_chapter(&state, chapter_id).await;
		let message = if let Err(error) = publish { Some(error.message().to_string()) }
			else if failures.is_empty() { None }
			else { Some(format!("download cache cleanup incomplete: {}", failures.join("; "))) };
		if let Some(message) = message {
			tracing::warn!(chapter_id, error = %message, "finished download cache cleanup failed");
			let _ = sqlx::query("UPDATE chapter_cache_state SET last_error=? WHERE chapter_id=?").bind(message).bind(chapter_id).execute(&self.pool).await;
		}
		self.emit("finished", chapter_id, done, total, user_id);
		true
	}

	/// Enqueues the deduped chapter list; rows belong to user_id, already finished/queued/running ones are skipped, error rows are reset and requeued
	pub async fn enqueue(self: &Arc<Self>, chapter_ids: &[i64], user_id: i64) -> Vec<i64> {
		let settings = match self.settings(user_id).await {
			Ok(settings) => settings,
			Err(e) => {
				tracing::warn!("download settings load failed: {e:?}");
				return Vec::new();
			}
		};
		let format = settings.download_format.as_str();
		let now = unix_now();
		let mut seen = HashSet::new();
		let mut queued = Vec::new();
		for &chapter_id in chapter_ids {
			if !seen.insert(chapter_id) {
				continue;
			}
			if let Some((path, shared_format)) = self.shared_artifact(chapter_id).await {
				self.settle_finished(chapter_id, user_id, &path, &shared_format).await;
				continue;
			}
			let status: Option<(String,)> =
				match sqlx::query_as("SELECT status FROM download WHERE chapter_id = ? AND user_id = ?")
					.bind(chapter_id)
					.bind(user_id)
					.fetch_optional(&self.pool)
					.await
				{
					Ok(row) => row,
					Err(e) => {
						tracing::warn!(chapter_id, "download enqueue query failed: {e}");
						continue;
					}
				};
			match status.map(|(s,)| s) {
				Some(s) if matches!(s.as_str(), "queued" | "downloading" | "paused" | "finished") => {
					continue;
				}
				Some(_) => {
					let result = sqlx::query(
						"UPDATE download SET status = 'queued', error = NULL, retries = 0, dismissed_at = NULL,
						 format = ?, updated_at = ? WHERE chapter_id = ? AND user_id = ?",
					)
					.bind(format)
					.bind(now)
					.bind(chapter_id)
					.bind(user_id)
					.execute(&self.pool)
					.await;
					if let Err(e) = result {
						tracing::warn!(chapter_id, "download requeue failed: {e}");
						continue;
					}
				}
				None => {
					// The FK rejects when the chapter does not exist; skip it
					let result = sqlx::query(
						"INSERT INTO download (chapter_id, user_id, status, format, created_at, updated_at)
						 VALUES (?, ?, 'queued', ?, ?, ?)",
					)
					.bind(chapter_id)
					.bind(user_id)
					.bind(format)
					.bind(now)
					.bind(now)
					.execute(&self.pool)
					.await;
					if let Err(e) = result {
						tracing::warn!(chapter_id, "download enqueue failed: {e}");
						continue;
					}
				}
			}
			self.queue.lock().await.push_back(QueueEntry { chapter_id, user_id });
			queued.push(chapter_id);
			self.emit("queued", chapter_id, 0, 0, user_id);
		}
		if !queued.is_empty() {
			tracing::info!(requested = chapter_ids.len(), queued = queued.len(), "downloads queued");
			self.notify.notify_one();
		}
		queued
	}

	/// Startup recovery: rows left downloading by the last process exit count as unfinished and go back to queued
	pub async fn recover(self: &Arc<Self>) {
		let result = sqlx::query(
			"UPDATE download SET status = 'queued', updated_at = ? WHERE status = 'downloading'",
		)
		.bind(unix_now())
		.execute(&self.pool)
		.await;
		if let Err(e) = result {
			tracing::warn!("download recover failed: {e}");
		}
		self.reload_queue().await;
	}

	async fn reload_queue(self: &Arc<Self>) {
		let entries: Vec<QueueEntry> = match sqlx::query_as(
			"SELECT chapter_id, user_id FROM download WHERE status = 'queued'
			 ORDER BY created_at ASC, chapter_id ASC, user_id ASC",
		)
		.fetch_all(&self.pool)
		.await
		{
			Ok(rows) => rows
				.into_iter()
				.map(|(chapter_id, user_id)| QueueEntry { chapter_id, user_id })
				.collect(),
			Err(e) => {
				tracing::warn!("download queue reload failed: {e}");
				return;
			}
		};
		{
			let mut queue = self.queue.lock().await;
			queue.clear();
			queue.extend(entries);
		}
		self.notify.notify_one();
	}

	/// stop only stops scheduling: running chapters finish, unstarted queued rows drop to paused; start re-feeds paused rows
	pub async fn set_running(self: &Arc<Self>, running: bool) {
		if running {
			self.running.store(true, Ordering::SeqCst);
			self.emit_running(true);
			let result = sqlx::query(
				"UPDATE download SET status = 'queued', updated_at = ? WHERE status = 'paused'",
			)
			.bind(unix_now())
			.execute(&self.pool)
			.await;
			if let Err(e) = result {
				tracing::warn!("download resume failed: {e}");
			}
			self.reload_queue().await;
		} else {
			self.running.store(false, Ordering::SeqCst);
			self.emit_running(false);
			let rows: Vec<(i64, i64)> =
				sqlx::query_as("SELECT chapter_id, user_id FROM download WHERE status = 'queued'")
					.fetch_all(&self.pool)
					.await
					.unwrap_or_default();
			if rows.is_empty() {
				return;
			}
			let result = sqlx::query(
				"UPDATE download SET status = 'paused', updated_at = ? WHERE status = 'queued'",
			)
			.bind(unix_now())
			.execute(&self.pool)
			.await;
			if let Err(e) = result {
				tracing::warn!("download pause failed: {e}");
			}
			self.queue.lock().await.clear();
			for (chapter_id, user_id) in rows {
				self.emit("paused", chapter_id, 0, 0, user_id);
			}
		}
	}

	pub async fn mark_cancel(&self, chapter_id: i64) {
		self.cancel.lock().await.insert(chapter_id);
	}

	pub async fn remove_from_queue(&self, chapter_id: i64, user_id: i64) {
		self.queue
			.lock()
			.await
			.retain(|e| e.chapter_id != chapter_id || e.user_id != user_id);
	}

	pub(crate) async fn cleanup_tmp(&self, chapter_id: i64) {
		let _ =
			tokio::fs::remove_dir_all(self.downloads_dir.join(".tmp").join(chapter_id.to_string()))
				.await;
	}

	/// Remove an old orphan artifact only after the active worker has exited. The
	/// chapter lease protects a concurrent finished-product read; references are
	/// rechecked after acquiring it, with no database transaction held while waiting.
	async fn cleanup_cancelled_artifact(&self, chapter_id: i64) {
		let Some(path) = self.cancelled_paths.lock().await.remove(&chapter_id) else { return; };
		let state = self.state.upgrade();
		let _cache_lease = if let Some(state) = &state { Some(state.chapter_cache_gate.read().await) } else { None };
		let chapter_lock = if let Some(state) = &state { Some(state.chapter_cache_lock(chapter_id).await) } else { None };
		let _chapter = if let Some(lock) = &chapter_lock { Some(lock.write().await) } else { None };
		let claimed = self.claimed_paths.lock().await;
		if claimed.get(&path).is_some_and(|owner| *owner != chapter_id) {
			return;
		}
		let referenced = sqlx::query_scalar::<_, i64>("SELECT 1 FROM download WHERE path = ? LIMIT 1")
			.bind(&path).fetch_optional(&self.pool).await;
		match referenced {
			Ok(None) => { let _ = remove_path(&product_path(&self.downloads_dir, &path)).await; }
			Ok(Some(_)) => {}
			Err(error) => tracing::warn!(chapter_id, "active-delete artifact reference check failed: {error}"),
		}
	}

	async fn cleanup_attempt(&self, chapter_id: i64, path: Option<&str>) {
		self.progress.lock().await.remove(&chapter_id);
		self.cancel.lock().await.remove(&chapter_id);
		self.cleanup_tmp(chapter_id).await;
		let Some(path) = path else { return };
		let referenced = sqlx::query_as::<_, (i64,)>("SELECT 1 FROM download WHERE path = ? LIMIT 1")
			.bind(path)
			.fetch_optional(&self.pool)
			.await;
		match referenced {
			Ok(None) => {
				let _ = remove_path(&product_path(&self.downloads_dir, path)).await;
			}
			Ok(Some(_)) => {}
			Err(e) => tracing::warn!(chapter_id, "download artifact reference check failed: {e}"),
		}
	}

	pub async fn delete(&self, chapter_id: i64, user_id: i64) -> ApiResult<bool> {
        self.delete_guarded(chapter_id, user_id, false).await
    }

    /// Delete a product, optionally only while its row is still finished. The state test and
    /// deletion share one SQLite write transaction, so finishedOnly cannot race a requeue.
    pub async fn delete_guarded(
        &self,
        chapter_id: i64,
        user_id: i64,
        finished_only: bool,
    ) -> ApiResult<bool> {
		loop {
			let status: Option<String> = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id=? AND user_id=?")
				.bind(chapter_id).bind(user_id).fetch_optional(&self.pool).await?;
			let Some(status) = status else { return Ok(false); };
			if finished_only && status != "finished" { return Ok(false); }
			if status != "finished" {
				match self.delete_nonfinished_row(chapter_id, user_id).await? {
					Some(deleted) => return Ok(deleted),
					None => continue,
				}
			}

			// Only a finished product is authoritative for readers and needs exclusive
			// filesystem/cache removal. Never hold a DB transaction while waiting.
			let state = self.state.upgrade();
			let _cache_lease = if let Some(state) = &state {
				let mut read = Box::pin(state.chapter_cache_gate.read());
				tokio::select! {
					biased;
					guard = &mut read => Some(guard),
					_ = tokio::task::yield_now() => {
						let current: Option<String> = sqlx::query_scalar(
							"SELECT status FROM download WHERE chapter_id=? AND user_id=?",
						)
						.bind(chapter_id).bind(user_id).fetch_optional(&self.pool).await?;
						if current.as_deref() != Some("finished") {
							drop(read);
							if finished_only { return Ok(false); }
							continue;
						}
						Some(read.await)
					}
				}
			} else { None };
			let chapter_lock = if let Some(state) = &state { Some(state.chapter_cache_lock(chapter_id).await) } else { None };
			let _chapter = if let Some(lock) = &chapter_lock {
				let mut write = Box::pin(lock.clone().write_owned());
				tokio::select! {
					biased;
					guard = &mut write => Some(guard),
					_ = tokio::task::yield_now() => {
						// The writer is now queued. Recheck state without holding a DB
						// transaction; fair writer ordering prevents a new worker read lease
						// from slipping in after this check.
						let current: Option<String> = sqlx::query_scalar(
							"SELECT status FROM download WHERE chapter_id=? AND user_id=?",
						)
						.bind(chapter_id).bind(user_id).fetch_optional(&self.pool).await?;
						if current.as_deref() != Some("finished") {
							drop(write);
							if finished_only { return Ok(false); }
							continue;
						}
						Some(write.await)
					}
				}
			} else { None };
			let mut tx = crate::db::begin_write(&self.pool).await?;
			let row: Option<(String, Option<String>)> =
				sqlx::query_as("SELECT status, path FROM download WHERE chapter_id = ? AND user_id = ?")
					.bind(chapter_id).bind(user_id).fetch_optional(&mut *tx).await?;
			let Some((status, path)) = row else { tx.rollback().await?; return Ok(false); };
			if finished_only && status != "finished" { tx.rollback().await?; return Ok(false); }
			if status != "finished" {
				tx.rollback().await?;
				if finished_only { return Ok(false); }
				continue;
			}
			let orphaned = Self::delete_row_in_transaction(&mut tx, chapter_id, user_id, path.as_deref()).await?;
			tx.commit().await?;
			self.remove_from_queue(chapter_id, user_id).await;
			self.progress.lock().await.remove(&chapter_id);
			self.cleanup_tmp(chapter_id).await;
			if let Some(path) = path && orphaned { let _ = remove_path(&product_path(&self.downloads_dir, &path)).await; }
			if let Some(state) = &state {
				let variants = state.config.page_blobs_dir().join(chapter_id.to_string()).join("variants");
				let invalidation = match tokio::fs::remove_dir_all(&variants).await {
					Ok(()) => None,
					Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
					Err(error) => Some(format!("invalidate page variants: {error}")),
				};
				if let Err(error) = crate::cache::publish_chapter(state, chapter_id).await {
					let message = error.message().to_string();
					tracing::warn!(chapter_id, "download deletion cache revision update failed: {message}");
				} else if let Some(message) = invalidation {
					tracing::warn!(chapter_id, "{message}");
					let _ = sqlx::query("UPDATE chapter_cache_state SET last_error=? WHERE chapter_id=?").bind(message).bind(chapter_id).execute(&self.pool).await;
				}
			}
			return Ok(true);
		}
	}

	/// Nonfinished rows are not a reader source. Delete them in a short write
	/// transaction without waiting on a chapter lease that an active source call holds.
	/// Returns `None` only when the row became finished and needs the exclusive path.
	async fn delete_nonfinished_row(&self, chapter_id: i64, user_id: i64) -> ApiResult<Option<bool>> {
		let mut tx = crate::db::begin_write(&self.pool).await?;
		let row: Option<(String, Option<String>)> = sqlx::query_as(
			"SELECT status, path FROM download WHERE chapter_id = ? AND user_id = ?",
		)
		.bind(chapter_id).bind(user_id).fetch_optional(&mut *tx).await?;
		let Some((status, path)) = row else { tx.rollback().await?; return Ok(Some(false)); };
		if status == "finished" { tx.rollback().await?; return Ok(None); }
		let orphaned = Self::delete_row_in_transaction(&mut tx, chapter_id, user_id, path.as_deref()).await?;
		let active = status == "downloading";
		let mut recorded_path = if active && orphaned && path.is_some() {
			Some(self.cancelled_paths.lock().await)
		} else { None };
		if active {
			// Cancellation is observed after the in-flight source/page operation returns.
			// Do not wait for that operation's chapter read lease here.
			if let (Some(paths), Some(path)) = (&mut recorded_path, path.as_ref()) {
				paths.insert(chapter_id, path.clone());
			}
			self.mark_cancel(chapter_id).await;
		} else {
			self.cleanup_tmp(chapter_id).await;
			if let Some(path) = &path && orphaned {
				let _ = remove_path(&product_path(&self.downloads_dir, path)).await;
			}
		}
		if let Err(error) = tx.commit().await {
			if active { self.cancel.lock().await.remove(&chapter_id); }
			if let Some(paths) = &mut recorded_path { paths.remove(&chapter_id); }
			return Err(error.into());
		}
		drop(recorded_path);
		self.remove_from_queue(chapter_id, user_id).await;
		self.progress.lock().await.remove(&chapter_id);
		Ok(Some(true))
	}

	async fn delete_row_in_transaction(
		tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
		chapter_id: i64,
		user_id: i64,
		path: Option<&str>,
	) -> Result<bool, sqlx::Error> {
		sqlx::query("DELETE FROM download WHERE chapter_id = ? AND user_id = ?")
			.bind(chapter_id).bind(user_id).execute(&mut **tx).await?;
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 0)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 0",
		)
		.bind(chapter_id).bind(user_id).execute(&mut **tx).await?;
		if let Some(path) = path {
			let referenced: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM download WHERE path = ? LIMIT 1")
				.bind(path).fetch_optional(&mut **tx).await?;
			Ok(referenced.is_none())
		} else { Ok(false) }
	}

	pub async fn enqueue_reader_ahead(self: &Arc<Self>, chapter_id: i64, user_id: i64) -> ApiResult<Vec<i64>> {
		let settings = self.settings(user_id).await?;
		if !settings.download_ahead || settings.download_ahead_count == 0 {
			return Ok(Vec::new());
		}
		let current: Option<(i64, i64, String)> = sqlx::query_as(
			"SELECT c.manga_id, c.source_order, s.kind FROM chapter c
			 JOIN manga m ON m.id = c.manga_id JOIN source s ON s.id = m.source_id WHERE c.id = ?",
		)
		.bind(chapter_id)
		.fetch_optional(&self.pool)
		.await?;
		let Some((manga_id, source_order, source_kind)) = current else {
			return Err(crate::error::ApiError::not_found("chapter not found"));
		};
		if source_kind == crate::model::SOURCE_KIND_LOCAL {
			return Ok(Vec::new());
		}
		let candidates: Vec<i64> = sqlx::query_scalar(
			"SELECT id FROM chapter c
			 WHERE c.manga_id = ? AND c.source_order < ?
			 AND NOT EXISTS (SELECT 1 FROM chapter_user cu
			 WHERE cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_read = 1)
			 ORDER BY c.source_order DESC LIMIT ?",
		)
		.bind(manga_id)
		.bind(source_order)
		.bind(user_id)
		.bind(settings.download_ahead_count)
		.fetch_all(&self.pool)
		.await?;
		let mut eligible = Vec::new();
		for candidate in candidates {
			let (downloaded, status): (bool, Option<String>) = sqlx::query_as(
				"SELECT COALESCE(cu.is_downloaded, 0), d.status FROM chapter c
				 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ?
				 LEFT JOIN download d ON d.chapter_id = c.id AND d.user_id = ?
				 WHERE c.id = ?",
			)
			.bind(user_id)
			.bind(user_id)
			.bind(candidate)
			.fetch_one(&self.pool)
			.await?;
			if !downloaded && matches!(status.as_deref(), None | Some("error")) {
				eligible.push(candidate);
			}
		}
		Ok(self.enqueue(&eligible, user_id).await)
	}

	pub async fn run(self: Arc<Self>) {
		// Keep a fallback poll besides notify, guarding against lost wakeups
		let mut poll = tokio::time::interval(std::time::Duration::from_secs(2));
		loop {
			self.dispatch().await;
			tokio::select! {
				_ = self.notify.notified() => {}
				_ = poll.tick() => {}
			}
		}
	}

	async fn dispatch(self: &Arc<Self>) {
		if !self.running.load(Ordering::SeqCst) {
			return;
		}
		let settings = match self.settings(1).await {
			Ok(settings) => settings,
			Err(e) => {
				tracing::warn!("download settings load failed: {e:?}");
				return;
			}
		};
		let concurrency = settings.download_concurrency.max(1);
		let mut deferred = Vec::new();
		loop {
			let next = self.queue.lock().await.pop_front();
			let Some(entry) = next else { break };
			if !self.running.load(Ordering::SeqCst) {
				deferred.push(entry);
				continue;
			}
			let row: Option<(String, i64)> = match sqlx::query_as(
				"SELECT d.status, m.source_id FROM download d
				 JOIN chapter c ON c.id = d.chapter_id JOIN manga m ON m.id = c.manga_id
				 WHERE d.chapter_id = ? AND d.user_id = ?",
			)
			.bind(entry.chapter_id)
			.bind(entry.user_id)
			.fetch_optional(&self.pool)
			.await
			{
				Ok(row) => row,
				Err(e) => {
					tracing::warn!(chapter_id = entry.chapter_id, "download dispatch query failed: {e}");
					deferred.push(entry);
					break;
				}
			};
			// The row may already be DELETEd; anything not queued (e.g. rows paused meanwhile) is not redispatched
			let Some((status, source_id)) = row else { continue };
			if status != "queued" {
				continue;
			}
			{
				let mut active = self.active_sources.lock().await;
				if active.contains(&source_id) || active.len() >= concurrency {
					deferred.push(entry);
					continue;
				}
				active.insert(source_id);
			}
			let manager = self.clone();
			let chapter_id = entry.chapter_id;
			tokio::spawn(async move {
				let worker = manager.clone();
				// Cleanup must run even if the inner task panics, otherwise the source holds its active slot forever
				let inner =
					tokio::spawn(async move { worker.run_chapter(entry.chapter_id, entry.user_id, source_id).await });
				let _ = inner.await;
				manager.cleanup_cancelled_artifact(chapter_id).await;
				manager.cancel.lock().await.remove(&chapter_id);
				// The row now records the path (or the attempt failed), so the in-memory claim is no longer needed
				manager.claimed_paths.lock().await.retain(|_, owner| *owner != chapter_id);
				manager.active_sources.lock().await.remove(&source_id);
				manager.notify.notify_one();
			});
		}
		if !deferred.is_empty() {
			let mut queue = self.queue.lock().await;
			for entry in deferred {
				queue.push_back(entry);
			}
		}
	}

	async fn handle_chapter_error(
		self: &Arc<Self>,
		chapter_id: i64,
		user_id: i64,
		attempt_id: i64,
		message: &str,
	) -> Option<u64> {
		let retries = match sqlx::query_scalar::<_, i64>(
			"SELECT retries FROM download
			 WHERE chapter_id = ? AND user_id = ? AND status = 'downloading' AND updated_at = ?",
		)
		.bind(chapter_id)
		.bind(user_id)
		.bind(attempt_id)
		.fetch_optional(&self.pool)
		.await
		{
			Ok(Some(retries)) => retries,
			Ok(None) => {
				self.cleanup_attempt(chapter_id, None).await;
				return None;
			}
			Err(e) => {
				tracing::warn!(chapter_id, "download retry read failed: {e}");
				self.cleanup_attempt(chapter_id, None).await;
				return None;
			}
		};
		self.apply_chapter_error(chapter_id, user_id, attempt_id, retries, message).await
	}

	async fn apply_chapter_error(
		self: &Arc<Self>,
		chapter_id: i64,
		user_id: i64,
		attempt_id: i64,
		retries: i64,
		message: &str,
	) -> Option<u64> {
		if retries >= MAX_CHAPTER_RETRIES as i64 {
			let (downloaded, total) = self.progress.lock().await.get(&chapter_id).copied().unwrap_or((0, 0));
			let result = sqlx::query(
				"UPDATE download SET status = 'error', error = ?, updated_at = ?
				 WHERE chapter_id = ? AND user_id = ? AND status = 'downloading' AND updated_at = ?",
			)
			.bind(message)
			.bind(unix_now())
			.bind(chapter_id)
			.bind(user_id)
			.bind(attempt_id)
			.execute(&self.pool)
			.await;
			match result {
				Ok(done) if done.rows_affected() == 1 => {
					self.progress.lock().await.remove(&chapter_id);
					let error = aidoku_wasm::log_safe_text(message);
					tracing::warn!(chapter_id, retries, downloaded, total, error = error.as_str(), "download failed");
					self.emit("error", chapter_id, downloaded, total, user_id);
				}
				Ok(_) => {
					self.cleanup_attempt(chapter_id, None).await;
				}
				Err(e) => {
					tracing::warn!(chapter_id, "download error update failed: {e}");
					self.cleanup_attempt(chapter_id, None).await;
				}
			}
			return None;
		}
		let result = sqlx::query(
			"UPDATE download SET retries = retries + 1, error = ?, updated_at = ?
			 WHERE chapter_id = ? AND user_id = ? AND status = 'downloading' AND updated_at = ?",
		)
		.bind(message)
		.bind(unix_now())
		.bind(chapter_id)
		.bind(user_id)
		.bind(attempt_id)
		.execute(&self.pool)
		.await;
		match result {
			Ok(done) if done.rows_affected() == 1 => {
				let delay = 1u64 << (retries + 1);
				let error = aidoku_wasm::log_safe_text(message);
				tracing::warn!(
					chapter_id,
					retry = retries + 1,
					delay_secs = delay,
					error = error.as_str(),
					"download retry scheduled"
				);
				Some(delay)
			}
			Ok(_) => {
				self.cleanup_attempt(chapter_id, None).await;
				None
			}
			Err(e) => {
				tracing::warn!(chapter_id, "download retry update failed: {e}");
				self.cleanup_attempt(chapter_id, None).await;
				None
			}
		}
	}

	async fn run_chapter(self: &Arc<Self>, chapter_id: i64, user_id: i64, source_id: i64) {
		if self.cancel.lock().await.remove(&chapter_id) {
			self.cleanup_tmp(chapter_id).await;
			return;
		}
		// While queued, an earlier download may have finished the same chapter: reuse the artifact instead of repackaging
		if let Some((path, format)) = self.shared_artifact(chapter_id).await {
			self.settle_finished(chapter_id, user_id, &path, &format).await;
			return;
		}
		let marked = sqlx::query(
			"UPDATE download SET status = 'downloading', updated_at = ? WHERE chapter_id = ? AND user_id = ?",
		)
		.bind(unix_now())
		.bind(chapter_id)
		.bind(user_id)
		.execute(&self.pool)
		.await;
		match marked {
			Ok(done) if done.rows_affected() == 0 => return,
			Err(e) => {
				tracing::warn!(chapter_id, "download status update failed: {e}");
				return;
			}
			_ => {}
		}
		let started = std::time::Instant::now();
		tracing::info!(chapter_id, source_id, "download started");
		let mut attempt = 0u32;
		loop {
			if self.cancel.lock().await.remove(&chapter_id) {
				self.cleanup_attempt(chapter_id, None).await;
				return;
			}
			let attempt_id = match sqlx::query_scalar::<_, i64>(
				"SELECT updated_at FROM download WHERE chapter_id = ? AND user_id = ? AND status = 'downloading'",
			)
			.bind(chapter_id)
			.bind(user_id)
			.fetch_optional(&self.pool)
			.await
			{
				Ok(Some(id)) => id,
				Ok(None) => {
					self.cleanup_attempt(chapter_id, None).await;
					return;
				}
				Err(e) => {
					tracing::warn!(chapter_id, "download attempt read failed: {e}");
					self.cleanup_attempt(chapter_id, None).await;
					return;
				}
			};
			attempt += 1;
			match self.try_chapter(chapter_id, user_id, source_id, attempt == 1).await {
				Ok((total, path)) => {
					if self.cancel.lock().await.remove(&chapter_id) {
						self.cleanup_attempt(chapter_id, Some(&path)).await;
						return;
					}
					let format = sqlx::query_scalar::<_, String>(
						"SELECT format FROM download WHERE chapter_id = ? AND user_id = ?",
					)
					.bind(chapter_id)
					.bind(user_id)
					.fetch_one(&self.pool)
					.await
					.unwrap_or_else(|_| "cbz".into());
					let finished = persist_finished(
						&self.pool,
						chapter_id,
						user_id,
						&path,
						&format,
						FinishMode::Update(attempt_id),
					)
					.await;
					match finished {
						Ok(true) => {
							self.progress.lock().await.remove(&chapter_id);
							tracing::info!(
								chapter_id,
								source_id,
								pages = total,
								attempt,
								dur_ms = started.elapsed().as_millis() as u64,
								path = %path,
								"download finished"
							);
							self.cleanup_page_cache_after_finish(chapter_id, total, total, user_id).await;
							self.cleanup_tmp(chapter_id).await;
						}
						Ok(false) => self.cleanup_attempt(chapter_id, Some(&path)).await,
						Err(e) => {
							tracing::warn!(chapter_id, "download finish update failed: {e}");
							self.progress.lock().await.remove(&chapter_id);
							self.cleanup_tmp(chapter_id).await;
						}
					}
					return;
				}
				Err(message) => {
					if self.cancel.lock().await.remove(&chapter_id) {
						self.cleanup_attempt(chapter_id, None).await;
						return;
					}
					let Some(delay) = self.handle_chapter_error(chapter_id, user_id, attempt_id, &message).await
					else {
						return;
					};
					tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
				}
			}
		}
	}

	/// Claims the artifact paths for chapter_id unless another chapter holds one of them, in a finished or
	/// in-flight download; `force` claims regardless (the chapter-id-suffixed fallback cannot belong to anyone else)
	async fn claim_paths(&self, chapter_id: i64, rel_path: &str, stored: Option<&str>, force: bool) -> Result<bool, String> {
		let paths: Vec<&str> = std::iter::once(rel_path).chain(stored).collect();
		let mut claimed = self.claimed_paths.lock().await;
		if !force {
			for path in &paths {
				if claimed.get(*path).is_some_and(|owner| *owner != chapter_id) {
					return Ok(false);
				}
				let taken = sqlx::query_scalar::<_, i64>("SELECT 1 FROM download WHERE path = ? AND chapter_id <> ? LIMIT 1")
					.bind(*path)
					.bind(chapter_id)
					.fetch_optional(&self.pool)
					.await
					.map_err(|e| format!("check artifact path: {e}"))?;
				if taken.is_some() {
					return Ok(false);
				}
			}
		}
		for path in paths {
			claimed.insert(path.to_string(), chapter_id);
		}
		Ok(true)
	}

	/// A single chapter-download attempt: pages accumulate in the tmp dir and get packaged; the caller counts retries on error
	async fn try_chapter(
		self: &Arc<Self>,
		chapter_id: i64,
		user_id: i64,
		source_id: i64,
		first_attempt: bool,
	) -> Result<(i64, String), String> {
		let state = self
			.state
			.upgrade()
			.ok_or_else(|| "server is shutting down".to_string())?;
        let _cache_lease = state.chapter_cache_gate.read().await;
        let cache_lock = state.chapter_cache_lock(chapter_id).await;
        let chapter_cache = cache_lock.read().await;
		let chapter = chapter_row(&self.pool, chapter_id)
			.await
			.map_err(|e| format!("load chapter: {e:?}"))?;
		let manga = manga_row(&self.pool, chapter.manga_id)
			.await
			.map_err(|e| format!("load manga: {e:?}"))?;
		let source: Option<(String, String)> =
			sqlx::query_as("SELECT key, name FROM source WHERE id = ?")
				.bind(source_id)
				.fetch_optional(&self.pool)
				.await
				.map_err(|e| format!("load source: {e:?}"))?;
		let Some((source_key, source_name)) = source else {
			return Err("source not found".into());
		};
		let format: Option<(String,)> =
			sqlx::query_as("SELECT format FROM download WHERE chapter_id = ? AND user_id = ?")
				.bind(chapter_id)
				.bind(user_id)
				.fetch_optional(&self.pool)
				.await
				.map_err(|e| format!("load download format: {e:?}"))?;
		let format = format.map(|(f,)| f).unwrap_or_else(|| "cbz".into());

		let mut pages = crate::source::load_pages_under_lease(&state, chapter_id, false)
			.await
			.map_err(|e| format!("load pages: {e:?}"))?;
		let stale_materialized = crate::source::has_missing_materialized_page(&state, chapter_id, &pages)
			.await.map_err(|e| format!("check materialized pages: {e:?}"))?;
		let _chapter_cache = if stale_materialized {
			drop(chapter_cache);
			let chapter_write = cache_lock.write().await;
			pages = crate::source::load_pages_under_lease(&state, chapter_id, false)
				.await.map_err(|e| format!("reload pages: {e:?}"))?;
			if crate::source::has_missing_materialized_page(&state, chapter_id, &pages)
				.await.map_err(|e| format!("check materialized pages: {e:?}"))? {
				crate::source::load_pages_under_lease(&state, chapter_id, true)
					.await.map_err(|e| format!("refresh pages: {e:?}"))?;
			}
			let chapter_read = tokio::sync::RwLockWriteGuard::downgrade(chapter_write);
			pages = crate::source::load_pages_under_lease(&state, chapter_id, false)
				.await.map_err(|e| format!("reload refreshed pages: {e:?}"))?;
			chapter_read
		} else { chapter_cache };
		let total = pages.len() as i64;
		self.progress.lock().await.insert(chapter_id, (0, total));
		if first_attempt {
			self.emit("started", chapter_id, 0, total, user_id);
		}
		let tmp_dir = self.downloads_dir.join(".tmp").join(chapter_id.to_string());
		// A rerun first clears leftovers from the previous round
		let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
		tokio::fs::create_dir_all(&tmp_dir)
			.await
			.map_err(|e| format!("create tmp dir: {e}"))?;

		type PageTaskResult = Result<(usize, String, Option<PageDimensionUpdate>), String>;
		let mut files: Vec<Option<String>> = vec![None; pages.len()];
		let mut dimension_updates = Vec::new();
		let mut tasks: JoinSet<PageTaskResult> = JoinSet::new();
		let mut next = 0usize;
		let mut done = 0i64;
		while done < total {
			while tasks.len() < PAGE_CONCURRENCY && next < pages.len() {
				let position = next;
				let page = pages[next].clone();
				next += 1;
				let state = state.clone();
				let tmp_dir = tmp_dir.clone();
				tasks.spawn(async move {
					let (name, update) = fetch_page(&state, source_id, chapter_id, &tmp_dir, &page).await?;
					Ok((position, name, update))
				});
			}
			match tasks.join_next().await {
				Some(Ok(Ok((position, name, update)))) => {
					files[position] = Some(name);
					if let Some(update) = update {
						dimension_updates.push(update);
					}
					done += 1;
					self.progress.lock().await.insert(chapter_id, (done, total));
					self.emit("progress", chapter_id, done, total, user_id);
				}
				Some(Ok(Err(e))) => {
					tasks.abort_all();
					return Err(e);
				}
				Some(Err(e)) => {
					tasks.abort_all();
					return Err(format!("page task failed: {e}"));
				}
				None => break,
			}
			if self.cancel.lock().await.contains(&chapter_id) {
				tasks.abort_all();
				return Err("download cancelled".into());
			}
		}

		let comic_info = comic_info_xml(&source_key, &manga, &chapter, total);
		let rel_dir = format!("{}/{}", sanitize(&source_name), sanitize(&manga.title));
		let chapter_name = chapter_display(&chapter);
		let template = move_template(&self.pool, &state.config).await;
		let plan = |base: String| {
			let product_name = if format == "folder" {
				base.clone()
			} else {
				format!("{base}.cbz")
			};
			let rel_path = format!("{rel_dir}/{product_name}");
			let moved = template.as_deref().map(|template| {
				let vars = MoveVars {
					source: &source_name,
					title: &manga.title,
					authors: &manga.authors.0,
					artists: &manga.artists.0,
					chapter: &chapter_name,
					filename: &base,
				};
				let rendered = move_target(template, &vars, &format, &product_name);
				match rendered.is_absolute() {
					true => (rendered.clone(), rendered.to_string_lossy().into_owned()),
					// A relative template joins under downloads_dir; the row still stores the relative path, so the read-side resolution convention is unchanged
					false => (
						self.downloads_dir.join(&rendered),
						rendered.to_string_lossy().into_owned(),
					),
				}
			});
			(rel_path, moved)
		};
		let base = sanitize(&chapter_name);
		let (mut rel_path, mut moved) = plan(base.clone());
		// The name carries only the display number/title, so another chapter (other group, language or volume,
		// or a same-titled manga) can map to the same path; the chapter id keeps this one from overwriting it
		if !self.claim_paths(chapter_id, &rel_path, moved.as_ref().map(|(_, stored)| stored.as_str()), false).await? {
			(rel_path, moved) = plan(format!("{base} [{chapter_id}]"));
			self.claim_paths(chapter_id, &rel_path, moved.as_ref().map(|(_, stored)| stored.as_str()), true).await?;
		}
		package(&self.downloads_dir, &tmp_dir, &rel_path, &format, &files, &comic_info).await?;
		persist_page_dimensions(&self.pool, chapter_id, &dimension_updates)
			.await
			.map_err(|e| format!("store page dimensions: {e:?}"))?;
		let Some((dest, stored)) = moved else {
			return Ok((total, rel_path));
		};
		match move_product(&self.downloads_dir.join(&rel_path), &dest).await {
			Ok(()) => Ok((total, stored)),
			// A failed move (mount dropped/permissions/disk full) must not sink the download: the artifact stays in place and the row keeps the original relative path
			Err(e) => {
				tracing::warn!(chapter_id, "download move to {} failed, product kept in place: {e}", dest.display());
				Ok((total, rel_path))
			}
		}
	}
}

/// Fetches one page into the tmp dir; dimension updates stay in memory until the artifact is packaged.
/// Dimensions are measured from the written file with a 64 KiB head read, falling back to a full read only when necessary.
async fn fetch_page(
	state: &Arc<AppState>,
	source_id: i64,
	chapter_id: i64,
	tmp_dir: &Path,
	page: &PageRow,
) -> Result<(String, Option<PageDimensionUpdate>), String> {
	let idx = page.idx;
	let stored: Value =
		serde_json::from_str(&page.content).map_err(|e| format!("parse page {idx}: {e}"))?;
	let content = stored.get("content").cloned().unwrap_or(Value::Null);
	let is_text = content.get("Text").is_some();
	let (bytes, ext) = page_bytes(state, source_id, chapter_id, idx, &content)
		.await
		.map_err(|e| format!("page {idx}: {e}"))?;
	let name = format!("{idx:03}.{ext}");
	let path = tmp_dir.join(&name);
	tokio::fs::write(&path, &bytes)
		.await
		.map_err(|e| format!("write page {idx}: {e}"))?;
	let update = if is_text {
		None
	} else {
		let head = read_page_file_head(&path, DIMENSION_HEAD_BYTES).await;
		let path = path.clone();
		let dims = measure_with_fallback(head, || async { read_page_file_full(&path).await }).await;
		dims.map(|d| PageDimensionUpdate::new(page, d))
	};
	Ok((name, update))
}

async fn read_page_file_head(path: &Path, head_bytes: usize) -> crate::error::ApiResult<Vec<u8>> {
	let file = tokio::fs::File::open(path)
		.await
		.map_err(|e| crate::error::ApiError::internal(format!("open downloaded page for dimension head read: {e}")))?;
	let mut buf = Vec::with_capacity(head_bytes);
	tokio::io::AsyncReadExt::read_to_end(&mut file.take(head_bytes as u64), &mut buf)
		.await
		.map_err(|e| crate::error::ApiError::internal(format!("read downloaded page head: {e}")))?;
	Ok(buf)
}

async fn read_page_file_full(path: &Path) -> crate::error::ApiResult<Vec<u8>> {
	tokio::fs::read(path)
		.await
		.map_err(|e| crate::error::ApiError::internal(format!("read downloaded page for dimension fallback: {e}")))
}

async fn page_bytes(
	state: &Arc<AppState>,
	source_id: i64,
	chapter_id: i64,
	index: i64,
	content: &Value,
) -> Result<(Vec<u8>, String), String> {
	if let Some(text) = content.get("Text").and_then(Value::as_str) {
		return Ok((text.as_bytes().to_vec(), "txt".into()));
	}
	if let Some(name) = content.get("Image").and_then(Value::as_str) {
		let bytes = crate::source::page_image_blob(state, chapter_id, name)
			.await
			.map_err(|e| format!("{e:?}"))?;
		return Ok((bytes, "png".into()));
	}
	if content.get("Url").is_some() || content.get("Zip").is_some() {
		let (bytes, mime) = crate::cache::page_original(
			state,
			chapter_id,
			index,
			source_id,
			content,
			false,
			async {
				if let Some(url) = content.get("Url").and_then(Value::as_array) {
					let url_str = url.first().and_then(Value::as_str).ok_or_else(|| crate::error::ApiError::bad_request("invalid url page content"))?;
					let context: Option<PageContext> = match url.get(1) {
						Some(value) if !value.is_null() => Some(serde_json::from_value(value.clone())?),
						_ => None,
					};
					let (bytes, mime) = fetch_page_image(state, source_id, url_str, context).await?;
					Ok((bytes, mime.unwrap_or_else(|| "application/octet-stream".into())))
				} else {
					let zip = content.get("Zip").and_then(Value::as_array).ok_or_else(|| crate::error::ApiError::bad_request("invalid zip page content"))?;
					let zip_url = zip.first().and_then(Value::as_str).ok_or_else(|| crate::error::ApiError::bad_request("invalid zip page content"))?;
					let entry_path = zip.get(1).and_then(Value::as_str).ok_or_else(|| crate::error::ApiError::bad_request("invalid zip page content"))?;
					let (zip_bytes, _) = http_get(state, source_id, zip_url).await?;
					let entry = extract_zip_entry(&zip_bytes, entry_path).map_err(crate::error::ApiError::bad_request)?;
					let mime = match ext_for_path(entry_path).as_str() {
						"jpg" | "jpeg" => "image/jpeg", "png" => "image/png", "webp" => "image/webp",
						"gif" => "image/gif", "bmp" => "image/bmp", "avif" => "image/avif",
						_ => "application/octet-stream",
					}.to_string();
					Ok((entry, mime))
				}
			},
		)
		.await
		.map_err(|e| format!("{e:?}"))?;
		return Ok((bytes, ext_for_mime(Some(&mime)).to_string()));
	}
	if let Some(entry) = content.get("OpdsZip").and_then(Value::as_str) {
		let chapter = chapter_row(&state.pool, chapter_id)
			.await
			.map_err(|e| format!("{e:?}"))?;
		let manga = manga_row(&state.pool, chapter.manga_id)
			.await
			.map_err(|e| format!("{e:?}"))?;
		let (bytes, mime) = crate::internal::opds_source::read_cbz_page(state, &manga, &chapter, entry)
			.await
			.map_err(|e| format!("{e:?}"))?;
		return Ok((bytes, ext_for_mime(Some(&mime)).to_string()));
	}
	Err("unknown page content".into())
}

fn extract_zip_entry(zip_bytes: &[u8], entry_path: &str) -> Result<Vec<u8>, String> {
	use std::io::Read;
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
		.map_err(|e| format!("invalid zip archive: {e}"))?;
	let mut entry = archive
		.by_name(entry_path)
		.map_err(|_| "zip entry not found".to_string())?;
	let mut out = Vec::with_capacity(entry.size() as usize);
	entry
		.read_to_end(&mut out)
		.map_err(|e| format!("read zip entry failed: {e}"))?;
	Ok(out)
}

fn ext_for_mime(mime: Option<&str>) -> &'static str {
	let mime = mime
		.unwrap_or_default()
		.split(';')
		.next()
		.unwrap_or_default()
		.trim()
		.to_ascii_lowercase();
	match mime.as_str() {
		"image/jpeg" => "jpg",
		"image/png" => "png",
		"image/webp" => "webp",
		"image/gif" => "gif",
		"image/bmp" => "bmp",
		"image/avif" => "avif",
		_ => "bin",
	}
}

fn ext_for_path(path: &str) -> String {
	let ext = path
		.rsplit('.')
		.next()
		.map(str::to_ascii_lowercase)
		.unwrap_or_default();
	match ext.as_str() {
		"jpg" | "jpeg" => "jpg".into(),
		"png" | "webp" | "gif" | "bmp" | "avif" | "txt" => ext,
		_ => "bin".into(),
	}
}

/// Writes the artifact to disk: cbz is packed in tmp first then renamed into place (atomic replace); folder lays out the file tree directly
async fn package(
	downloads_dir: &Path,
	tmp_dir: &Path,
	rel_path: &str,
	format: &str,
	files: &[Option<String>],
	comic_info: &str,
) -> Result<(), String> {
	let target = downloads_dir.join(rel_path);
	if let Some(parent) = target.parent() {
		tokio::fs::create_dir_all(parent)
			.await
			.map_err(|e| format!("create target dir: {e}"))?;
	}
	if format == "folder" {
		// A redownload rebuilds the whole directory, clearing leftover pages from the previous version
		let _ = tokio::fs::remove_dir_all(&target).await;
		tokio::fs::create_dir_all(&target)
			.await
			.map_err(|e| format!("create target dir: {e}"))?;
		tokio::fs::write(target.join("ComicInfo.xml"), comic_info)
			.await
			.map_err(|e| format!("write ComicInfo: {e}"))?;
		for (position, name) in files.iter().enumerate() {
			let name = name
				.as_ref()
				.ok_or_else(|| format!("missing page file at {position}"))?;
			tokio::fs::copy(tmp_dir.join(name), target.join(name))
				.await
				.map_err(|e| format!("copy page {name}: {e}"))?;
		}
		return Ok(());
	}
	let names: Vec<String> = files
		.iter()
		.enumerate()
		.map(|(position, name)| {
			name.clone()
				.ok_or_else(|| format!("missing page file at {position}"))
		})
		.collect::<Result<_, _>>()?;
	let tmp_dir = tmp_dir.to_path_buf();
	let tmp_cbz = tmp_dir.join("out.cbz");
	let cbz_src = tmp_cbz.clone();
	let comic_info = comic_info.to_string();
	tokio::task::spawn_blocking(move || write_cbz(&tmp_dir, &cbz_src, &names, &comic_info))
		.await
		.map_err(|e| format!("packaging task failed: {e}"))??;
	tokio::fs::rename(&tmp_cbz, &target)
		.await
		.map_err(|e| format!("move cbz into place: {e}"))?;
	Ok(())
}

/// Resolution convention for a download row's path: absolute paths as-is (moved artifacts), relative paths joined under downloads_dir
pub fn product_path(downloads_dir: &Path, path: &str) -> PathBuf {
	let stored = Path::new(path);
	if stored.is_absolute() {
		stored.to_path_buf()
	} else {
		downloads_dir.join(stored)
	}
}

/// A chapter's finished download artifact: its format and resolved on-disk path.
pub struct DownloadProduct {
	pub format: String,
	pub path: PathBuf,
}

/// Resolves a chapter's finished download product. Returns None when there is no finished row,
/// the stored path is missing, or the artifact is no longer on disk (moved away or deleted);
/// callers fall back to the page cache/source path in that case.
async fn finished_product(state: &Arc<AppState>, chapter_id: i64) -> ApiResult<Option<DownloadProduct>> {
	let row: Option<(String, Option<String>)> = sqlx::query_as(
		"SELECT format, path FROM download WHERE chapter_id = ? AND user_id = ? AND status = 'finished'",
	)
	.bind(chapter_id)
	.bind(crate::USER_ID)
	.fetch_optional(&state.pool)
	.await?;
	let Some((format, path)) = row else {
		return Ok(None);
	};
	let Some(path) = path else {
		return Ok(None);
	};
	let path = product_path(&state.config.downloads_dir, &path);
	let meta = match tokio::fs::metadata(&path).await {
		Ok(meta) => meta,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(ApiError::internal(format!("inspect finished download artifact: {error}"))),
	};
	if (format == "folder" && !meta.is_dir()) || (format != "folder" && !meta.is_file()) {
		return Ok(None);
	}
	Ok(Some(DownloadProduct { format, path }))
}

/// Sort key for a product's page files: the downloader writes {idx:03}.{ext}, so the leading
/// number is the page position; files without a numeric prefix sort after the numbered ones.
fn product_entry_key(entry: &str) -> (u64, String) {
	let digits = entry.bytes().take_while(|b| b.is_ascii_digit()).count();
	let number = if digits > 0 {
		entry[..digits].parse().unwrap_or(u64::MAX)
	} else {
		u64::MAX
	};
	(number, entry.to_string())
}

/// Page file names inside a finished product in reading order (position == reader page index),
/// ComicInfo.xml excluded. None when the chapter has no usable finished product.
pub async fn product_entries(state: &Arc<AppState>, chapter_id: i64) -> ApiResult<Option<Vec<String>>> {
	let Some(product) = finished_product(state, chapter_id).await? else {
		return Ok(None);
	};
	Ok(Some(product_entry_names(state, &product).await?))
}

async fn product_entry_names(state: &Arc<AppState>, product: &DownloadProduct) -> ApiResult<Vec<String>> {
	let mut entries = if product.format == "folder" {
		let mut names = Vec::new();
		let mut read_dir = tokio::fs::read_dir(&product.path)
			.await
			.map_err(|e| ApiError::internal(format!("read product dir: {e}")))?;
		while let Some(entry) = read_dir
			.next_entry()
			.await
			.map_err(|e| ApiError::internal(format!("read product dir: {e}")))?
		{
			let name = entry.file_name().to_string_lossy().into_owned();
			if name == "ComicInfo.xml" || entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
				continue;
			}
			names.push(name);
		}
		names
	} else {
		// CBZ handles go through the shared ZipCache: reopening an archive re-reads the whole
		// central directory, which would cost one full read per page request.
		let zip = state.local_zip.clone();
		let path = product.path.clone();
		let names = tokio::task::spawn_blocking(move || zip.entry_names(&path))
			.await
			.map_err(|e| ApiError::internal(format!("read product archive task failed: {e}")))?
			.map_err(|e| ApiError::internal(format!("read product archive: {e}")))?;
		names.into_iter().filter(|name| name != "ComicInfo.xml").collect()
	};
	entries.sort_by_key(|entry| product_entry_key(entry));
	Ok(entries)
}

/// Reads one named entry's bytes from the finished product. The entry must be a single file
/// component; entries always come from `product_entries`, never from client input.
pub async fn read_product_entry(state: &Arc<AppState>, chapter_id: i64, entry: &str) -> ApiResult<Option<Vec<u8>>> {
	if entry.is_empty()
		|| entry == "." || entry == ".."
		|| entry.bytes().any(|b| matches!(b, b'/' | b'\\' | b':'))
	{
		return Err(ApiError::bad_request("invalid product entry"));
	}
	let Some(product) = finished_product(state, chapter_id).await? else {
		return Ok(None);
	};
	read_product_entry_from(state, &product, entry).await
}

async fn read_product_entry_from(
	state: &Arc<AppState>,
	product: &DownloadProduct,
	entry: &str,
) -> ApiResult<Option<Vec<u8>>> {
	let result: std::io::Result<Vec<u8>> = if product.format == "folder" {
		tokio::fs::read(product.path.join(entry)).await
	} else {
		let zip = state.local_zip.clone();
		let path = product.path.clone();
		let entry = entry.to_string();
		tokio::task::spawn_blocking(move || zip.read_entry(&path, &entry))
			.await
			.map_err(|e| ApiError::internal(format!("read product entry task failed: {e}")))?
	};
	match result {
		Ok(bytes) => Ok(Some(bytes)),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
		Err(e) => Err(ApiError::from(sqlx::Error::Io(e))),
	}
}

/// Reads the page at `index` from the finished product: (bytes, entry name).
/// None when there is no usable product, the index is out of range, or the entry vanished.
pub async fn read_product_page(
	state: &Arc<AppState>,
	chapter_id: i64,
	index: usize,
) -> ApiResult<Option<(Vec<u8>, String)>> {
	let Some(product) = finished_product(state, chapter_id).await? else {
		return Ok(None);
	};
	let entries = product_entry_names(state, &product).await?;
	let Some(entry) = entries.get(index).cloned() else {
		return Ok(None);
	};
	match read_product_entry_from(state, &product, &entry).await? {
		Some(bytes) => Ok(Some((bytes, entry))),
		None => Ok(None),
	}
}

/// True when a product page file is a text page (the downloader stores Text pages as {idx:03}.txt).
pub fn is_text_entry(entry: &str) -> bool {
	entry
		.rsplit('.')
		.next()
		.map(|ext| ext.eq_ignore_ascii_case("txt"))
		.unwrap_or(false)
}

/// Synthesizes page rows for a downloaded chapter from its finished product with no source access,
/// so the page list stays readable when the source is unreachable. Text entries are inlined from
/// the product; image entries carry the `{"Downloaded": <entry>}` marker that `api/page.rs`
/// resolves against the product. Dimensions from existing page rows are reused when present.
/// Nothing is written to the page table: the rows are derived live, so a deleted product falls
/// back to the page cache/source path automatically.
pub async fn pages_from_product(state: &Arc<AppState>, chapter_id: i64) -> ApiResult<Option<Vec<PageRow>>> {
	let Some(entries) = product_entries(state, chapter_id).await? else {
		return Ok(None);
	};
	let dims: Vec<(i64, Option<i64>, Option<i64>)> =
		sqlx::query_as("SELECT idx, width, height FROM page WHERE chapter_id = ?")
			.bind(chapter_id)
			.fetch_all(&state.pool)
			.await?;
	let dims: HashMap<i64, (Option<i64>, Option<i64>)> = dims
		.into_iter()
		.map(|(idx, width, height)| (idx, (width, height)))
		.collect();
	let mut rows = Vec::with_capacity(entries.len());
	for (idx, entry) in entries.iter().enumerate() {
		let content = if is_text_entry(entry) {
			match read_product_entry(state, chapter_id, entry).await? {
				Some(bytes) => serde_json::json!({ "Text": String::from_utf8_lossy(&bytes).into_owned() }),
				None => serde_json::json!({ "Downloaded": entry }),
			}
		} else {
			serde_json::json!({ "Downloaded": entry })
		};
		let (width, height) = dims.get(&(idx as i64)).copied().unwrap_or((None, None));
		// Keep the same wire shape as cached page rows: content plus the required page-list
		// fields. Product pages have no thumbnail or description, matching the local-source rows.
		rows.push(PageRow {
			idx: idx as i64,
			content: serde_json::to_string(&serde_json::json!({
				"content": content,
				"thumbnail": null,
				"hasDescription": false,
				"description": null,
			}))
			.map_err(|e| ApiError::internal(format!("serialize product page: {e}")))?,
			width,
			height,
			dimensions_checked: false,
		});
	}
	Ok(Some(rows))
}

/// Move-template layering: env (config.download_move_path) > the download.move_path setting > empty (no move)
async fn move_template(pool: &SqlitePool, config: &crate::config::Config) -> Option<String> {
	let raw = match &config.download_move_path {
		Some(v) => Some(v.clone()),
		None => {
			let row: Option<(String,)> =
				match sqlx::query_as("SELECT value FROM setting WHERE key = 'download.move_path'")
					.fetch_optional(pool)
					.await
				{
					Ok(row) => row,
					Err(e) => {
						tracing::warn!("download move_path setting read failed: {e}");
						None
					}
				};
			row.and_then(|(raw,)| serde_json::from_str::<String>(&raw).ok())
		}
	};
	raw.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Template variables: every value goes through sanitize to strip path separators (guarding against metadata-injected traversal); empty/unknown variables become Unknown
struct MoveVars<'a> {
	source: &'a str,
	title: &'a str,
	authors: &'a [String],
	artists: &'a [String],
	chapter: &'a str,
	/// Default base name (no extension) of the packaged artifact, doubling as the switch for "the template renders a full file name"
	filename: &'a str,
}

fn render_move_template(template: &str, vars: &MoveVars) -> String {
	let mut out = String::with_capacity(template.len());
	let mut rest = template;
	while let Some(start) = rest.find('{') {
		out.push_str(&rest[..start]);
		let after = &rest[start + 1..];
		match after.find('}') {
			Some(end) => {
				out.push_str(&move_var(&after[..end], vars));
				rest = &after[end + 1..];
			}
			None => {
				out.push('{');
				rest = after;
			}
		}
	}
	out.push_str(rest);
	out
}

fn move_var(name: &str, vars: &MoveVars) -> String {
	let raw = match name {
		"source" => vars.source.to_string(),
		"title" | "manga" => vars.title.to_string(),
		"author" => vars.authors.join(", "),
		"artist" => vars.artists.join(", "),
		"chapter" => vars.chapter.to_string(),
		"filename" => vars.filename.to_string(),
		_ => String::new(),
	};
	if raw.trim().is_empty() { "Unknown".into() } else { sanitize(&raw) }
}

const ARCHIVE_EXTS: &[&str] = &["cbz", "zip", "cbr", "cb7", "cbt", "rar", "7z", "tar"];

fn has_archive_ext(path: &str) -> bool {
	Path::new(path)
		.extension()
		.and_then(|e| e.to_str())
		.map(|e| ARCHIVE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
		.unwrap_or(false)
}

/// Renders the move target: when the template contains {filename} the result is a full file name (cbz format gets
/// .cbz appended when no known archive extension); otherwise the result is a directory and the default artifact name is appended under it
fn move_target(template: &str, vars: &MoveVars, format: &str, product_name: &str) -> PathBuf {
	let rendered = render_move_template(template, vars);
	if !template.contains("{filename}") {
		return Path::new(&rendered).join(product_name);
	}
	if format != "folder" && !has_archive_ext(&rendered) {
		return PathBuf::from(format!("{rendered}.cbz"));
	}
	PathBuf::from(rendered)
}

/// Moves the artifact: the target is a shared content area (same-person redownloads / multiple users on the same
/// chapter render the same target, and identical content may be overwritten), and rename falls back to copy+remove
/// across devices (the norm for mounted volumes)
async fn move_product(src: &Path, dest: &Path) -> Result<(), String> {
	if let Some(parent) = dest.parent() {
		tokio::fs::create_dir_all(parent)
			.await
			.map_err(|e| format!("create move target dir: {e}"))?;
	}
	remove_path(dest).await?;
	match tokio::fs::rename(src, dest).await {
		Ok(()) => return Ok(()),
		Err(e) if e.kind() != std::io::ErrorKind::CrossesDevices => {
			return Err(format!("rename product: {e}"));
		}
		_ => {}
	}
	if src.is_dir() {
		copy_dir_all(src, dest).await?;
		tokio::fs::remove_dir_all(src)
			.await
			.map_err(|e| format!("remove moved dir: {e}"))?;
	} else {
		tokio::fs::copy(src, dest)
			.await
			.map_err(|e| format!("copy product: {e}"))?;
		tokio::fs::remove_file(src)
			.await
			.map_err(|e| format!("remove moved file: {e}"))?;
	}
	Ok(())
}

/// Clears the target spot; already missing (mount dropped/manually cleaned) counts as deleted
async fn remove_path(path: &Path) -> Result<(), String> {
	let result = if path.is_dir() {
		tokio::fs::remove_dir_all(path).await
	} else {
		tokio::fs::remove_file(path).await
	};
	match result {
		Ok(()) => Ok(()),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(e) => Err(format!("clear move target: {e}")),
	}
}

fn copy_dir_all<'a>(
	src: &'a Path,
	dst: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
	Box::pin(async move {
		tokio::fs::create_dir_all(dst)
			.await
			.map_err(|e| format!("create dir {}: {e}", dst.display()))?;
		let mut entries = tokio::fs::read_dir(src)
			.await
			.map_err(|e| format!("read dir {}: {e}", src.display()))?;
		while let Some(entry) = entries
			.next_entry()
			.await
			.map_err(|e| format!("read dir {}: {e}", src.display()))?
		{
			let from = entry.path();
			let to = dst.join(entry.file_name());
			let kind = entry
				.file_type()
				.await
				.map_err(|e| format!("stat {}: {e}", from.display()))?;
			if kind.is_dir() {
				copy_dir_all(&from, &to).await?;
			} else {
				tokio::fs::copy(&from, &to)
					.await
					.map_err(|e| format!("copy {}: {e}", from.display()))?;
			}
		}
		Ok(())
	})
}

fn write_cbz(
	tmp_dir: &Path,
	target: &Path,
	files: &[String],
	comic_info: &str,
) -> Result<(), String> {
	use std::io::Write;
	// A fixed timestamp keeps redownloads of the same chapter byte-reproducible
	let options = zip::write::SimpleFileOptions::default()
		.compression_method(zip::CompressionMethod::Deflated)
		.last_modified_time(
			zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).expect("dos epoch is valid"),
		);
	let file = std::fs::File::create(target).map_err(|e| format!("create cbz: {e}"))?;
	let mut archive = zip::ZipWriter::new(file);
	archive
		.start_file("ComicInfo.xml", options)
		.map_err(|e| format!("cbz entry: {e}"))?;
	archive
		.write_all(comic_info.as_bytes())
		.map_err(|e| format!("cbz write: {e}"))?;
	for name in files {
		let data = std::fs::read(tmp_dir.join(name)).map_err(|e| format!("read page {name}: {e}"))?;
		archive
			.start_file(name, options)
			.map_err(|e| format!("cbz entry: {e}"))?;
		archive
			.write_all(&data)
			.map_err(|e| format!("cbz write: {e}"))?;
	}
	archive.finish().map_err(|e| format!("finish cbz: {e}"))?;
	Ok(())
}

fn comic_info_xml(source_key: &str, manga: &MangaRow, chapter: &ChapterRow, page_count: i64) -> String {
	let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ComicInfo>");
	let title = xml_escape(&manga.title);
	xml.push_str(&format!("<Manga>{title}</Manga><Series>{title}</Series>"));
	if let Some(number) = chapter.number {
		xml.push_str(&format!("<Number>{number}</Number>"));
	}
	if let Some(title) = &chapter.title {
		xml.push_str(&format!("<Title>{}</Title>", xml_escape(title)));
	}
	if !manga.authors.0.is_empty() {
		xml.push_str(&format!("<Writer>{}</Writer>", xml_escape(&manga.authors.0.join(", "))));
	}
	if !manga.artists.0.is_empty() {
		xml.push_str(&format!(
			"<Penciller>{}</Penciller>",
			xml_escape(&manga.artists.0.join(", "))
		));
	}
	if !manga.tags.0.is_empty() {
		xml.push_str(&format!("<Genre>{}</Genre>", xml_escape(&manga.tags.0.join(", "))));
	}
	if let Some(url) = &manga.url {
		xml.push_str(&format!("<Web>{}</Web>", xml_escape(url)));
	}
	if let Some(description) = &manga.description {
		xml.push_str(&format!("<Summary>{}</Summary>", xml_escape(description)));
	}
	// Notes embeds JSON so a read-back can restore the binding (borrowed from Aidoku's AIDOKUDATA)
	let notes = serde_json::json!({
		"sourceKey": source_key,
		"mangaKey": manga.key,
		"chapterKey": chapter.key,
	});
	xml.push_str(&format!("<Notes>!SUMIDOKUDATA{}</Notes>", xml_escape(&notes.to_string())));
	xml.push_str(&format!("<PageCount>{page_count}</PageCount></ComicInfo>"));
	xml
}

fn xml_escape(text: &str) -> String {
	text.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
}

fn chapter_display(chapter: &ChapterRow) -> String {
	if let Some(number) = chapter.number {
		format!("第 {number} 话")
	} else if let Some(title) = &chapter.title {
		title.clone()
	} else {
		chapter.key.clone()
	}
}

/// File-name sanitizing: strips path-illegal characters, collapses whitespace, trims leading/trailing spaces and dots
pub fn sanitize(name: &str) -> String {
	let mut out = String::with_capacity(name.len());
	let mut last_whitespace = false;
	for c in name.chars() {
		if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
			continue;
		}
		if c.is_whitespace() {
			if !last_whitespace {
				out.push(' ');
			}
			last_whitespace = true;
		} else {
			out.push(c);
			last_whitespace = false;
		}
	}
	let trimmed = out.trim_matches(|c| c == ' ' || c == '.');
	let truncated: String = trimmed.chars().take(80).collect();
	if truncated.is_empty() {
		"_".into()
	} else {
		truncated
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Clone, Default)]
	struct TestLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

	impl TestLogs {
		fn text(&self) -> String {
			String::from_utf8(self.0.lock().expect("log buffer").clone()).expect("utf8 logs")
		}
	}

	impl std::io::Write for TestLogs {
		fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
			self.0.lock().expect("log buffer").extend_from_slice(buf);
			Ok(buf.len())
		}

		fn flush(&mut self) -> std::io::Result<()> {
			Ok(())
		}
	}

	impl tracing_subscriber::fmt::MakeWriter<'_> for TestLogs {
		type Writer = Self;

		fn make_writer(&self) -> Self::Writer {
			self.clone()
		}
	}

	fn vars() -> MoveVars<'static> {
		MoveVars {
			source: "Example Source",
			title: "Example Manga",
			authors: &[],
			artists: &[],
			chapter: "第 1 话",
			filename: "第 1 话",
		}
	}

	#[test]
	fn template_substitutes_all_variables() {
		let authors = vec!["Author A".to_string(), "Author B".to_string()];
		let artists = vec!["Artist C".to_string()];
		let vars = MoveVars {
			authors: &authors,
			artists: &artists,
			..vars()
		};
		let out = render_move_template("/lib/{source}/{manga}/{author}/{artist}/{chapter}/{filename}", &vars);
		assert_eq!(out, "/lib/Example Source/Example Manga/Author A, Author B/Artist C/第 1 话/第 1 话");
	}

	#[test]
	fn template_title_aliases_manga() {
		assert_eq!(
			render_move_template("{title}", &vars()),
			render_move_template("{manga}", &vars())
		);
	}

	#[test]
	fn template_empty_and_unknown_variables_become_unknown() {
		// Empty authors/artists fall back to Unknown; unknown names also land on Unknown
		let out = render_move_template("{author}/{artist}/{scanlator}", &vars());
		assert_eq!(out, "Unknown/Unknown/Unknown");
	}

	#[test]
	fn template_values_are_sanitized_against_traversal() {
		let vars = MoveVars {
			title: "../../etc/passwd",
			chapter: "a/b\\c:d",
			..vars()
		};
		let out = render_move_template("/lib/{manga}/{chapter}", &vars);
		assert_eq!(out, "/lib/etcpasswd/abcd");
	}

	#[test]
	fn template_unclosed_brace_stays_literal() {
		assert_eq!(render_move_template("/lib/{manga", &vars()), "/lib/{manga");
	}

	#[test]
	fn target_without_filename_var_is_directory() {
		let target = move_target("/lib/{author}/{manga}", &vars(), "cbz", "第 1 话.cbz");
		assert_eq!(target, PathBuf::from("/lib/Unknown/Example Manga/第 1 话.cbz"));
		let target = move_target("/lib/{manga}", &vars(), "folder", "第 1 话");
		assert_eq!(target, PathBuf::from("/lib/Example Manga/第 1 话"));
	}

	#[test]
	fn target_with_filename_var_is_full_name() {
		let target = move_target("/lib/{manga}/{filename}", &vars(), "cbz", "第 1 话.cbz");
		assert_eq!(target, PathBuf::from("/lib/Example Manga/第 1 话.cbz"));
	}

	#[test]
	fn target_with_filename_var_appends_cbz_only_without_archive_ext() {
		let target = move_target("/lib/{manga}/{filename}.zip", &vars(), "cbz", "第 1 话.cbz");
		assert_eq!(target, PathBuf::from("/lib/Example Manga/第 1 话.zip"));
		// A folder-format artifact is a directory and never gets an extension appended
		let target = move_target("/lib/{manga}/{filename}", &vars(), "folder", "第 1 话");
		assert_eq!(target, PathBuf::from("/lib/Example Manga/第 1 话"));
	}

	#[test]
	fn target_relative_and_absolute_render_as_is() {
		let target = move_target("moved/{filename}", &vars(), "cbz", "第 1 话.cbz");
		assert_eq!(target, PathBuf::from("moved/第 1 话.cbz"));
		assert!(!target.is_absolute());
		let target = move_target("/mnt/lib/{filename}", &vars(), "cbz", "第 1 话.cbz");
		assert!(target.is_absolute());
	}

	#[test]
	fn product_path_absolute_as_is_relative_joined() {
		let downloads_dir = Path::new("/downloads");
		assert_eq!(
			product_path(downloads_dir, "/mnt/lib/a.cbz"),
			PathBuf::from("/mnt/lib/a.cbz")
		);
		assert_eq!(
			product_path(downloads_dir, "Source/Manga/a.cbz"),
			PathBuf::from("/downloads/Source/Manga/a.cbz")
		);
	}

	async fn finish_db() -> (SqlitePool, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, wasm_path) VALUES ('finish-source', 'Source', '') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("source row");
		let manga_id: i64 =
			sqlx::query_scalar("INSERT INTO manga (source_id, key) VALUES (?, 'manga') RETURNING id")
				.bind(source_id)
				.fetch_one(&pool)
				.await
				.expect("manga row");
		let chapter_id: i64 =
			sqlx::query_scalar("INSERT INTO chapter (manga_id, key) VALUES (?, 'chapter') RETURNING id")
				.bind(manga_id)
				.fetch_one(&pool)
				.await
				.expect("chapter row");
		(pool, chapter_id, tmp)
	}

	async fn insert_downloading(pool: &SqlitePool, chapter_id: i64) {
		sqlx::query("INSERT INTO download (chapter_id, user_id, status) VALUES (?, 1, 'downloading')")
			.bind(chapter_id)
			.execute(pool)
			.await
			.expect("downloading row");
	}

	async fn reject_finish_flag(pool: &SqlitePool) {
		sqlx::query(
			"CREATE TRIGGER reject_finish_flag BEFORE INSERT ON chapter_user
			 BEGIN SELECT RAISE(ABORT, 'flag rejected'); END",
		)
		.execute(pool)
		.await
		.expect("flag rejection trigger");
	}

	async fn finish_state(pool: &SqlitePool, chapter_id: i64) -> (Option<String>, Option<i64>) {
		let status = sqlx::query_scalar::<_, String>("SELECT status FROM download WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.fetch_optional(pool)
			.await
			.expect("download status");
		let downloaded = sqlx::query_scalar::<_, i64>(
			"SELECT is_downloaded FROM chapter_user WHERE chapter_id = ? AND user_id = 1",
		)
		.bind(chapter_id)
		.fetch_optional(pool)
		.await
		.expect("download flag");
		(status, downloaded)
	}

	#[tokio::test]
	async fn persist_finished_update_rolls_back_when_flag_fails() {
		let (pool, chapter_id, _tmp) = finish_db().await;
		insert_downloading(&pool, chapter_id).await;
		reject_finish_flag(&pool).await;

		persist_finished(&pool, chapter_id, 1, "chapter.cbz", "cbz", FinishMode::Update(0))
			.await
			.expect_err("flag write must fail");
		assert_eq!(finish_state(&pool, chapter_id).await, (Some("downloading".into()), None));
	}

	#[tokio::test]
	async fn persist_finished_upsert_rolls_back_when_flag_fails() {
		let (pool, chapter_id, _tmp) = finish_db().await;
		reject_finish_flag(&pool).await;

		persist_finished(&pool, chapter_id, 1, "chapter.cbz", "cbz", FinishMode::Upsert)
			.await
			.expect_err("flag write must fail");
		assert_eq!(finish_state(&pool, chapter_id).await, (None, None));
	}

	#[tokio::test]
	async fn persist_finished_update_commits_row_and_flag() {
		let (pool, chapter_id, _tmp) = finish_db().await;
		insert_downloading(&pool, chapter_id).await;

		persist_finished(&pool, chapter_id, 1, "chapter.cbz", "cbz", FinishMode::Update(0))
			.await
			.expect("finish update");
		assert_eq!(finish_state(&pool, chapter_id).await, (Some("finished".into()), Some(1)));
	}

	#[tokio::test]
	async fn persist_finished_update_ignores_a_stale_attempt() {
		let (pool, chapter_id, _tmp) = finish_db().await;
		insert_downloading(&pool, chapter_id).await;
		sqlx::query("UPDATE download SET updated_at = 7 WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.execute(&pool)
			.await
			.expect("move attempt");

		let applied = persist_finished(&pool, chapter_id, 1, "chapter.cbz", "cbz", FinishMode::Update(0))
			.await
			.expect("stale finish");
		assert!(!applied);
		assert_eq!(finish_state(&pool, chapter_id).await, (Some("downloading".into()), None));
	}

	#[tokio::test]
	async fn persist_finished_upsert_commits_row_and_flag() {
		let (pool, chapter_id, _tmp) = finish_db().await;

		persist_finished(&pool, chapter_id, 1, "chapter.cbz", "cbz", FinishMode::Upsert)
			.await
			.expect("finish upsert");
		assert_eq!(finish_state(&pool, chapter_id).await, (Some("finished".into()), Some(1)));
	}

	#[tokio::test]
	async fn move_product_renames_file_and_replaces_target() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let src = tmp.path().join("in/a.cbz");
		let dest = tmp.path().join("out/a.cbz");
		std::fs::create_dir_all(src.parent().expect("src parent")).expect("mkdir src");
		std::fs::write(&src, b"new").expect("write src");
		std::fs::create_dir_all(dest.parent().expect("dest parent")).expect("mkdir dest");
		std::fs::write(&dest, b"old").expect("write dest");

		move_product(&src, &dest).await.expect("move");
		assert!(!src.exists());
		assert_eq!(std::fs::read(&dest).expect("read dest"), b"new");
	}

	#[tokio::test]
	async fn move_product_replaces_directory_target() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let src = tmp.path().join("in/chapter");
		let dest = tmp.path().join("out/chapter");
		std::fs::create_dir_all(&src).expect("mkdir src");
		std::fs::write(src.join("000.png"), b"new").expect("write page");
		std::fs::create_dir_all(&dest).expect("mkdir dest");
		std::fs::write(dest.join("stale.png"), b"old").expect("write stale page");

		move_product(&src, &dest).await.expect("move");
		assert!(!src.exists());
		assert!(!dest.join("stale.png").exists());
		assert_eq!(std::fs::read(dest.join("000.png")).expect("read page"), b"new");
	}

	#[tokio::test]
	async fn move_product_failure_keeps_source() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let src = tmp.path().join("in/a.cbz");
		std::fs::create_dir_all(src.parent().expect("src parent")).expect("mkdir src");
		std::fs::write(&src, b"data").expect("write src");
		// The target's parent path is blocked by a same-named file: mkdir must fail and the artifact must stay in place
		let blocker = tmp.path().join("blocker");
		std::fs::write(&blocker, b"x").expect("write blocker");

		let result = move_product(&src, &blocker.join("a.cbz")).await;
		assert!(result.is_err());
		assert!(src.exists(), "source product must stay in place");
	}

	async fn manager_db(config: crate::config::Config) -> (Arc<AppState>, i64, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let mut config = config;
		config.config_dir = tmp.path().to_path_buf();
		config.database_url = format!("sqlite://{}/test.db?mode=rwc", tmp.path().display());
		config.covers_dir = tmp.path().join("covers");
		config.downloads_dir = tmp.path().join("downloads");
		let pool = crate::db::create_pool(&config.database_url).await.expect("pool");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, wasm_path) VALUES ('manager-source', 'Source', '') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("source row");
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key) VALUES (?, 'manga') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("manga row");
		let chapter_id: i64 = sqlx::query_scalar("INSERT INTO chapter (manga_id, key) VALUES (?, 'chapter') RETURNING id")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("chapter row");
		let (defaults_tx, _defaults_rx) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = tokio::sync::mpsc::unbounded_channel();
		let state = AppState::new(pool, config, defaults_tx, cookie_tx);
		assert!(state.downloads.set(DownloadManager::new(&state)).is_ok());
		(state, manga_id, chapter_id, tmp)
	}

	#[tokio::test]
	async fn cancelled_artifact_cleanup_preserves_another_chapters_claim() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let path = "Source/Manga/shared.cbz";
		let artifact = state.config.downloads_dir.join(path);
		std::fs::create_dir_all(artifact.parent().expect("artifact parent")).expect("artifact dir");
		std::fs::write(&artifact, b"claimed artifact").expect("artifact bytes");
		let manager = state.downloads();
		manager.cancelled_paths.lock().await.insert(chapter_id, path.into());
		manager.claimed_paths.lock().await.insert(path.into(), chapter_id + 1);

		manager.cleanup_cancelled_artifact(chapter_id).await;

		assert!(artifact.exists(), "cleanup must preserve a path claimed by another chapter");
	}

	/// Writes a CBZ-style archive (ComicInfo.xml first, then the given pages) with the same layout
	/// the downloader produces, so the product readers see real artifacts.
	fn write_test_cbz(path: &Path, pages: &[(&str, &[u8])]) {
		use std::io::Write;
		let file = std::fs::File::create(path).expect("create cbz");
		let mut archive = zip::ZipWriter::new(file);
		let options = zip::write::SimpleFileOptions::default();
		archive.start_file("ComicInfo.xml", options).expect("start comic info");
		archive.write_all(b"<ComicInfo/>").expect("write comic info");
		for (name, bytes) in pages {
			archive.start_file(*name, options).expect("start page");
			archive.write_all(bytes).expect("write page");
		}
		archive.finish().expect("finish cbz");
	}

	async fn insert_finished_product(state: &AppState, chapter_id: i64, format: &str, path: &str) {
		sqlx::query(
			"INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', ?, ?)",
		)
		.bind(chapter_id)
		.bind(format)
		.bind(path)
		.execute(&state.pool)
		.await
		.expect("finished download row");
	}

	#[tokio::test]
	async fn pages_from_product_lists_cbz_entries_and_inlines_text() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(
			&dir.join("000.cbz"),
			&[("000.png", b"\x89PNG-0"), ("001.png", b"\x89PNG-1"), ("002.txt", b"hello text")],
		);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/000.cbz").await;
		let rows = pages_from_product(&state, chapter_id).await.expect("product pages").expect("some");
		assert_eq!(rows.len(), 3);
		assert_eq!(rows[0].idx, 0);
		assert!(rows[0].content.contains("\"Downloaded\":\"000.png\""));
		assert!(rows[2].content.contains("\"Text\":\"hello text\""));
	}

	#[tokio::test]
	async fn pages_from_product_reads_folder_format() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga/ch");
		std::fs::create_dir_all(&dir).expect("product dir");
		std::fs::write(dir.join("ComicInfo.xml"), b"<ComicInfo/>").expect("comic info");
		std::fs::write(dir.join("000.png"), b"\x89PNG-F").expect("page");
		std::fs::write(dir.join("001.txt"), b"chapter one").expect("text page");
		insert_finished_product(&state, chapter_id, "folder", "Source/Manga/ch").await;
		let rows = pages_from_product(&state, chapter_id).await.expect("product pages").expect("some");
		assert_eq!(rows.len(), 2);
		assert!(rows[0].content.contains("\"Downloaded\":\"000.png\""));
		assert!(rows[1].content.contains("\"Text\":\"chapter one\""));
	}

	#[tokio::test]
	async fn pages_from_product_none_without_usable_product() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		// No download row at all
		assert!(pages_from_product(&state, chapter_id).await.expect("no row").is_none());
		// A finished row whose artifact is missing on disk is not a usable product either
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/gone.cbz").await;
		assert!(pages_from_product(&state, chapter_id).await.expect("missing file").is_none());
	}

	#[tokio::test]
	async fn read_product_page_orders_entries_by_numeric_prefix() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		// Entries deliberately out of archive order: position 0 must still resolve to 000.png
		write_test_cbz(
			&dir.join("000.cbz"),
			&[("002.png", b"two"), ("000.png", b"zero"), ("001.png", b"one")],
		);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/000.cbz").await;
		let (bytes, entry) = read_product_page(&state, chapter_id, 0).await.expect("page 0").expect("some");
		assert_eq!(entry, "000.png");
		assert_eq!(bytes, b"zero");
		assert!(read_product_page(&state, chapter_id, 3).await.expect("oob").is_none());
	}

	#[tokio::test]
	async fn finished_event_follows_page_cache_cleanup_and_revision_update() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(&dir.join("done.cbz"), &[("000.png", b"product-page")]);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/done.cbz").await;
		let blob_dir = state.config.page_blobs_dir().join(chapter_id.to_string());
		std::fs::create_dir_all(blob_dir.join("variants/256")).expect("blob dirs");
		std::fs::write(blob_dir.join("0.png"), b"materialized").expect("materialized page");
		std::fs::write(blob_dir.join("variants/256/0.png"), b"variant").expect("variant");
		sqlx::query("INSERT INTO page(chapter_id,idx,content,width,height,dimensions_checked) VALUES (?,0,'{\"content\":{\"Image\":\"0.png\"}}',640,480,1)")
			.bind(chapter_id).execute(&state.pool).await.expect("page metadata");
		let old_revision = crate::cache::chapter_revision(&state.pool, chapter_id).await.expect("old revision");
		let manager = state.downloads();
		let mut events = manager.subscribe();
		let chapter_lock = state.chapter_cache_lock(chapter_id).await;
		let chapter_reader = chapter_lock.read().await;
		let cleanup_manager = manager.clone();
		let cleanup = tokio::spawn(async move { cleanup_manager.cleanup_page_cache_after_finish(chapter_id, 1, 1, 1).await });
		while chapter_lock.try_read().is_ok() {
			assert!(!cleanup.is_finished(), "completion cleanup must queue behind the chapter reader");
			tokio::task::yield_now().await;
		}
		assert!(!cleanup.is_finished(), "completion cleanup waits for the chapter reader");
		drop(chapter_reader);
		assert!(cleanup.await.expect("cleanup task"));
		assert!(!blob_dir.exists(), "all source-derived page assets are removed");
		let row: (i64, Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
			"SELECT count(*), max(width), max(height), max(dimensions_checked) FROM page WHERE chapter_id=?",
		)
		.bind(chapter_id).fetch_one(&state.pool).await.expect("page row");
		assert_eq!(row, (1, Some(640), Some(480), Some(1)), "cleanup preserves page metadata");
		assert!(crate::cache::chapter_revision(&state.pool, chapter_id).await.expect("new revision") > old_revision);
		let event = events.try_recv().expect("finished event after cleanup");
		assert_eq!(event.kind, "finished");
		assert_eq!(event.chapter_id, chapter_id);
	}

	#[tokio::test]
	async fn failed_page_cache_cleanup_keeps_product_and_reports_actual_usage_before_event() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		let artifact = dir.join("done.cbz");
		write_test_cbz(&artifact, &[("000.png", b"finished-page")]);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/done.cbz").await;
		let page_json = r#"{"content":{"Image":"0.png"},"thumbnail":null,"hasDescription":false,"description":null}"#;
		sqlx::query("INSERT INTO page(chapter_id,idx,content) VALUES (?,0,?)")
			.bind(chapter_id).bind(page_json).execute(&state.pool).await.expect("page row");
		let blob_dir = state.config.page_blobs_dir().join(chapter_id.to_string());
		std::fs::create_dir_all(blob_dir.join("originals/0")).expect("original dir");
		std::fs::write(blob_dir.join("originals/0/bytes"), b"source-page-bytes").expect("original bytes");
		crate::cache::reconcile_chapter(&state.pool, &state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), chapter_id).await.expect("initial accounting");
		let manager = state.downloads();
		let mut events = manager.subscribe();
		assert!(manager.cleanup_page_cache_after_finish_with_failure(chapter_id, 1, 1, 1, Some(blob_dir.clone())).await);
		assert!(artifact.exists(), "cache cleanup failure never removes the committed product");
		assert!(blob_dir.join("originals/0/bytes").exists(), "injected cleanup failure leaves the cache files");
		let status: String = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id=? AND user_id=1")
			.bind(chapter_id).fetch_one(&state.pool).await.expect("download status");
		assert_eq!(status, "finished");
		let (accounted, last_error): (i64, Option<String>) = sqlx::query_as("SELECT bytes,last_error FROM chapter_cache_state WHERE chapter_id=?")
			.bind(chapter_id).fetch_one(&state.pool).await.expect("cache state");
		let page_bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM page WHERE chapter_id=?")
			.bind(chapter_id).fetch_one(&state.pool).await.expect("page bytes");
		let disk_bytes = crate::cache::chapter_disk_bytes_blocking(&state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), chapter_id).expect("disk usage");
		assert_eq!(accounted, page_bytes + disk_bytes, "failed cleanup reports actual remaining usage");
		assert!(last_error.is_some_and(|error| error.contains("injected cache cleanup failure")));
		let event = events.try_recv().expect("finished event follows attempted cleanup and accounting");
		assert_eq!(event.kind, "finished");
	}

	#[tokio::test]
	async fn stale_cleanup_after_delete_preserves_cache_and_emits_no_finished_event() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(&dir.join("gone.cbz"), &[("000.png", b"page")]);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/gone.cbz").await;
		let blob_dir = state.config.page_blobs_dir().join(chapter_id.to_string());
		std::fs::create_dir_all(blob_dir.join("originals/0")).expect("original cache dir");
		std::fs::write(blob_dir.join("originals/0/bytes"), b"source page").expect("cached bytes");
		let manager = state.downloads();
		let mut events = manager.subscribe();
		assert!(manager.delete_guarded(chapter_id, 1, false).await.expect("delete product"));
		assert!(!manager.cleanup_page_cache_after_finish(chapter_id, 1, 1, 1).await, "late completion recheck sees the deleted row");
		assert!(blob_dir.join("originals/0/bytes").exists(), "stale cleanup cannot remove page cache after deletion");
		assert!(matches!(events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
	}

	#[tokio::test]
	async fn finished_only_delete_does_not_cancel_an_active_download() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		sqlx::query("INSERT INTO download(chapter_id,user_id,status) VALUES (?,1,'downloading')")
			.bind(chapter_id).execute(&state.pool).await.expect("downloading row");
		let manager = state.downloads();
		assert!(!manager.delete_guarded(chapter_id, 1, true).await.expect("finished-only delete"));
		assert!(!manager.cancel.lock().await.contains(&chapter_id), "finished-only action leaves active cancellation state alone");
		let status: String = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id=? AND user_id=1")
			.bind(chapter_id).fetch_one(&state.pool).await.expect("download status");
		assert_eq!(status, "downloading");
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

	async fn add_ordered_chapter(state: &AppState, manga_id: i64, key: &str, source_order: i64) -> i64 {
		sqlx::query_scalar("INSERT INTO chapter (manga_id, key, source_order) VALUES (?, ?, ?) RETURNING id")
			.bind(manga_id)
			.bind(key)
			.bind(source_order)
			.fetch_one(&state.pool)
			.await
			.expect("chapter row")
	}

	#[tokio::test]
	async fn settings_reload_and_enqueue_uses_current_format() {
		let config = crate::config::Config {
			auto_download_new_chapters: true,
			auto_download_limit: 7,
			download_format: "folder".into(),
			download_concurrency: 4,
			..crate::config::Config::default()
		};
		let (state, _manga_id, chapter_id, _tmp) = manager_db(config).await;
		let loaded = DownloadSettings::load(&state.pool, 1, &state.config).await.expect("defaults");
		assert!(loaded.auto_download_new_chapters);
		assert_eq!(loaded.auto_download_limit, 7);
		assert_eq!(loaded.download_format, "folder");
		assert_eq!(loaded.download_concurrency, 4);
		assert!(loaded.auto_skip_manga_with_unread);
		assert_eq!(loaded.download_ahead_count, 2);

		set_setting(&state, AUTO_DOWNLOAD_NEW_CHAPTERS, serde_json::json!(false)).await;
		set_setting(&state, AUTO_DOWNLOAD_LIMIT, serde_json::json!(0)).await;
		set_setting(&state, DOWNLOAD_FORMAT, serde_json::json!("cbz")).await;
		set_setting(&state, DOWNLOAD_CONCURRENCY, serde_json::json!(1)).await;
		set_setting(&state, DELETE_AFTER_MANUAL_READ, serde_json::json!(true)).await;
		set_setting(&state, DELETE_AFTER_READ, serde_json::json!(true)).await;
		set_setting(&state, ALLOW_DELETE_BOOKMARKED, serde_json::json!(true)).await;
		set_setting(&state, AUTO_SKIP_MANGA_WITH_UNREAD, serde_json::json!(false)).await;
		set_setting(&state, AUTO_IGNORE_REUPLOADS, serde_json::json!(true)).await;
		set_setting(&state, DOWNLOAD_AHEAD, serde_json::json!(true)).await;
		set_setting(&state, DOWNLOAD_AHEAD_COUNT, serde_json::json!(5)).await;
		set_setting(&state, "user.1.library.excluded_download_categories", serde_json::json!([3, 4])).await;
		let loaded = DownloadSettings::load(&state.pool, 1, &state.config).await.expect("runtime values");
		assert!(!loaded.auto_download_new_chapters);
		assert_eq!(loaded.auto_download_limit, 0);
		assert_eq!(loaded.download_format, "cbz");
		assert_eq!(loaded.download_concurrency, 1);
		assert!(loaded.delete_after_manual_read && loaded.delete_after_read && loaded.allow_delete_bookmarked);
		assert!(!loaded.auto_skip_manga_with_unread && loaded.auto_ignore_reuploads);
		assert!(loaded.download_ahead);
		assert_eq!(loaded.download_ahead_count, 5);
		assert_eq!(loaded.excluded_download_categories, vec![3, 4]);

		set_setting(&state, DOWNLOAD_FORMAT, serde_json::json!("folder")).await;
		let manager = state.downloads();
		assert_eq!(manager.enqueue(&[chapter_id], 1).await, vec![chapter_id]);
		let format: String = sqlx::query_scalar("SELECT format FROM download WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.fetch_one(&state.pool)
			.await
			.expect("format");
		assert_eq!(format, "folder");
		manager.delete(chapter_id, 1).await.expect("delete");
		set_setting(&state, DOWNLOAD_FORMAT, serde_json::json!("cbz")).await;
		assert_eq!(manager.enqueue(&[chapter_id], 1).await, vec![chapter_id]);
		let format: String = sqlx::query_scalar("SELECT format FROM download WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.fetch_one(&state.pool)
			.await
			.expect("format");
		assert_eq!(format, "cbz");
	}

	#[tokio::test]
	async fn delete_keeps_shared_artifact_until_last_reference() {
		let (state, manga_id, first, _tmp) = manager_db(crate::config::Config::default()).await;
		let second = add_ordered_chapter(&state, manga_id, "second", 1).await;
		std::fs::create_dir_all(&state.config.downloads_dir).expect("downloads dir");
		let artifact = state.config.downloads_dir.join("shared.cbz");
		std::fs::write(&artifact, b"shared").expect("artifact");
		for chapter_id in [first, second] {
			sqlx::query("INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', 'cbz', 'shared.cbz')")
				.bind(chapter_id)
				.execute(&state.pool)
				.await
				.expect("download row");
			sqlx::query("INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, 1, 1)")
				.bind(chapter_id)
				.execute(&state.pool)
				.await
				.expect("download flag");
		}
		let manager = state.downloads();
		assert!(manager.delete(first, 1).await.expect("first delete"));
		assert!(artifact.exists());
		assert_eq!(finish_state(&state.pool, first).await, (None, Some(0)));
		assert!(manager.delete(second, 1).await.expect("second delete"));
		assert!(!artifact.exists());
		assert!(!manager.delete(second, 1).await.expect("missing delete"));
	}

	#[tokio::test]
	async fn stale_attempt_cleanup_keeps_referenced_artifact() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		std::fs::create_dir_all(&state.config.downloads_dir).expect("downloads dir");
		let artifact = state.config.downloads_dir.join("shared.cbz");
		std::fs::write(&artifact, b"shared").expect("artifact");
		sqlx::query("INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', 'cbz', 'shared.cbz')")
			.bind(chapter_id)
			.execute(&state.pool)
			.await
			.expect("download row");

		let manager = state.downloads();
		manager.cleanup_attempt(chapter_id, Some("shared.cbz")).await;
		assert!(artifact.exists());
	}

	#[tokio::test]
	async fn error_handling_retries_and_finishes_current_attempt() {
		let logs = TestLogs::default();
		let subscriber = tracing_subscriber::fmt()
			.with_writer(logs.clone())
			.with_ansi(false)
			.with_max_level(tracing::Level::INFO)
			.finish();
		let _guard = tracing::subscriber::set_default(subscriber);
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		sqlx::query(
			"INSERT INTO download (chapter_id, user_id, status, retries, updated_at)
			 VALUES (?, 1, 'downloading', 0, 7)",
		)
		.bind(chapter_id)
		.execute(&state.pool)
		.await
		.expect("downloading row");
		let manager = state.downloads();
		let mut events = manager.subscribe();
		let retry_message = "fetch https://user:retry-pass@manga.example.com/secret/path?token=retry-secret failed";

		let delay = manager.handle_chapter_error(chapter_id, 1, 7, retry_message).await;
		assert_eq!(delay, Some(2));
		let row: (String, i64, Option<String>) =
			sqlx::query_as("SELECT status, retries, error FROM download WHERE chapter_id = ? AND user_id = 1")
				.bind(chapter_id)
				.fetch_one(&state.pool)
				.await
				.expect("retry row");
		assert_eq!(row, ("downloading".into(), 1, Some(retry_message.into())));
		assert!(matches!(events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));

		sqlx::query("UPDATE download SET retries = ?, updated_at = 8 WHERE chapter_id = ? AND user_id = 1")
			.bind(MAX_CHAPTER_RETRIES as i64)
			.bind(chapter_id)
			.execute(&state.pool)
			.await
			.expect("last retry");
		manager.progress.lock().await.insert(chapter_id, (1, 2));
		let final_message = "fetch https://user:final-pass@manga.example.com/other/path?token=final-secret failed";
		let delay = manager.handle_chapter_error(chapter_id, 1, 8, final_message).await;
		assert_eq!(delay, None);
		let row: (String, i64, Option<String>) =
			sqlx::query_as("SELECT status, retries, error FROM download WHERE chapter_id = ? AND user_id = 1")
				.bind(chapter_id)
				.fetch_one(&state.pool)
				.await
				.expect("error row");
		assert_eq!(row, ("error".into(), MAX_CHAPTER_RETRIES as i64, Some(final_message.into())));
		let event = events.recv().await.expect("error event");
		assert_eq!(event.kind, "error");
		assert_eq!((event.chapter_id, event.downloaded, event.total), (chapter_id, 1, 2));

		let text = logs.text();
		let retry_line = text
			.lines()
			.find(|line| line.contains("download retry scheduled"))
			.unwrap_or_else(|| panic!("retry log missing: {text}"));
		for field in ["WARN", "retry=1", "delay_secs=2", "manga.example.com"] {
			assert!(retry_line.contains(field), "missing {field:?} in {retry_line}");
		}
		let final_line = text
			.lines()
			.find(|line| line.contains("download failed"))
			.unwrap_or_else(|| panic!("final failure log missing: {text}"));
		for field in ["WARN", "retries=3", "downloaded=1", "total=2", "manga.example.com"] {
			assert!(final_line.contains(field), "missing {field:?} in {final_line}");
		}
		for secret in ["retry-pass", "retry-secret", "final-pass", "final-secret", "/secret/path", "/other/path"] {
			assert!(!text.contains(secret), "sensitive URL part {secret:?} leaked: {text}");
		}
	}

	#[tokio::test]
	async fn stale_error_attempt_preserves_requeued_row_and_emits_nothing() {
		for old_retries in [0, MAX_CHAPTER_RETRIES as i64] {
			let (state, manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
			let source_id: i64 = sqlx::query_scalar("SELECT source_id FROM manga WHERE id = ?")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await
				.expect("source id");
			sqlx::query(
				"INSERT INTO download (chapter_id, user_id, status, retries, updated_at)
				 VALUES (?, 1, 'downloading', ?, 41)",
			)
			.bind(chapter_id)
			.bind(old_retries)
			.execute(&state.pool)
			.await
			.expect("downloading row");
			let manager = state.downloads();
			let mut events = manager.subscribe();
			let message = manager
				.try_chapter(chapter_id, 1, source_id, true)
				.await
				.expect_err("controlled source failure");
			assert!(manager.delete(chapter_id, 1).await.expect("delete"));
			assert_eq!(manager.enqueue(&[chapter_id], 1).await, vec![chapter_id]);
			let before: (String, i64, Option<String>, i64) = sqlx::query_as(
				"SELECT status, retries, error, updated_at FROM download WHERE chapter_id = ? AND user_id = 1",
			)
			.bind(chapter_id)
			.fetch_one(&state.pool)
			.await
			.expect("requeued row");
			assert_eq!((&before.0, before.1, before.2.as_deref()), (&"queued".into(), 0, None));
			while events.try_recv().is_ok() {}

			assert_eq!(manager.handle_chapter_error(chapter_id, 1, 41, &message).await, None);
			// The write phase models the row changing after a successful retries read.
			assert_eq!(manager.apply_chapter_error(chapter_id, 1, 41, old_retries, &message).await, None);
			assert!(matches!(events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
			let after: (String, i64, Option<String>, i64) = sqlx::query_as(
				"SELECT status, retries, error, updated_at FROM download WHERE chapter_id = ? AND user_id = 1",
			)
			.bind(chapter_id)
			.fetch_one(&state.pool)
			.await
			.expect("preserved row");
			assert_eq!(after, before);
		}
	}

	#[tokio::test]
	async fn reader_ahead_orders_dedupes_and_does_not_recurse() {
		let (state, manga_id, current, _tmp) = manager_db(crate::config::Config::default()).await;
		sqlx::query("UPDATE chapter SET source_order = 10 WHERE id = ?")
			.bind(current)
			.execute(&state.pool)
			.await
			.expect("current order");
		let next = add_ordered_chapter(&state, manga_id, "next", 9).await;
		let retry = add_ordered_chapter(&state, manga_id, "retry", 8).await;
		let later = add_ordered_chapter(&state, manga_id, "later", 7).await;
		sqlx::query("INSERT INTO download (chapter_id, user_id, status, format) VALUES (?, 1, 'error', 'cbz')")
			.bind(retry)
			.execute(&state.pool)
			.await
			.expect("error row");
		set_setting(&state, DOWNLOAD_AHEAD, serde_json::json!(true)).await;
		set_setting(&state, DOWNLOAD_AHEAD_COUNT, serde_json::json!(2)).await;
		let manager = state.downloads();
		let queued = manager.enqueue_reader_ahead(current, 1).await.expect("ahead");
		assert_eq!(queued, vec![next, retry]);
		let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE status = 'queued'")
			.fetch_one(&state.pool)
			.await
			.expect("queue count");
		assert_eq!(count, 2);
		assert!(manager.enqueue_reader_ahead(current, 1).await.expect("deduped").is_empty());
		let later_status: Option<String> = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id = ?")
			.bind(later)
			.fetch_optional(&state.pool)
			.await
			.expect("later status");
		assert_eq!(later_status, None);
	}

	#[tokio::test]
	async fn reader_ahead_excludes_states_after_selecting_unread_window() {
		let (state, manga_id, current, _tmp) = manager_db(crate::config::Config::default()).await;
		sqlx::query("UPDATE chapter SET source_order = 10 WHERE id = ?")
			.bind(current)
			.execute(&state.pool)
			.await
			.expect("current order");
		let queued = add_ordered_chapter(&state, manga_id, "queued", 9).await;
		let error = add_ordered_chapter(&state, manga_id, "error", 8).await;
		let outside = add_ordered_chapter(&state, manga_id, "outside", 7).await;
		sqlx::query("INSERT INTO download (chapter_id, user_id, status, format) VALUES (?, 1, 'queued', 'cbz')")
			.bind(queued)
			.execute(&state.pool)
			.await
			.expect("queued row");
		set_setting(&state, DOWNLOAD_AHEAD, serde_json::json!(true)).await;
		set_setting(&state, DOWNLOAD_AHEAD_COUNT, serde_json::json!(2)).await;
		let manager = state.downloads();
		assert_eq!(manager.enqueue_reader_ahead(current, 1).await.expect("ahead"), vec![error]);
		let outside_status: Option<String> = sqlx::query_scalar("SELECT status FROM download WHERE chapter_id = ?")
			.bind(outside)
			.fetch_optional(&state.pool)
			.await
			.expect("outside status");
		assert_eq!(outside_status, None);
	}

	#[tokio::test]
	async fn reader_ahead_skips_local_source() {
		let (state, manga_id, current, _tmp) = manager_db(crate::config::Config::default()).await;
		sqlx::query("UPDATE source SET kind = 'local' WHERE id = (SELECT source_id FROM manga WHERE id = ?)")
			.bind(manga_id)
			.execute(&state.pool)
			.await
			.expect("local source");
		sqlx::query("UPDATE chapter SET source_order = 10 WHERE id = ?")
			.bind(current)
			.execute(&state.pool)
			.await
			.expect("current order");
		add_ordered_chapter(&state, manga_id, "next", 9).await;
		add_ordered_chapter(&state, manga_id, "later", 8).await;
		set_setting(&state, DOWNLOAD_AHEAD, serde_json::json!(true)).await;
		set_setting(&state, DOWNLOAD_AHEAD_COUNT, serde_json::json!(2)).await;

		let manager = state.downloads();
		assert!(manager.enqueue_reader_ahead(current, 1).await.expect("ahead").is_empty());
		let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download")
			.fetch_one(&state.pool)
			.await
			.expect("download count");
		assert_eq!(count, 0);
	}

	#[tokio::test]
	async fn dispatch_reloads_concurrency_without_cancelling_active_sources() {
		let (state, _manga_id, chapter_id, _tmp) = manager_db(crate::config::Config::default()).await;
		let manager = state.downloads();
		manager.active_sources.lock().await.extend([91, 92]);
		manager.enqueue(&[chapter_id], 1).await;
		set_setting(&state, DOWNLOAD_CONCURRENCY, serde_json::json!(1)).await;
		manager.dispatch().await;
		assert_eq!(manager.active_sources.lock().await.len(), 2);
		assert_eq!(manager.queue.lock().await.len(), 1);
	}
}
