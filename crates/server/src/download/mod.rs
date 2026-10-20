use crate::model::{ChapterRow, MangaRow, unix_now};
use crate::source::{chapter_row, fetch_page_image, http_get, load_pages, manga_row};
use crate::state::AppState;
use crate::USER_ID;
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
}

/// 下载管理器：源内串行、源间并行，全局并发源数受 semaphore 限制。
/// 状态机落 download 表（queued → downloading → (paused) → finished / error），重启可恢复。
pub struct DownloadManager {
	// Weak 防循环引用：AppState 经 OnceCell 持有本管理器
	state: Weak<AppState>,
	pool: SqlitePool,
	queue: Mutex<VecDeque<i64>>,
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

	fn emit(&self, kind: &'static str, chapter_id: i64, downloaded: i64, total: i64) {
		let _ = self.events.send(DownloadEvent {
			kind,
			chapter_id,
			downloaded,
			total,
			running: None,
		});
	}

	fn emit_running(&self, running: bool) {
		let _ = self.events.send(DownloadEvent {
			kind: "running",
			chapter_id: 0,
			downloaded: 0,
			total: 0,
			running: Some(running),
		});
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
			self.queue.lock().await.push_back(chapter_id);
			queued.push(chapter_id);
			self.emit("queued", chapter_id, 0, 0);
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
		let ids: Vec<i64> = match sqlx::query_scalar(
			"SELECT chapter_id FROM download WHERE status = 'queued'
			 ORDER BY created_at ASC, chapter_id ASC",
		)
		.fetch_all(&self.pool)
		.await
		{
			Ok(ids) => ids,
			Err(e) => {
				tracing::warn!("download queue reload failed: {e}");
				return;
			}
		};
		{
			let mut queue = self.queue.lock().await;
			queue.clear();
			queue.extend(ids);
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
			let rows: Vec<i64> =
				sqlx::query_scalar("SELECT chapter_id FROM download WHERE status = 'queued'")
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
			for chapter_id in rows {
				self.emit("paused", chapter_id, 0, 0);
			}
		}
	}

	pub async fn mark_cancel(&self, chapter_id: i64) {
		self.cancel.lock().await.insert(chapter_id);
	}

	pub async fn remove_from_queue(&self, chapter_id: i64) {
		self.queue.lock().await.retain(|&id| id != chapter_id);
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
			let Some(chapter_id) = next else { break };
			if !self.running.load(Ordering::SeqCst) {
				deferred.push(chapter_id);
				continue;
			}
			let row: Option<(String, i64)> = match sqlx::query_as(
				"SELECT d.status, m.source_id FROM download d
				 JOIN chapter c ON c.id = d.chapter_id JOIN manga m ON m.id = c.manga_id
				 WHERE d.chapter_id = ?",
			)
			.bind(chapter_id)
			.fetch_optional(&self.pool)
			.await
			{
				Ok(row) => row,
				Err(e) => {
					tracing::warn!(chapter_id, "download dispatch query failed: {e}");
					deferred.push(chapter_id);
					break;
				}
			};
			// 行可能已被 DELETE 掉；非 queued（例如暂停期间的行）不重派
			let Some((status, source_id)) = row else { continue };
			if status != "queued" {
				continue;
			}
			if self.active_sources.lock().await.contains(&source_id) {
				deferred.push(chapter_id);
				continue;
			}
			let Ok(permit) = self.semaphore.clone().try_acquire_owned() else {
				deferred.push(chapter_id);
				continue;
			};
			self.active_sources.lock().await.insert(source_id);
			let manager = self.clone();
			tokio::spawn(async move {
				let worker = manager.clone();
				// 里层 panic 也要放行清理，否则该源永远占住 active 位
				let inner = tokio::spawn(async move { worker.run_chapter(chapter_id, source_id).await });
				let _ = inner.await;
				drop(permit);
				manager.active_sources.lock().await.remove(&source_id);
				manager.notify.notify_one();
			});
		}
		if !deferred.is_empty() {
			let mut queue = self.queue.lock().await;
			for id in deferred {
				queue.push_back(id);
			}
		}
	}

	async fn run_chapter(self: &Arc<Self>, chapter_id: i64, source_id: i64) {
		if self.cancel.lock().await.remove(&chapter_id) {
			self.cleanup_tmp(chapter_id).await;
			return;
		}
		let marked = sqlx::query(
			"UPDATE download SET status = 'downloading', updated_at = ? WHERE chapter_id = ?",
		)
		.bind(unix_now())
		.bind(chapter_id)
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
			match self.try_chapter(chapter_id, source_id, attempt == 1).await {
				Ok((total, path)) => {
					if self.cancel.lock().await.remove(&chapter_id) {
						self.progress.lock().await.remove(&chapter_id);
						self.cleanup_tmp(chapter_id).await;
						return;
					}
					let finished = sqlx::query(
						"UPDATE download SET status = 'finished', path = ?, error = NULL, updated_at = ?
						 WHERE chapter_id = ?",
					)
					.bind(&path)
					.bind(unix_now())
					.bind(chapter_id)
					.execute(&self.pool)
					.await;
					if let Err(e) = finished {
						tracing::warn!(chapter_id, "download finish update failed: {e}");
					}
					let flag = sqlx::query(
						"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 1)
						 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 1",
					)
					.bind(chapter_id)
					.bind(USER_ID)
					.execute(&self.pool)
					.await;
					if let Err(e) = flag {
						tracing::warn!(chapter_id, "download flag update failed: {e}");
					}
					self.progress.lock().await.remove(&chapter_id);
					self.emit("finished", chapter_id, total, total);
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
						"SELECT retries FROM download WHERE chapter_id = ?",
					)
					.bind(chapter_id)
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
							 WHERE chapter_id = ?",
						)
						.bind(&message)
						.bind(unix_now())
						.bind(chapter_id)
						.execute(&self.pool)
						.await;
						if let Err(e) = result {
							tracing::warn!(chapter_id, "download error update failed: {e}");
						}
						self.emit("error", chapter_id, downloaded, total);
						return;
					}
					let result = sqlx::query(
						"UPDATE download SET retries = retries + 1, error = ?, updated_at = ?
						 WHERE chapter_id = ?",
					)
					.bind(&message)
					.bind(unix_now())
					.bind(chapter_id)
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
			sqlx::query_as("SELECT format FROM download WHERE chapter_id = ?")
				.bind(chapter_id)
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
			self.emit("started", chapter_id, 0, total);
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
					self.emit("progress", chapter_id, done, total);
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
		let base = sanitize(&chapter_display(&chapter));
		let rel_path = if format == "folder" {
			format!("{rel_dir}/{base}")
		} else {
			format!("{rel_dir}/{base}.cbz")
		};
		package(&self.downloads_dir, &tmp_dir, &rel_path, &format, &files, &comic_info).await?;
		Ok((total, rel_path))
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
