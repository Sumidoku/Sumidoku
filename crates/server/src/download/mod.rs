use crate::model::{ChapterRow, MangaRow, unix_now};
use crate::source::{chapter_row, fetch_page_image, http_get, load_pages, manga_row};
use crate::state::AppState;
use aidoku::PageContext;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, Notify, Semaphore, broadcast};
use tokio::task::JoinSet;

/// 章级重跑次数上限（不含首次尝试），退避 2/4/8s
const MAX_CHAPTER_RETRIES: u32 = 3;
/// 章内页图并行上限
const PAGE_CONCURRENCY: usize = 5;

#[derive(Debug, Clone, Serialize)]
pub struct DownloadEvent {
	#[serde(rename = "type")]
	pub kind: &'static str,
	#[serde(rename = "chapterId")]
	pub chapter_id: i64,
	pub downloaded: i64,
	pub total: i64,
	/// 仅 running 事件（kind="running"、chapter_id=0）携带；chapter 事件恒 None 不上 wire
	#[serde(skip_serializing_if = "Option::is_none")]
	pub running: Option<bool>,
	/// 行归属用户，WS 按连接用户过滤；running 事件是全局管理器状态（None，全员广播）
	#[serde(skip)]
	pub user_id: Option<i64>,
}

/// 内存队列条目：无请求上下文，行归属随条目走
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueEntry {
	chapter_id: i64,
	user_id: i64,
}

/// 下载管理器：源内串行、源间并行，全局并发源数受 semaphore 限制。
/// 状态机落 download 表（queued → downloading → (paused) → finished / error），重启可恢复。
pub struct DownloadManager {
	// Weak 防循环引用：AppState 经 OnceCell 持有本管理器
	state: Weak<AppState>,
	pool: SqlitePool,
	queue: Mutex<VecDeque<QueueEntry>>,
	notify: Notify,
	events: broadcast::Sender<DownloadEvent>,
	running: AtomicBool,
	active_sources: Mutex<HashSet<i64>>,
	semaphore: Arc<Semaphore>,
	progress: Mutex<HashMap<i64, (i64, i64)>>,
	cancel: Mutex<HashSet<i64>>,
	downloads_dir: PathBuf,
	format: String,
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
			semaphore: Arc::new(Semaphore::new(state.config.download_concurrency.max(1))),
			progress: Mutex::new(HashMap::new()),
			cancel: Mutex::new(HashSet::new()),
			downloads_dir: state.config.downloads_dir.clone(),
			format: state.config.download_format.clone(),
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
		self.events.subscribe()
	}

	pub fn downloads_dir(&self) -> &Path {
		&self.downloads_dir
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

	/// 共享产物：另一用户已下好同一章且产物在盘，返回 (path, format) 供直接落 finished
	async fn shared_artifact(&self, chapter_id: i64, user_id: i64) -> Option<(String, String)> {
		let row: Option<(String, String)> = sqlx::query_as(
			"SELECT path, format FROM download
			 WHERE chapter_id = ? AND user_id != ? AND status = 'finished' AND path IS NOT NULL
			 ORDER BY updated_at DESC LIMIT 1",
		)
		.bind(chapter_id)
		.bind(user_id)
		.fetch_optional(&self.pool)
		.await
		.ok()?;
		let (path, format) = row?;
		let exists = tokio::fs::try_exists(product_path(&self.downloads_dir, &path)).await.ok()?;
		exists.then_some((path, format))
	}

	/// 复用共享产物落行：不重复打包，行直接 finished 并打下载标记
	async fn settle_finished(&self, chapter_id: i64, user_id: i64, path: &str, format: &str) {
		let now = unix_now();
		let result = sqlx::query(
			"INSERT INTO download (chapter_id, user_id, status, format, path, created_at, updated_at)
			 VALUES (?, ?, 'finished', ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 status = 'finished', format = excluded.format, path = excluded.path,
			 error = NULL, retries = 0, updated_at = excluded.updated_at",
		)
		.bind(chapter_id)
		.bind(user_id)
		.bind(format)
		.bind(path)
		.bind(now)
		.bind(now)
		.execute(&self.pool)
		.await;
		if let Err(e) = result {
			tracing::warn!(chapter_id, "download shared-artifact settle failed: {e}");
			return;
		}
		self.flag_downloaded(chapter_id, user_id).await;
		self.emit("finished", chapter_id, 0, 0, user_id);
	}

	async fn flag_downloaded(&self, chapter_id: i64, user_id: i64) {
		let flag = sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 1)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 1",
		)
		.bind(chapter_id)
		.bind(user_id)
		.execute(&self.pool)
		.await;
		if let Err(e) = flag {
			tracing::warn!(chapter_id, "download flag update failed: {e}");
		}
	}

	/// 入队去重后的章节列表；行归属 user_id，已 finished/在队/在跑的跳过，error 行重置重排
	pub async fn enqueue(self: &Arc<Self>, chapter_ids: &[i64], user_id: i64) -> Vec<i64> {
		let format = match self.format.as_str() {
			"folder" => "folder",
			_ => "cbz",
		};
		let now = unix_now();
		let mut seen = HashSet::new();
		let mut queued = Vec::new();
		for &chapter_id in chapter_ids {
			if !seen.insert(chapter_id) {
				continue;
			}
			if let Some((path, shared_format)) = self.shared_artifact(chapter_id, user_id).await {
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
						"UPDATE download SET status = 'queued', error = NULL, retries = 0,
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
					// 章节不存在时 FK 拒绝，直接跳过
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
			self.notify.notify_one();
		}
		queued
	}

	/// 启动恢复：上次进程退出的 downloading 行一律按未完成处理，回 queued 重排
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

	/// stop 只停调度：在跑章跑完，未开工的 queued 落 paused；start 时 paused 重灌
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

	pub async fn run(self: Arc<Self>) {
		// notify 之外留兜底轮询，防丢唤醒
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
			// 行可能已被 DELETE 掉；非 queued（例如暂停期间的行）不重派
			let Some((status, source_id)) = row else { continue };
			if status != "queued" {
				continue;
			}
			if self.active_sources.lock().await.contains(&source_id) {
				deferred.push(entry);
				continue;
			}
			let Ok(permit) = self.semaphore.clone().try_acquire_owned() else {
				deferred.push(entry);
				continue;
			};
			self.active_sources.lock().await.insert(source_id);
			let manager = self.clone();
			tokio::spawn(async move {
				let worker = manager.clone();
				// 里层 panic 也要放行清理，否则该源永远占住 active 位
				let inner =
					tokio::spawn(async move { worker.run_chapter(entry.chapter_id, entry.user_id, source_id).await });
				let _ = inner.await;
				drop(permit);
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

	async fn run_chapter(self: &Arc<Self>, chapter_id: i64, user_id: i64, source_id: i64) {
		if self.cancel.lock().await.remove(&chapter_id) {
			self.cleanup_tmp(chapter_id).await;
			return;
		}
		// 排队期间另一用户可能已下好同一章：产物共享，直接落 finished 不重复打包
		if let Some((path, format)) = self.shared_artifact(chapter_id, user_id).await {
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
		let mut attempt = 0u32;
		loop {
			if self.cancel.lock().await.remove(&chapter_id) {
				self.progress.lock().await.remove(&chapter_id);
				self.cleanup_tmp(chapter_id).await;
				return;
			}
			attempt += 1;
			match self.try_chapter(chapter_id, user_id, source_id, attempt == 1).await {
				Ok((total, path)) => {
					if self.cancel.lock().await.remove(&chapter_id) {
						self.progress.lock().await.remove(&chapter_id);
						self.cleanup_tmp(chapter_id).await;
						return;
					}
					let finished = sqlx::query(
						"UPDATE download SET status = 'finished', path = ?, error = NULL, updated_at = ?
						 WHERE chapter_id = ? AND user_id = ?",
					)
					.bind(&path)
					.bind(unix_now())
					.bind(chapter_id)
					.bind(user_id)
					.execute(&self.pool)
					.await;
					if let Err(e) = finished {
						tracing::warn!(chapter_id, "download finish update failed: {e}");
					}
					self.flag_downloaded(chapter_id, user_id).await;
					self.progress.lock().await.remove(&chapter_id);
					self.emit("finished", chapter_id, total, total, user_id);
					self.cleanup_tmp(chapter_id).await;
					return;
				}
				Err(message) => {
					if self.cancel.lock().await.remove(&chapter_id) {
						self.progress.lock().await.remove(&chapter_id);
						self.cleanup_tmp(chapter_id).await;
						return;
					}
					let row: Option<(i64,)> = sqlx::query_as(
						"SELECT retries FROM download WHERE chapter_id = ? AND user_id = ?",
					)
					.bind(chapter_id)
					.bind(user_id)
					.fetch_optional(&self.pool)
					.await
					.unwrap_or(None);
					// 行没了说明下载已被删除，静默退出
					let Some((retries,)) = row else {
						self.progress.lock().await.remove(&chapter_id);
						return;
					};
					if retries >= MAX_CHAPTER_RETRIES as i64 {
						let (downloaded, total) =
							self.progress.lock().await.remove(&chapter_id).unwrap_or((0, 0));
						let result = sqlx::query(
							"UPDATE download SET status = 'error', error = ?, updated_at = ?
							 WHERE chapter_id = ? AND user_id = ?",
						)
						.bind(&message)
						.bind(unix_now())
						.bind(chapter_id)
						.bind(user_id)
						.execute(&self.pool)
						.await;
						if let Err(e) = result {
							tracing::warn!(chapter_id, "download error update failed: {e}");
						}
						self.emit("error", chapter_id, downloaded, total, user_id);
						return;
					}
					let result = sqlx::query(
						"UPDATE download SET retries = retries + 1, error = ?, updated_at = ?
						 WHERE chapter_id = ? AND user_id = ?",
					)
					.bind(&message)
					.bind(unix_now())
					.bind(chapter_id)
					.bind(user_id)
					.execute(&self.pool)
					.await;
					if let Err(e) = result {
						tracing::warn!(chapter_id, "download retry update failed: {e}");
					}
					tokio::time::sleep(std::time::Duration::from_secs(1 << (retries + 1))).await;
				}
			}
		}
	}

	/// 单次章下载尝试：页攒进 tmp 目录后打包，错误由调用方计重试
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

		let pages = load_pages(&state, chapter_id, false)
			.await
			.map_err(|e| format!("load pages: {e:?}"))?;
		let total = pages.len() as i64;
		self.progress.lock().await.insert(chapter_id, (0, total));
		if first_attempt {
			self.emit("started", chapter_id, 0, total, user_id);
		}
		let tmp_dir = self.downloads_dir.join(".tmp").join(chapter_id.to_string());
		// 重跑先清上一轮残留
		let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
		tokio::fs::create_dir_all(&tmp_dir)
			.await
			.map_err(|e| format!("create tmp dir: {e}"))?;

		let mut files: Vec<Option<String>> = vec![None; pages.len()];
		let mut tasks: JoinSet<Result<(usize, String), String>> = JoinSet::new();
		let mut next = 0usize;
		let mut done = 0i64;
		while done < total {
			while tasks.len() < PAGE_CONCURRENCY && next < pages.len() {
				let position = next;
				let (idx, content) = pages[next].clone();
				next += 1;
				let state = state.clone();
				let tmp_dir = tmp_dir.clone();
				tasks.spawn(async move {
					let name = fetch_page(&state, source_id, chapter_id, &tmp_dir, idx, &content).await?;
					Ok((position, name))
				});
			}
			match tasks.join_next().await {
				Some(Ok(Ok((position, name)))) => {
					files[position] = Some(name);
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
		let base = sanitize(&chapter_name);
		let product_name = if format == "folder" {
			base.clone()
		} else {
			format!("{base}.cbz")
		};
		let rel_path = format!("{rel_dir}/{product_name}");
		package(&self.downloads_dir, &tmp_dir, &rel_path, &format, &files, &comic_info).await?;
		let Some(template) = move_template(&self.pool, &state.config).await else {
			return Ok((total, rel_path));
		};
		let vars = MoveVars {
			source: &source_name,
			title: &manga.title,
			authors: &manga.authors.0,
			artists: &manga.artists.0,
			chapter: &chapter_name,
			filename: &base,
		};
		let rendered = move_target(&template, &vars, &format, &product_name);
		let (dest, stored) = match rendered.is_absolute() {
			true => (rendered.clone(), rendered.to_string_lossy().into_owned()),
			// 相对模板并到 downloads_dir 下，行里仍存相对路径，读取侧解析约定不变
			false => (
				self.downloads_dir.join(&rendered),
				rendered.to_string_lossy().into_owned(),
			),
		};
		match move_product(&self.downloads_dir.join(&rel_path), &dest).await {
			Ok(()) => Ok((total, stored)),
			// 移动失败（挂载掉线/权限/盘满）不拖垮下载：产物留原位，行存原相对路径
			Err(e) => {
				tracing::warn!(chapter_id, "download move to {} failed, product kept in place: {e}", dest.display());
				Ok((total, rel_path))
			}
		}
	}
}

/// 拉一页内容写进 tmp 目录，返回产物文件名
async fn fetch_page(
	state: &Arc<AppState>,
	source_id: i64,
	chapter_id: i64,
	tmp_dir: &Path,
	idx: i64,
	stored: &str,
) -> Result<String, String> {
	let stored: Value =
		serde_json::from_str(stored).map_err(|e| format!("parse page {idx}: {e}"))?;
	let content = stored.get("content").cloned().unwrap_or(Value::Null);
	let (bytes, ext) = page_bytes(state, source_id, chapter_id, &content)
		.await
		.map_err(|e| format!("page {idx}: {e}"))?;
	let name = format!("{idx:03}.{ext}");
	tokio::fs::write(tmp_dir.join(&name), &bytes)
		.await
		.map_err(|e| format!("write page {idx}: {e}"))?;
	Ok(name)
}

async fn page_bytes(
	state: &Arc<AppState>,
	source_id: i64,
	chapter_id: i64,
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
	if let Some(url) = content.get("Url").and_then(Value::as_array) {
		let url_str = url
			.first()
			.and_then(Value::as_str)
			.ok_or("invalid url page content")?;
		let context: Option<PageContext> = match url.get(1) {
			Some(value) if !value.is_null() => serde_json::from_value(value.clone())
				.map_err(|e| format!("invalid page context: {e}"))?,
			_ => None,
		};
		let (bytes, mime) = fetch_page_image(state, source_id, url_str, context)
			.await
			.map_err(|e| format!("{e:?}"))?;
		return Ok((bytes, ext_for_mime(mime.as_deref()).to_string()));
	}
	if let Some(zip) = content.get("Zip").and_then(Value::as_array) {
		let zip_url = zip
			.first()
			.and_then(Value::as_str)
			.ok_or("invalid zip page content")?;
		let entry_path = zip
			.get(1)
			.and_then(Value::as_str)
			.ok_or("invalid zip page content")?;
		let (zip_bytes, _) = http_get(state, source_id, zip_url)
			.await
			.map_err(|e| format!("{e:?}"))?;
		let entry = extract_zip_entry(&zip_bytes, entry_path)?;
		return Ok((entry, ext_for_path(entry_path)));
	}
	if let Some(entry) = content.get("OpdsZip").and_then(Value::as_str) {
		let chapter = chapter_row(&state.pool, chapter_id)
			.await
			.map_err(|e| format!("{e:?}"))?;
		let manga = manga_row(&state.pool, chapter.manga_id)
			.await
			.map_err(|e| format!("{e:?}"))?;
		let (bytes, mime) = crate::opds_source::read_cbz_page(state, &manga, &chapter, entry)
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

/// 产物落盘：cbz 先在 tmp 打好再 rename 到目标（原子替换），folder 直接铺文件树
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
		// 重下时整目录重建，清掉上一版残留页
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

/// 下载行 path 的解析约定：绝对路径原样（已移动产物），相对路径并到 downloads_dir
pub fn product_path(downloads_dir: &Path, path: &str) -> PathBuf {
	let stored = Path::new(path);
	if stored.is_absolute() {
		stored.to_path_buf()
	} else {
		downloads_dir.join(stored)
	}
}

/// 移动模板分层：env（config.download_move_path）> setting 表 download.move_path > 空（不动）
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

/// 模板变量：每个值先过 sanitize 剥路径分隔符（防元数据注入目录穿越），空/未知变量落 Unknown
struct MoveVars<'a> {
	source: &'a str,
	title: &'a str,
	authors: &'a [String],
	artists: &'a [String],
	chapter: &'a str,
	/// 打包产物的默认基名（不含扩展名），同时充当「模板渲染结果是完整文件名」的开关
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

/// 渲染移动目标：模板含 {filename} 时结果是完整文件名（cbz 格式无已知压缩扩展名补 .cbz），
/// 否则结果是目录、默认产物名追加其下
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

/// 产物搬家：目标是共享内容区（同人重下/多用户同章渲染同一目标，内容相同覆盖即可），
/// rename 跨设备（挂载盘常态）回退 copy+remove
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

/// 清目标位；已缺失（挂载掉线/手动清过）视为已删
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
	// 固定时间戳保证同一章重下字节可复现
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
	// Notes 内嵌 JSON 供回读还原绑定（借鉴 Aidoku 的 AIDOKUDATA）
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

/// 文件名清洗：剥离路径非法字符、折叠空白、trim 首尾空格与点
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
		// authors/artists 空回退 Unknown；未知名也落 Unknown
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
		// folder 格式产物是目录，永不补扩展名
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
		// 目标父路径被同名文件堵住：建目录必败，产物必须留在原位
		let blocker = tmp.path().join("blocker");
		std::fs::write(&blocker, b"x").expect("write blocker");

		let result = move_product(&src, &blocker.join("a.cbz")).await;
		assert!(result.is_err());
		assert!(src.exists(), "source product must stay in place");
	}
}
