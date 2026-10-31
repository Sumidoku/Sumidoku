//! Persistent accounting and clearing for derived chapter page caches.

use crate::error::ApiResult;
use crate::model::unix_now;
use serde::Serialize;
use sqlx::SqlitePool;
use std::path::Path;

pub const DEFAULT_MAX_BYTES: i64 = 2 * 1024 * 1024 * 1024;
pub const MIN_MAX_BYTES: i64 = 0;

/// Byte split of one chapter's (or an aggregate's) cache usage. The total `bytes`
/// column always equals the sum of these components.
#[derive(Debug, Default, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterCacheBreakdown {
	pub page_bytes: i64,
	pub original_bytes: i64,
	pub blob_bytes: i64,
	pub opds_bytes: i64,
}

impl ChapterCacheBreakdown {
	fn total(&self) -> i64 {
		self
			.page_bytes
			.saturating_add(self.original_bytes)
			.saturating_add(self.blob_bytes)
			.saturating_add(self.opds_bytes)
	}
}

/// Per-source aggregate of the chapter cache, for scoped reporting and clearing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterCacheSourceStats {
	pub source_id: i64,
	pub source_name: String,
	pub used_bytes: i64,
	pub chapter_count: i64,
	pub breakdown: ChapterCacheBreakdown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterCacheStats {
	pub enabled: bool,
	pub max_bytes: i64,
	pub used_bytes: i64,
	pub chapter_count: i64,
	pub last_error: Option<String>,
	pub cache_revision: i64,
	pub breakdown: ChapterCacheBreakdown,
	pub sources: Vec<ChapterCacheSourceStats>,
}

/// Revision is stable between rebuilds and changes whenever that chapter's cache is cleared.
pub async fn chapter_revision(pool: &SqlitePool, chapter_id: i64) -> ApiResult<i64> {
	let revision: Option<i64> =
		sqlx::query_scalar("SELECT revision FROM chapter_cache_state WHERE chapter_id = ?")
			.bind(chapter_id)
			.fetch_optional(pool)
			.await?;
	if let Some(revision) = revision {
		Ok(revision)
	} else {
		Ok(
			sqlx::query_scalar("SELECT revision FROM chapter_cache_meta WHERE id=1")
				.fetch_one(pool)
				.await?,
		)
	}
}

pub async fn publish_chapter(state: &std::sync::Arc<crate::state::AppState>, chapter_id: i64) -> ApiResult<()> {
	let accounting_lock = state.chapter_accounting_lock(chapter_id).await;
	let _accounting = accounting_lock.lock().await;
	let pool = &state.pool;
	let blobs = state.config.page_blobs_dir();
	let opds = state.config.opds_cbz_dir();
	let pages: i64 = sqlx::query_scalar(
		"SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM page WHERE chapter_id=?",
	)
	.bind(chapter_id)
	.fetch_one(pool)
	.await?;
	let (original_bytes, blob_bytes, opds_bytes) =
		chapter_disk_breakdown(blobs.clone(), opds.clone(), chapter_id).await?;
	let mut tx = crate::db::begin_write(pool).await?;
	let revision: i64 = sqlx::query_scalar(
		"UPDATE chapter_cache_meta SET revision=revision+1 WHERE id=1 RETURNING revision",
	)
	.fetch_one(&mut *tx)
	.await?;
	let breakdown = ChapterCacheBreakdown {
		page_bytes: pages,
		original_bytes,
		blob_bytes,
		opds_bytes,
	};
	let bytes = breakdown.total();
	sqlx::query("INSERT INTO chapter_cache_state(chapter_id,bytes,page_bytes,original_bytes,blob_bytes,opds_bytes,accessed_at,revision) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(chapter_id) DO UPDATE SET bytes=excluded.bytes, page_bytes=excluded.page_bytes, original_bytes=excluded.original_bytes, blob_bytes=excluded.blob_bytes, opds_bytes=excluded.opds_bytes, accessed_at=excluded.accessed_at, revision=excluded.revision, last_error=NULL")
		.bind(chapter_id).bind(bytes).bind(breakdown.page_bytes).bind(breakdown.original_bytes).bind(breakdown.blob_bytes).bind(breakdown.opds_bytes).bind(unix_now()).bind(revision).execute(&mut *tx).await?;
	tx.commit().await?;
	Ok(())
}

fn tree_bytes(path: &Path) -> std::io::Result<i64> {
	match std::fs::symlink_metadata(path) {
		Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Ok(0),
		Ok(_) => {}
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
		Err(error) => return Err(error),
	}
	let entries = match std::fs::read_dir(path) {
		Ok(entries) => entries,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
		Err(error) => return Err(error),
	};
	let mut bytes = 0i64;
	for entry in entries {
		let entry = entry?;
		let kind = entry.file_type()?;
		if kind.is_symlink() {
			continue;
		}
		if kind.is_dir() {
			bytes = bytes.saturating_add(tree_bytes(&entry.path())?);
		} else {
			bytes = bytes.saturating_add(entry.metadata()?.len().min(i64::MAX as u64) as i64);
		}
	}
	Ok(bytes)
}

/// Disk usage of one chapter split into remote originals, materialized local image blobs
/// and the OPDS archive. The originals tree lives inside the blob directory, so its bytes
/// are subtracted from the blob total.
pub(crate) fn chapter_disk_breakdown_blocking(
	blobs: &Path,
	opds: &Path,
	id: i64,
) -> std::io::Result<(i64, i64, i64)> {
	let chapter_dir = blobs.join(id.to_string());
	let blob_total = tree_bytes(&chapter_dir)?;
	let original_bytes = tree_bytes(&chapter_dir.join("originals"))?;
	let path = opds.join(id.to_string());
	let archive_bytes = match std::fs::symlink_metadata(&path) {
		Ok(meta) if meta.file_type().is_symlink() => 0,
		Ok(meta) if meta.is_dir() => tree_bytes(&path)?,
		Ok(meta) => meta.len().min(i64::MAX as u64) as i64,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
		Err(error) => return Err(error),
	};
	Ok((
		original_bytes,
		blob_total.saturating_sub(original_bytes),
		archive_bytes,
	))
}

/// Test-only convenience wrapper over the breakdown scan.
#[cfg(test)]
pub(crate) fn chapter_disk_bytes_blocking(blobs: &Path, opds: &Path, id: i64) -> std::io::Result<i64> {
	let (originals, blobs, opds) = chapter_disk_breakdown_blocking(blobs, opds, id)?;
	Ok(originals.saturating_add(blobs).saturating_add(opds))
}

async fn chapter_disk_breakdown(
	blobs: std::path::PathBuf,
	opds: std::path::PathBuf,
	id: i64,
) -> ApiResult<(i64, i64, i64)> {
	tokio::task::spawn_blocking(move || chapter_disk_breakdown_blocking(&blobs, &opds, id))
		.await
		.map_err(|error| {
			crate::error::ApiError::internal(format!("chapter cache scan failed: {error}"))
		})?
		.map_err(|error| {
			crate::error::ApiError::internal(format!("chapter cache scan failed: {error}"))
		})
}

/// Cache persistence is enabled only with a positive budget; LRU collection always runs.
pub async fn settings(pool: &SqlitePool) -> ApiResult<(bool, i64)> {
	let raw: Option<String> = sqlx::query_scalar("SELECT value FROM setting WHERE key='cache.chapterMaxBytes'")
		.fetch_optional(pool).await?;
	let max_bytes = raw.and_then(|raw| serde_json::from_str::<i64>(&raw).ok())
		.filter(|value| *value >= 0).unwrap_or(DEFAULT_MAX_BYTES);
	Ok((max_bytes > 0, max_bytes))
}

pub async fn touch(pool: &SqlitePool, chapter_id: i64) -> ApiResult<()> {
	let _ = sqlx::query("INSERT INTO chapter_cache_state(chapter_id, bytes, accessed_at) VALUES (?, 0, ?) ON CONFLICT(chapter_id) DO UPDATE SET accessed_at=excluded.accessed_at")
		.bind(chapter_id).bind(unix_now()).execute(pool).await?;
	Ok(())
}

/// Recency is advisory and must never make a readable chapter fail.
pub async fn touch_best_effort(pool: &SqlitePool, chapter_id: i64) {
	if let Err(error) = touch(pool, chapter_id).await {
		tracing::warn!(chapter_id, "chapter cache recency update failed: {}", error.message());
	}
}

fn original_dir(state: &crate::state::AppState, chapter_id: i64, index: i64) -> std::path::PathBuf {
	state.config.page_blobs_dir().join(chapter_id.to_string()).join("originals").join(index.to_string())
}

pub fn original_identity(source_id: i64, content: &serde_json::Value) -> ApiResult<Vec<u8>> {
	serde_json::to_vec(&(source_id, content)).map_err(Into::into)
}

fn original_digest(bytes: &[u8], mime: &str, identity: &[u8]) -> String {
	use sha2::Digest;
	let mut hasher = sha2::Sha256::new();
	hasher.update(identity);
	hasher.update([0]);
	hasher.update(mime.as_bytes());
	hasher.update([0]);
	hasher.update(bytes);
	hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Serializes concurrent reader/download misses for one page. Download callers pass `persist`
/// false: they can reuse an overlapping reader's persistent/fetched result without creating cache
/// files of their own. Successful in-flight bytes live only while callers hold the weakly-indexed
/// flight Arc.
pub async fn page_original<F>(
	state: &std::sync::Arc<crate::state::AppState>,
	chapter_id: i64,
	index: i64,
	source_id: i64,
	content: &serde_json::Value,
	persist: bool,
	fetch: F,
) -> ApiResult<(Vec<u8>, String)>
where
	F: std::future::Future<Output = ApiResult<(Vec<u8>, String)>>,
{
	let identity = original_identity(source_id, content)?;
	let flight = state.chapter_original_lock(chapter_id, index).await;
	let _guard = flight.lock.lock().await;
	let enabled = settings(&state.pool).await?.0;
	if let Some(value) = flight.cached(&identity) {
		let value = (value.0.clone(), value.1.clone());
		if enabled && persist && read_page_original(state, chapter_id, index, source_id, content).await.is_none() {
			if let Err(error) = write_page_original(state, chapter_id, index, source_id, content, &value.0, &value.1).await {
				tracing::warn!(chapter_id, index, "page original cache write failed: {}", error.message());
			} else {
				let _ = enforce_limit(state).await;
			}
		}
		return Ok(value);
	}
	if let Some(value) = if enabled { read_page_original(state, chapter_id, index, source_id, content).await } else { None } {
		flight.publish(identity, value.0.clone(), value.1.clone());
		return Ok(value);
	}
	let value = fetch.await?;
	if persist && enabled {
		if let Err(error) = write_page_original(state, chapter_id, index, source_id, content, &value.0, &value.1).await {
			tracing::warn!(chapter_id, index, "page original cache write failed: {}", error.message());
		} else {
			let _ = enforce_limit(state).await;
		}
	}
	flight.publish(identity, value.0.clone(), value.1.clone());
	Ok(value)
}

/// Returns a validated exact upstream response. The caller holds the per-page original lock.
pub async fn read_page_original(
	state: &crate::state::AppState,
	chapter_id: i64,
	index: i64,
	source_id: i64,
	content: &serde_json::Value,
) -> Option<(Vec<u8>, String)> {
	let dir = original_dir(state, chapter_id, index);
	let bytes = tokio::fs::read(dir.join("bytes")).await.ok()?;
	let mime = tokio::fs::read_to_string(dir.join("mime")).await.ok()?;
	let identity = tokio::fs::read(dir.join("identity")).await.ok()?;
	let digest = tokio::fs::read_to_string(dir.join("sha256")).await.ok()?;
	if mime.is_empty() || original_identity(source_id, content).ok()?.as_slice() != identity {
		return None;
	}
	let actual = original_digest(&bytes, &mime, &identity);
	(actual == digest).then_some((bytes, mime))
}

/// Stores an exact remote page response as a staged directory. The caller holds the per-page
/// original lock and chapter cache read lease, so readers and clear/refresh cannot see partial data.
pub async fn write_page_original(
	state: &std::sync::Arc<crate::state::AppState>,
	chapter_id: i64,
	index: i64,
	source_id: i64,
	content: &serde_json::Value,
	bytes: &[u8],
	mime: &str,
) -> ApiResult<()> {
	let dir = original_dir(state, chapter_id, index);
	let parent = dir.parent().expect("originals parent");
	let staging = parent.join(format!(".{index}.staging"));
	let accounting_lock = state.chapter_accounting_lock(chapter_id).await;
	let _accounting = accounting_lock.lock().await;
	if !settings(&state.pool).await?.0 { return Ok(()); }
	let before = original_tree_bytes(dir.clone()).await?.saturating_add(original_tree_bytes(staging.clone()).await?);
	let intact = original_entry_is_intact(&dir).await;
	let identity = original_identity(source_id, content)?;
	let digest = original_digest(bytes, mime, &identity);
	let result = async {
		tokio::fs::create_dir_all(parent).await?;
		let _ = tokio::fs::remove_dir_all(&staging).await;
		tokio::fs::create_dir(&staging).await?;
		tokio::fs::write(staging.join("bytes"), bytes).await?;
		tokio::fs::write(staging.join("mime"), mime).await?;
		tokio::fs::write(staging.join("identity"), &identity).await?;
		tokio::fs::write(staging.join("sha256"), digest).await?;
		let _ = tokio::fs::remove_dir_all(&dir).await;
		tokio::fs::rename(&staging, &dir).await?;
		Ok::<(), std::io::Error>(())
	}.await;
	if result.is_err() {
		let _ = tokio::fs::remove_dir_all(&staging).await;
	}
	if intact || before == 0 {
		let after = original_tree_bytes(dir).await?.saturating_add(original_tree_bytes(staging).await?);
		update_byte_delta(state, chapter_id, after - before).await?;
	} else {
		reconcile_chapter(&state.pool, &state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), chapter_id).await?;
	}
	result.map_err(|error| crate::error::ApiError::internal(format!("write page original cache: {error}")))
}

async fn update_byte_delta(state: &crate::state::AppState, chapter_id: i64, delta: i64) -> ApiResult<()> {
	sqlx::query("UPDATE chapter_cache_state SET bytes=max(0,bytes+?), original_bytes=max(0,original_bytes+?), accessed_at=? WHERE chapter_id=?")
		.bind(delta).bind(delta).bind(unix_now()).bind(chapter_id).execute(&state.pool).await?;
	Ok(())
}

async fn original_tree_bytes(path: std::path::PathBuf) -> ApiResult<i64> {
	tokio::task::spawn_blocking(move || tree_bytes(&path)).await
		.map_err(|error| crate::error::ApiError::internal(format!("page original accounting task failed: {error}")))?
		.map_err(|error| crate::error::ApiError::internal(format!("inspect page original cache: {error}")))
}

async fn original_entry_is_intact(dir: &Path) -> bool {
	let Ok(bytes) = tokio::fs::read(dir.join("bytes")).await else { return false; };
	let Ok(mime) = tokio::fs::read_to_string(dir.join("mime")).await else { return false; };
	let Ok(identity) = tokio::fs::read(dir.join("identity")).await else { return false; };
	let Ok(digest) = tokio::fs::read_to_string(dir.join("sha256")).await else { return false; };
	!mime.is_empty() && original_digest(&bytes, &mime, &identity) == digest
}

pub async fn reconcile_chapter(
	pool: &SqlitePool,
	blobs: &Path,
	opds: &Path,
	id: i64,
) -> ApiResult<()> {
	let pages: i64 = sqlx::query_scalar(
		"SELECT coalesce(sum(length(CAST(content AS BLOB))), 0) FROM page WHERE chapter_id = ?",
	)
	.bind(id)
	.fetch_one(pool)
	.await?;
	let (original_bytes, blob_bytes, opds_bytes) =
		chapter_disk_breakdown(blobs.to_path_buf(), opds.to_path_buf(), id).await?;
	let breakdown = ChapterCacheBreakdown {
		page_bytes: pages,
		original_bytes,
		blob_bytes,
		opds_bytes,
	};
	let bytes = breakdown.total();
	let _ = sqlx::query("INSERT INTO chapter_cache_state(chapter_id, bytes, page_bytes, original_bytes, blob_bytes, opds_bytes, accessed_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(chapter_id) DO UPDATE SET bytes=excluded.bytes, page_bytes=excluded.page_bytes, original_bytes=excluded.original_bytes, blob_bytes=excluded.blob_bytes, opds_bytes=excluded.opds_bytes")
		.bind(id).bind(bytes).bind(breakdown.page_bytes).bind(breakdown.original_bytes).bind(breakdown.blob_bytes).bind(breakdown.opds_bytes).bind(unix_now()).execute(pool).await?;
	Ok(())
}

pub async fn reconcile_chapter_accounted(state: &std::sync::Arc<crate::state::AppState>, id: i64) -> ApiResult<()> {
	let lock = state.chapter_accounting_lock(id).await;
	let _guard = lock.lock().await;
	reconcile_chapter(&state.pool, &state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), id).await
}

type SourceStatRow = (i64, String, i64, i64, i64, i64, i64, i64);

pub async fn stats(
	pool: &SqlitePool,
	_blobs: &Path,
	_opds: &Path,
	max_bytes: i64,
) -> ApiResult<ChapterCacheStats> {
	let (used, count, last_error): (i64, i64, Option<String>) = sqlx::query_as(
		"SELECT coalesce(sum(bytes),0), coalesce(sum(bytes > 0),0), max(last_error) FROM chapter_cache_state"
	).fetch_one(pool).await?;
	let (page_bytes, original_bytes, blob_bytes, opds_bytes): (i64, i64, i64, i64) = sqlx::query_as(
		"SELECT coalesce(sum(page_bytes),0), coalesce(sum(original_bytes),0), coalesce(sum(blob_bytes),0), coalesce(sum(opds_bytes),0) FROM chapter_cache_state"
	).fetch_one(pool).await?;
	let source_rows: Vec<SourceStatRow> = sqlx::query_as(
		"SELECT m.source_id, s.name, coalesce(sum(st.bytes),0), coalesce(sum(st.bytes > 0),0), \
		 coalesce(sum(st.page_bytes),0), coalesce(sum(st.original_bytes),0), \
		 coalesce(sum(st.blob_bytes),0), coalesce(sum(st.opds_bytes),0) \
		 FROM chapter_cache_state st \
		 JOIN chapter c ON c.id = st.chapter_id \
		 JOIN manga m ON m.id = c.manga_id \
		 JOIN source s ON s.id = m.source_id \
		 GROUP BY m.source_id \
		 HAVING coalesce(sum(st.bytes),0) > 0 \
		 ORDER BY sum(st.bytes) DESC, m.source_id ASC"
	).fetch_all(pool).await?;
	let revision: i64 = sqlx::query_scalar("SELECT revision FROM chapter_cache_meta WHERE id=1")
		.fetch_one(pool)
		.await?;
	Ok(ChapterCacheStats {
		enabled: max_bytes > 0,
		max_bytes: max_bytes.max(MIN_MAX_BYTES),
		used_bytes: used,
		chapter_count: count,
		last_error,
		cache_revision: revision,
		breakdown: ChapterCacheBreakdown {
			page_bytes,
			original_bytes,
			blob_bytes,
			opds_bytes,
		},
		sources: source_rows
			.into_iter()
			.map(
				|(source_id, source_name, used_bytes, chapter_count, pages, originals, blobs, opds)| {
					ChapterCacheSourceStats {
						source_id,
						source_name,
						used_bytes,
						chapter_count,
						breakdown: ChapterCacheBreakdown {
							page_bytes: pages,
							original_bytes: originals,
							blob_bytes: blobs,
							opds_bytes: opds,
						},
					}
				},
			)
			.collect(),
	})
}

/// Startup reconciliation charges historical page rows, blob trees and OPDS archives, then
/// removes cache files whose chapter row no longer exists without following symlinks.
pub async fn reconcile_all(pool: &SqlitePool, blobs: &Path, opds: &Path) -> ApiResult<()> {
	let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM chapter")
		.fetch_all(pool)
		.await?;
	let known = ids.into_iter().collect::<std::collections::HashSet<i64>>();
	let blobs = blobs.to_path_buf();
	let opds = opds.to_path_buf();
	let orphan_blobs = blobs.clone();
	let orphan_opds = opds.clone();
	let orphan_known = known.clone();
	tokio::task::spawn_blocking(move || remove_orphans(&orphan_blobs, &orphan_opds, &orphan_known))
		.await
		.map_err(|error| {
			crate::error::ApiError::internal(format!(
				"chapter cache reconciliation failed: {error}"
			))
		})?
		.map_err(|error| {
			crate::error::ApiError::internal(format!(
				"chapter cache reconciliation failed: {error}"
			))
		})?;
	let staging_root = blobs.clone();
	tokio::task::spawn_blocking(move || remove_nested_staging(&staging_root))
		.await
		.map_err(|error| crate::error::ApiError::internal(format!("chapter cache staging cleanup failed: {error}")))?
		.map_err(|error| crate::error::ApiError::internal(format!("chapter cache staging cleanup failed: {error}")))?;
	for id in &known {
		reconcile_chapter(pool, &blobs, &opds, *id).await?;
	}
	Ok(())
}

fn remove_orphans(
	blobs: &Path,
	opds: &Path,
	known: &std::collections::HashSet<i64>,
) -> std::io::Result<()> {
	for root in [blobs, opds] {
		let root_meta = match std::fs::symlink_metadata(root) {
			Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => continue,
			Ok(meta) => meta,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
			Err(error) => return Err(error),
		};
		let _ = root_meta;
		let entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
		for item in &entries {
			let Some(name) = item.file_name().to_str().map(str::to_string) else {
				continue;
			};
			if let Some(id) = name
				.strip_suffix(".previous")
				.and_then(|s| s.parse::<i64>().ok())
			{
				let previous = item.path();
				let canonical = root.join(id.to_string());
				let kind = item.file_type()?;
				if kind.is_symlink() || !known.contains(&id) {
					remove_path_sync(&previous)?;
				} else if !canonical.exists() {
					std::fs::rename(previous, canonical)?;
				} else {
					remove_path_sync(&previous)?;
				}
			}
		}
		for item in entries {
			let kind = item.file_type()?;
			if kind.is_symlink() {
				continue;
			}
			if item.file_name().to_string_lossy().ends_with(".previous") {
				continue;
			}
			if item.file_name().to_string_lossy().ends_with(".staging") {
				remove_path_sync(&item.path())?;
				continue;
			}
			let Some(id) = item
				.file_name()
				.to_str()
				.and_then(|name| name.parse::<i64>().ok())
			else {
				if kind.is_dir() {
					std::fs::remove_dir_all(item.path())?;
				} else {
					std::fs::remove_file(item.path())?;
				}
				continue;
			};
			if !known.contains(&id) {
				if kind.is_dir() {
					std::fs::remove_dir_all(item.path())?;
				} else {
					std::fs::remove_file(item.path())?;
				}
			}
		}
	}
	Ok(())
}

fn remove_nested_staging(path: &Path) -> std::io::Result<()> {
	let meta = match std::fs::symlink_metadata(path) {
		Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Ok(()),
		Ok(meta) => meta,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error),
	};
	let _ = meta;
	for entry in std::fs::read_dir(path)? {
		let entry = entry?;
		let kind = entry.file_type()?;
		if kind.is_symlink() { continue; }
		let child = entry.path();
		if kind.is_dir() && entry.file_name().to_string_lossy().ends_with(".staging") {
			std::fs::remove_dir_all(child)?;
		} else if kind.is_dir() {
			remove_nested_staging(&child)?;
		}
	}
	Ok(())
}

fn remove_path_sync(path: &Path) -> std::io::Result<()> {
	match std::fs::symlink_metadata(path) {
		Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => std::fs::remove_file(path),
		Ok(_) => std::fs::remove_dir_all(path),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error),
	}
}

async fn remove_path_async(path: &Path) -> std::io::Result<()> {
	match tokio::fs::symlink_metadata(path).await {
		Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
			tokio::fs::remove_file(path).await
		}
		Ok(_) => tokio::fs::remove_dir_all(path).await,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error),
	}
}

pub async fn clear(pool: &SqlitePool, blobs: &Path, opds: &Path) -> ApiResult<(i64, i64, i64)> {
	let ids: Vec<i64> = sqlx::query_scalar(
		"SELECT id FROM chapter WHERE pages_analyzed_at IS NOT NULL
		 UNION SELECT chapter_id FROM chapter_cache_state
		 UNION SELECT chapter_id FROM page",
	)
	.fetch_all(pool)
	.await?;
	clear_inner(pool, blobs, opds, None, ids).await
}

/// Scoped variant of [`clear`]: only chapters belonging to one source. Download products
/// are untouched here the same as in the global clear.
pub async fn clear_for_source(
	pool: &SqlitePool,
	blobs: &Path,
	opds: &Path,
	source_id: i64,
) -> ApiResult<(i64, i64, i64)> {
	let ids: Vec<i64> = sqlx::query_scalar(
		"SELECT id FROM chapter WHERE manga_id IN (SELECT id FROM manga WHERE source_id = ?)
		 AND (pages_analyzed_at IS NOT NULL
		 OR id IN (SELECT chapter_id FROM chapter_cache_state)
		 OR id IN (SELECT chapter_id FROM page))",
	)
	.bind(source_id)
	.fetch_all(pool)
	.await?;
	clear_inner(pool, blobs, opds, None, ids).await
}

async fn clear_inner(
	pool: &SqlitePool,
	blobs: &Path,
	opds: &Path,
	fail_on: Option<&Path>,
	ids: Vec<i64>,
) -> ApiResult<(i64, i64, i64)> {
	let current = stats(pool, blobs, opds, DEFAULT_MAX_BYTES).await?;
	let mut failures = Vec::<(i64, String)>::new();
	for id in &ids {
		for suffix in ["", ".previous", ".staging"] {
			for root in [blobs, opds] {
				let path = root.join(format!("{id}{suffix}"));
				let removal = if fail_on == Some(path.as_path()) {
					Err(std::io::Error::new(
						std::io::ErrorKind::PermissionDenied,
						"injected removal failure",
					))
				} else {
					remove_path_async(&path).await
				};
				if let Err(error) = removal {
					failures.push((*id, format!("chapter {id} cache path: {error}")));
				}
			}
		}
	}
	let mut tx = crate::db::begin_write(pool).await?;
	let revision = if ids.is_empty() {
		None
	} else {
		Some(
			sqlx::query_scalar::<_, i64>(
				"UPDATE chapter_cache_meta SET revision=revision+1 WHERE id=1 RETURNING revision",
			)
			.fetch_one(&mut *tx)
			.await?,
		)
	};
	for id in &ids {
		sqlx::query("DELETE FROM page WHERE chapter_id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		sqlx::query("INSERT INTO chapter_cache_state(chapter_id,bytes,page_bytes,original_bytes,blob_bytes,opds_bytes,accessed_at,revision,last_error) VALUES (?,0,0,0,0,0,?,?,NULL) ON CONFLICT(chapter_id) DO UPDATE SET bytes=0, page_bytes=0, original_bytes=0, blob_bytes=0, opds_bytes=0, revision=excluded.revision, accessed_at=excluded.accessed_at, last_error=NULL")
			.bind(id).bind(unix_now()).bind(revision.unwrap_or(1)).execute(&mut *tx).await?;
	}
	tx.commit().await?;
	for id in &ids {
		reconcile_chapter(pool, blobs, opds, *id).await?;
	}
	if !failures.is_empty() {
		let message = failures
			.iter()
			.map(|(_, error)| error.as_str())
			.collect::<Vec<_>>()
			.join("; ");
		for (id, error) in &failures {
			let _ = sqlx::query("UPDATE chapter_cache_state SET last_error=? WHERE chapter_id=?")
				.bind(error)
				.bind(id)
				.execute(pool)
				.await;
		}
		return Err(crate::error::ApiError::internal(format!(
			"chapter cache clear incomplete: {message}"
		)));
	}
	let after = stats(pool, blobs, opds, DEFAULT_MAX_BYTES).await?;
	Ok((
		current.used_bytes.saturating_sub(after.used_bytes),
		after.used_bytes,
		after.chapter_count,
	))
}

pub async fn clear_state(
	state: &std::sync::Arc<crate::state::AppState>,
) -> ApiResult<(i64, i64, i64)> {
	let _lease = state.chapter_cache_gate.write().await;
	clear(
		&state.pool,
		&state.config.page_blobs_dir(),
		&state.config.config_dir.join("opds-cbz"),
	)
	.await
}

/// Scoped clear under the same global write lease: readers never observe a half-cleared chapter.
pub async fn clear_source_state(
	state: &std::sync::Arc<crate::state::AppState>,
	source_id: i64,
) -> ApiResult<(i64, i64, i64)> {
	let _lease = state.chapter_cache_gate.write().await;
	clear_for_source(
		&state.pool,
		&state.config.page_blobs_dir(),
		&state.config.config_dir.join("opds-cbz"),
		source_id,
	)
	.await
}

/// Enforce the configured soft limit oldest-first. The global read lease excludes a manual clear;
/// a per-chapter write lease skips only assets currently used by a reader or download.
pub async fn enforce_limit(state: &std::sync::Arc<crate::state::AppState>) -> ApiResult<()> {
	let (_, max_bytes) = settings(&state.pool).await?;
	enforce_limit_with_budget(state, max_bytes).await
}

async fn enforce_limit_with_budget(
	state: &std::sync::Arc<crate::state::AppState>,
	max_bytes: i64,
) -> ApiResult<()> {
	let Some(_collector) = state.try_chapter_cache_collect() else {
		return Ok(());
	};
	let Ok(_shared) = state.chapter_cache_gate.try_read() else {
		return Ok(());
	};
	let (used,): (i64,) = sqlx::query_as("SELECT coalesce(sum(bytes),0) FROM chapter_cache_state")
		.fetch_one(&state.pool)
		.await?;
	if used <= max_bytes {
		return Ok(());
	}
	let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT chapter_id, bytes FROM chapter_cache_state WHERE bytes > 0 AND accessed_at < ? ORDER BY accessed_at ASC, chapter_id ASC")
		.bind(unix_now() - 300).fetch_all(&state.pool).await?;
	let mut projected = used;
	for (id, _listed_bytes) in rows {
		if projected <= max_bytes {
			break;
		}
		let chapter_lock = state.chapter_cache_lock(id).await;
		let Ok(_chapter) = chapter_lock.try_write() else {
			continue;
		};
		let current: Option<(i64, i64)> =
			sqlx::query_as("SELECT bytes, accessed_at FROM chapter_cache_state WHERE chapter_id=?")
				.bind(id)
				.fetch_optional(&state.pool)
				.await?;
		let Some((current_bytes, accessed_at)) = current else {
			continue;
		};
		if current_bytes <= 0 || accessed_at >= unix_now() - 300 {
			continue;
		}
		let blob_path = state.config.page_blobs_dir().join(id.to_string());
		let opds_path = state
			.config
			.config_dir
			.join("opds-cbz")
			.join(id.to_string());
		let blob_result = remove_path_async(&blob_path).await;
		let opds_result = remove_path_async(&opds_path).await;
		let _ = remove_path_async(&blob_path.with_extension("previous")).await;
		let _ = remove_path_async(&blob_path.with_extension("staging")).await;
		let _ = remove_path_async(&opds_path.with_extension("previous")).await;
		let _ = remove_path_async(&opds_path.with_extension("staging")).await;
		let failure = if blob_result
			.as_ref()
			.is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
			|| opds_result
				.as_ref()
				.is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
		{
			Some(format!(
				"blob removal: {blob_result:?}; OPDS removal: {opds_result:?}"
			))
		} else {
			None
		};
		let mut tx = crate::db::begin_write(&state.pool).await?;
		let revision: i64 = sqlx::query_scalar(
			"UPDATE chapter_cache_meta SET revision=revision+1 WHERE id=1 RETURNING revision",
		)
		.fetch_one(&mut *tx)
		.await?;
		sqlx::query("DELETE FROM page WHERE chapter_id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		sqlx::query("UPDATE chapter SET pages_analyzed_at = NULL WHERE id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		sqlx::query(
			"UPDATE chapter_cache_state SET bytes=0, page_bytes=0, original_bytes=0, blob_bytes=0, opds_bytes=0, revision=?, last_error=? WHERE chapter_id=?",
		)
		.bind(revision)
		.bind(failure.as_deref())
		.bind(id)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		reconcile_chapter(
			&state.pool,
			&state.config.page_blobs_dir(),
			&state.config.config_dir.join("opds-cbz"),
			id,
		)
		.await?;
		if let Some(failure) = failure {
			let _ = sqlx::query("UPDATE chapter_cache_state SET last_error=? WHERE chapter_id=?")
				.bind(failure)
				.bind(id)
				.execute(&state.pool)
				.await?;
		}
		projected = sqlx::query_scalar("SELECT coalesce(sum(bytes),0) FROM chapter_cache_state")
			.fetch_one(&state.pool)
			.await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Arc;

	async fn app_state(tmp: &tempfile::TempDir) -> std::sync::Arc<crate::state::AppState> {
		let config = crate::config::Config {
			config_dir: tmp.path().to_path_buf(),
			database_url: format!("sqlite://{}/gate.db?mode=rwc", tmp.path().display()),
			downloads_dir: tmp.path().join("downloads"),
			..crate::config::Config::default()
		};
		let pool = crate::db::create_pool(&config.database_url)
			.await
			.expect("pool");
		let (defaults_tx, _) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _) = tokio::sync::mpsc::unbounded_channel();
		crate::state::AppState::new(pool, config, defaults_tx, cookie_tx)
	}

	async fn cached_chapter(
		state: &crate::state::AppState,
		key: &str,
		bytes: i64,
		accessed_at: i64,
	) -> i64 {
		let source: i64 = sqlx::query_scalar(
			"INSERT INTO source(key,name,wasm_path) VALUES (?, 'Source','') RETURNING id",
		)
		.bind(format!("source-{key}"))
		.fetch_one(&state.pool)
		.await
		.expect("source");
		let manga: i64 =
			sqlx::query_scalar("INSERT INTO manga(source_id,key) VALUES (?, ?) RETURNING id")
				.bind(source)
				.bind(format!("manga-{key}"))
				.fetch_one(&state.pool)
				.await
				.expect("manga");
		let chapter: i64 = sqlx::query_scalar(
			"INSERT INTO chapter(manga_id,key,page_count) VALUES (?, ?,1) RETURNING id",
		)
		.bind(manga)
		.bind(format!("chapter-{key}"))
		.fetch_one(&state.pool)
		.await
		.expect("chapter");
		sqlx::query(
			"INSERT INTO page(chapter_id,idx,content) VALUES (?,0,'{\"content\":\"cached\"}')",
		)
		.bind(chapter)
		.execute(&state.pool)
		.await
		.expect("page");
		sqlx::query("INSERT INTO chapter_cache_state(chapter_id,bytes,accessed_at) VALUES (?,?,?)")
			.bind(chapter)
			.bind(bytes)
			.bind(accessed_at)
			.execute(&state.pool)
			.await
			.expect("cache state");
		let blob = state.config.page_blobs_dir().join(chapter.to_string());
		std::fs::create_dir_all(&blob).expect("blob dir");
		std::fs::write(blob.join("0.png"), vec![b'x'; bytes as usize]).expect("blob");
		chapter
	}

	#[tokio::test]
	async fn breakdown_components_sum_to_total_and_stats_group_by_source() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let chapter = cached_chapter(&state, "breakdown", 100, unix_now()).await;
		// Originals subtree: counted as original bytes, not blob bytes.
		let originals = state.config.page_blobs_dir().join(chapter.to_string()).join("originals/0");
		std::fs::create_dir_all(&originals).expect("originals dir");
		std::fs::write(originals.join("bytes"), vec![b'o'; 40]).expect("original bytes");
		std::fs::write(originals.join("mime"), b"image/png").expect("original mime");
		let opds = state.config.config_dir.join("opds-cbz");
		std::fs::create_dir_all(&opds).expect("opds dir");
		std::fs::write(opds.join(chapter.to_string()), vec![b'z'; 7]).expect("opds archive");
		reconcile_chapter(&state.pool, &state.config.page_blobs_dir(), &opds, chapter)
			.await
			.expect("reconcile");
		let (bytes, page_bytes, original_bytes, blob_bytes, opds_bytes): (i64, i64, i64, i64, i64) =
			sqlx::query_as("SELECT bytes, page_bytes, original_bytes, blob_bytes, opds_bytes FROM chapter_cache_state WHERE chapter_id=?")
				.bind(chapter)
				.fetch_one(&state.pool)
				.await
				.expect("state row");
		assert_eq!(page_bytes, "{\"content\":\"cached\"}".len() as i64);
		assert_eq!(original_bytes, 40 + "image/png".len() as i64);
		assert_eq!(blob_bytes, 100);
		assert_eq!(opds_bytes, 7);
		assert_eq!(bytes, page_bytes + original_bytes + blob_bytes + opds_bytes);
		let report = stats(&state.pool, &state.config.page_blobs_dir(), &opds, DEFAULT_MAX_BYTES)
			.await
			.expect("stats");
		assert_eq!(report.used_bytes, bytes);
		assert_eq!(report.breakdown.total(), report.used_bytes);
		assert_eq!(report.breakdown.original_bytes, original_bytes);
		assert_eq!(report.sources.len(), 1);
		let source_row = &report.sources[0];
		assert_eq!(source_row.used_bytes, bytes);
		assert_eq!(source_row.chapter_count, 1);
		assert_eq!(source_row.breakdown.blob_bytes, blob_bytes);
	}

	#[tokio::test]
	async fn clear_for_source_only_clears_that_sources_chapters() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let first = cached_chapter(&state, "first", 100, unix_now()).await;
		let second = cached_chapter(&state, "second", 200, unix_now()).await;
		let blobs = state.config.page_blobs_dir();
		let opds = state.config.config_dir.join("opds-cbz");
		reconcile_chapter(&state.pool, &blobs, &opds, first).await.expect("reconcile first");
		reconcile_chapter(&state.pool, &blobs, &opds, second).await.expect("reconcile second");
		let second_source: i64 = sqlx::query_scalar(
			"SELECT m.source_id FROM manga m JOIN chapter c ON c.manga_id=m.id WHERE c.id=?",
		)
		.bind(second)
		.fetch_one(&state.pool)
		.await
		.expect("second source");
		let (cleared, used_after, _count) =
			clear_for_source(&state.pool, &blobs, &opds, second_source)
				.await
				.expect("scoped clear");
		assert!(cleared > 0, "scoped clear reports freed bytes");
		// The other source's chapter keeps its pages, blob files and accounting.
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
				.bind(first)
				.fetch_one(&state.pool)
				.await
				.expect("first pages"),
			1
		);
		assert!(blobs.join(first.to_string()).join("0.png").exists());
		assert!(!blobs.join(second.to_string()).exists());
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
				.bind(second)
				.fetch_one(&state.pool)
				.await
				.expect("second pages"),
			0
		);
		let remaining: i64 = sqlx::query_scalar(
			"SELECT bytes FROM chapter_cache_state WHERE chapter_id=?",
		)
		.bind(first)
		.fetch_one(&state.pool)
		.await
		.expect("first bytes");
		assert_eq!(used_after, remaining);
		let report = stats(&state.pool, &blobs, &opds, DEFAULT_MAX_BYTES)
			.await
			.expect("stats after");
		assert_eq!(report.sources.len(), 1);
		assert_eq!(report.sources[0].source_id, {
			sqlx::query_scalar::<_, i64>(
				"SELECT m.source_id FROM manga m JOIN chapter c ON c.manga_id=m.id WHERE c.id=?",
			)
			.bind(first)
			.fetch_one(&state.pool)
			.await
			.expect("first source")
		});
	}

	#[tokio::test]
	async fn overlapping_download_miss_and_reader_share_fetch_and_reader_persists_it() {
		use std::sync::atomic::{AtomicUsize, Ordering};
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let chapter = cached_chapter(&state, "flight", 0, unix_now()).await;
		let source: i64 = sqlx::query_scalar("SELECT m.source_id FROM manga m JOIN chapter c ON c.manga_id=m.id WHERE c.id=?")
			.bind(chapter).fetch_one(&state.pool).await.expect("source id");
		let content = serde_json::json!({"Url": ["http://fixture.invalid/page.jpg", null]});
		let count = Arc::new(AtomicUsize::new(0));
		let started = Arc::new(tokio::sync::Notify::new());
		let release = Arc::new(tokio::sync::Notify::new());
		let download_state = state.clone();
		let download_count = count.clone();
		let download_started = started.clone();
		let download_release = release.clone();
		let download_content = content.clone();
		let download = tokio::spawn(async move {
			page_original(&download_state, chapter, 0, source, &download_content, false, async move {
				download_count.fetch_add(1, Ordering::SeqCst);
				download_started.notify_one();
				download_release.notified().await;
				Ok((b"upstream-image".to_vec(), "image/jpeg".to_string()))
			}).await
		});
		started.notified().await;
		let flight = state.chapter_original_lock(chapter, 0).await;
		let reader_state = state.clone();
		let reader_count = count.clone();
		let reader_content = content.clone();
		let reader = tokio::spawn(async move {
			page_original(&reader_state, chapter, 0, source, &reader_content, true, async move {
				reader_count.fetch_add(1, Ordering::SeqCst);
				Ok((b"unexpected-refetch".to_vec(), "image/jpeg".to_string()))
			}).await
		});
		while Arc::strong_count(&flight) < 3 { tokio::task::yield_now().await; }
		drop(flight);
		release.notify_one();
		assert_eq!(download.await.expect("download task").expect("download bytes").0, b"upstream-image");
		assert_eq!(reader.await.expect("reader task").expect("reader bytes").0, b"upstream-image");
		assert_eq!(count.load(Ordering::SeqCst), 1);
		assert_eq!(read_page_original(&state, chapter, 0, source, &content).await.expect("reader persisted original"), (b"upstream-image".to_vec(), "image/jpeg".to_string()));
	}

	#[tokio::test]
	async fn corrupt_original_is_a_miss_and_write_failure_still_serves_fetched_bytes() {
		use std::sync::atomic::{AtomicUsize, Ordering};
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let chapter = cached_chapter(&state, "integrity", 0, unix_now()).await;
		let source: i64 = sqlx::query_scalar("SELECT m.source_id FROM manga m JOIN chapter c ON c.manga_id=m.id WHERE c.id=?")
			.bind(chapter).fetch_one(&state.pool).await.expect("source id");
		let content = serde_json::json!({"Zip": ["http://fixture.invalid/book.cbz", "page.png"]});
		crate::cache::reconcile_chapter(&state.pool, &state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), chapter).await.expect("initial accounting");
		page_original(&state, chapter, 0, source, &content, true, async { Ok((b"original".to_vec(), "image/png".to_string())) }).await.expect("cache original");
		let original_dir = state.config.page_blobs_dir().join(chapter.to_string()).join("originals/0");
		std::fs::write(original_dir.join("bytes"), b"corrupt").expect("corrupt bytes");
		assert!(read_page_original(&state, chapter, 0, source, &content).await.is_none());
		let count = Arc::new(AtomicUsize::new(0));
		let fetch_count = count.clone();
		let repaired = page_original(&state, chapter, 0, source, &content, true, async move {
			fetch_count.fetch_add(1, Ordering::SeqCst);
			Ok((b"repaired".to_vec(), "image/png".to_string()))
		}).await.expect("repair serves bytes");
		assert_eq!(repaired.0, b"repaired");
		assert_eq!(count.load(Ordering::SeqCst), 1);
		assert_eq!(read_page_original(&state, chapter, 0, source, &content).await.expect("valid replacement").0, b"repaired");
		let page_bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM page WHERE chapter_id=?")
			.bind(chapter).fetch_one(&state.pool).await.expect("page bytes");
		let disk_bytes = chapter_disk_bytes_blocking(&state.config.page_blobs_dir(), &state.config.opds_cbz_dir(), chapter).expect("disk bytes");
		let accounted: i64 = sqlx::query_scalar("SELECT bytes FROM chapter_cache_state WHERE chapter_id=?")
			.bind(chapter).fetch_one(&state.pool).await.expect("accounted bytes");
		assert_eq!(accounted, page_bytes + disk_bytes, "corrupt-entry repair reconciles actual bytes without double charging");

		let blocked_chapter = cached_chapter(&state, "write-failure", 0, unix_now()).await;
		let blocked_source: i64 = sqlx::query_scalar("SELECT m.source_id FROM manga m JOIN chapter c ON c.manga_id=m.id WHERE c.id=?")
			.bind(blocked_chapter).fetch_one(&state.pool).await.expect("source id");
		let blocked_content = serde_json::json!({"Url": ["http://fixture.invalid/blocked.png", null]});
		let chapter_dir = state.config.page_blobs_dir().join(blocked_chapter.to_string());
		std::fs::create_dir_all(&chapter_dir).expect("chapter dir");
		std::fs::write(chapter_dir.join("originals"), b"not a directory").expect("block originals dir");
		let served = page_original(&state, blocked_chapter, 0, blocked_source, &blocked_content, true, async {
			Ok((b"still-served".to_vec(), "image/png".to_string()))
		}).await.expect("cache write failure does not fail image response");
		assert_eq!(served.0, b"still-served");
		assert!(read_page_original(&state, blocked_chapter, 0, blocked_source, &blocked_content).await.is_none());
	}

	#[tokio::test]
	async fn clear_removes_page_cache_but_preserves_chapter_state_and_download_artifact() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let db = tmp.path().join("cache.db");
		let pool = crate::db::create_pool(&format!("sqlite://{}?mode=rwc", db.display()))
			.await
			.expect("pool");
		let source: i64 = sqlx::query_scalar(
			"INSERT INTO source(key,name,wasm_path) VALUES ('source','Source','') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("source");
		let manga: i64 =
			sqlx::query_scalar("INSERT INTO manga(source_id,key) VALUES (?, 'manga') RETURNING id")
				.bind(source)
				.fetch_one(&pool)
				.await
				.expect("manga");
		let chapter: i64 = sqlx::query_scalar("INSERT INTO chapter(manga_id,key,page_count,pages_analyzed_at) VALUES (?, 'chapter',2,123) RETURNING id").bind(manga).fetch_one(&pool).await.expect("chapter");
		sqlx::query(
			"INSERT INTO page(chapter_id,idx,content) VALUES (?,0,'{\"content\":\"cache\"}')",
		)
		.bind(chapter)
		.execute(&pool)
		.await
		.expect("page");
		sqlx::query("INSERT INTO chapter_user(chapter_id,user_id,last_page_read,is_bookmarked,is_downloaded) VALUES (?,1,1,1,1)").bind(chapter).execute(&pool).await.expect("progress");
		sqlx::query("INSERT INTO download(chapter_id,user_id,status,format,path) VALUES (?,1,'finished','cbz','kept.cbz')").bind(chapter).execute(&pool).await.expect("download");
		let blobs = tmp.path().join("blobs");
		let opds = tmp.path().join("opds-cbz");
		std::fs::create_dir_all(blobs.join(chapter.to_string()).join("originals/1")).expect("blobs");
		std::fs::write(blobs.join(chapter.to_string()).join("0.png"), b"image").expect("image");
		std::fs::write(
			blobs.join(chapter.to_string()).join("originals/1/bytes"),
			b"original",
		)
		.expect("original");
		std::fs::create_dir_all(&opds).expect("opds");
		std::fs::write(opds.join(chapter.to_string()), b"archive cache").expect("archive");
		reconcile_chapter(&pool, &blobs, &opds, chapter)
			.await
			.expect("reconcile");
		let before = stats(&pool, &blobs, &opds, DEFAULT_MAX_BYTES)
			.await
			.expect("stats");
		assert!(before.used_bytes > 0);
		let (cleared, used, _) = clear(&pool, &blobs, &opds).await.expect("clear");
		assert_eq!(cleared, before.used_bytes);
		assert_eq!(used, 0);
		let (pages, page_count, analyzed, progress, downloaded, bookmark, status): (i64,i64,Option<i64>,i64,i64,i64,String) = sqlx::query_as(
			"SELECT (SELECT count(*) FROM page WHERE chapter_id=c.id), c.page_count,c.pages_analyzed_at,cu.last_page_read,cu.is_downloaded,cu.is_bookmarked,d.status FROM chapter c JOIN chapter_user cu ON cu.chapter_id=c.id JOIN download d ON d.chapter_id=c.id WHERE c.id=?"
		).bind(chapter).fetch_one(&pool).await.expect("retained records");
		assert_eq!(
			(
				pages,
				page_count,
				analyzed,
				progress,
				downloaded,
				bookmark,
				status.as_str()
			),
			(0, 2, None, 1, 1, 1, "finished")
		);
		assert!(!blobs.join(chapter.to_string()).exists());
		assert!(!opds.join(chapter.to_string()).exists());
	}

	#[tokio::test]
	async fn clear_removes_zero_byte_cache_assets_and_advances_revision() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let source: i64 = sqlx::query_scalar(
			"INSERT INTO source(key,name,wasm_path) VALUES ('marker-source','Source','') RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("source");
		let manga: i64 = sqlx::query_scalar(
			"INSERT INTO manga(source_id,key) VALUES (?, 'marker-manga') RETURNING id",
		)
		.bind(source)
		.fetch_one(&state.pool)
		.await
		.expect("manga");
		let chapter: i64 = sqlx::query_scalar(
			"INSERT INTO chapter(manga_id,key) VALUES (?, 'marker-chapter') RETURNING id",
		)
		.bind(manga)
		.fetch_one(&state.pool)
		.await
		.expect("chapter");
		touch(&state.pool, chapter).await.expect("touch cache");
		let blobs = state.config.page_blobs_dir();
		let opds = state.config.config_dir.join("opds-cbz");
		let marker = blobs
			.join(chapter.to_string())
			.join("originals/0/sha256");
		std::fs::create_dir_all(marker.parent().expect("marker parent"))
			.expect("asset dir");
		std::fs::write(&marker, []).expect("zero-byte cache asset");
		let before = chapter_revision(&state.pool, chapter).await.expect("before revision");
		assert!(marker.exists());

		clear(&state.pool, &blobs, &opds).await.expect("clear marker");

		assert!(!blobs.join(chapter.to_string()).exists());
		assert!(
			chapter_revision(&state.pool, chapter).await.expect("after revision") > before,
			"manual clear advances the revision for zero-byte cache assets"
		);
	}

	#[tokio::test]
	async fn manual_clear_waits_for_active_cache_read_lease() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let read = state.chapter_cache_gate.read().await;
		let (started_tx, started_rx) = tokio::sync::oneshot::channel();
		let writer_state = state.clone();
		let writer = tokio::spawn(async move {
			let _ = started_tx.send(());
			clear_state(&writer_state).await
		});
		started_rx.await.expect("writer started");
		tokio::task::yield_now().await;
		assert!(
			!writer.is_finished(),
			"clear must wait on the actual AppState read lease"
		);
		drop(read);
		tokio::time::timeout(std::time::Duration::from_secs(1), writer)
			.await
			.expect("writer unblocked")
			.expect("join")
			.expect("clear");
	}

	#[tokio::test]
	async fn lru_evicts_oldest_and_skips_a_recent_active_chapter() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let old = cached_chapter(&state, "old", 10, unix_now() - 900).await;
		let active = cached_chapter(&state, "active", 20, unix_now() - 800).await;
		let active_lock = state.chapter_cache_lock(active).await;
		let _active = active_lock.read().await;
		enforce_limit_with_budget(&state, 20)
			.await
			.expect("evict");
		let old_pages: i64 = sqlx::query_scalar("SELECT count(*) FROM page WHERE chapter_id=?")
			.bind(old)
			.fetch_one(&state.pool)
			.await
			.expect("old pages");
		let active_pages: i64 = sqlx::query_scalar("SELECT count(*) FROM page WHERE chapter_id=?")
			.bind(active)
			.fetch_one(&state.pool)
			.await
			.expect("active pages");
		assert_eq!(old_pages, 0, "least recently used chapter is evicted");
		assert_eq!(active_pages, 1, "active chapter is skipped");
	}

	#[tokio::test]
	async fn lru_touch_and_recent_grace_are_respected_at_zero_budget() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let touched = cached_chapter(&state, "touched", 10, unix_now() - 900).await;
		let older = cached_chapter(&state, "older", 10, unix_now() - 800).await;
		touch(&state.pool, touched).await.expect("touch");
		enforce_limit_with_budget(&state, 10)
			.await
			.expect("evict");
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
				.bind(touched)
				.fetch_one(&state.pool)
				.await
				.expect("touched row"),
			1
		);
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
				.bind(older)
				.fetch_one(&state.pool)
				.await
				.expect("older row"),
			0
		);

		let recent_a = cached_chapter(&state, "recent-a", 10, unix_now()).await;
		let recent_b = cached_chapter(&state, "recent-b", 10, unix_now()).await;
		enforce_limit_with_budget(&state, 0)
			.await
			.expect("recent grace");
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id IN (?,?)")
				.bind(recent_a)
				.bind(recent_b)
				.fetch_one(&state.pool)
				.await
				.expect("recent rows"),
			2
		);
	}

	#[tokio::test]
	async fn reconciliation_counts_unicode_page_json_blobs_and_opds_and_removes_orphans() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let source: i64 = sqlx::query_scalar(
			"INSERT INTO source(key,name,wasm_path) VALUES ('unicode-source','Source','') RETURNING id",
		)
		.fetch_one(&state.pool)
		.await
		.expect("source");
		let manga: i64 = sqlx::query_scalar(
			"INSERT INTO manga(source_id,key) VALUES (?, 'unicode-manga') RETURNING id",
		)
		.bind(source)
		.fetch_one(&state.pool)
		.await
		.expect("manga");
		let chapter: i64 = sqlx::query_scalar(
			"INSERT INTO chapter(manga_id,key,page_count) VALUES (?, 'unicode',1) RETURNING id",
		)
		.bind(manga)
		.fetch_one(&state.pool)
		.await
		.expect("chapter");
		let content = "{\"content\":\"漫画\"}";
		sqlx::query("INSERT INTO page(chapter_id,idx,content) VALUES (?,0,?)")
			.bind(chapter)
			.bind(content)
			.execute(&state.pool)
			.await
			.expect("page");
		let blobs = state.config.page_blobs_dir();
		let opds = state.config.config_dir.join("opds-cbz");
		std::fs::create_dir_all(blobs.join(chapter.to_string()).join("originals/0"))
			.expect("blob tree");
		std::fs::write(blobs.join(chapter.to_string()).join("0.png"), b"original")
			.expect("original");
		std::fs::write(
			blobs.join(chapter.to_string()).join("originals/0/bytes"),
			b"derived",
		)
		.expect("derived");
		std::fs::create_dir_all(&opds).expect("opds");
		std::fs::write(opds.join(chapter.to_string()), b"cbz").expect("cbz");
		std::fs::create_dir_all(blobs.join("999999")).expect("orphan");
		std::fs::write(blobs.join("999999/0.png"), b"orphan").expect("orphan bytes");
		reconcile_all(&state.pool, &blobs, &opds)
			.await
			.expect("startup reconciliation");
		let expected = content.len() as i64
			+ b"original".len() as i64
			+ b"derived".len() as i64
			+ b"cbz".len() as i64;
		let stats = stats(&state.pool, &blobs, &opds, DEFAULT_MAX_BYTES)
			.await
			.expect("stats");
		assert_eq!(stats.used_bytes, expected);
		assert_eq!(stats.chapter_count, 1);
		assert!(!blobs.join("999999").exists());
	}

	#[tokio::test]
	async fn partial_clear_invalidates_rows_reports_actual_usage_and_can_retry() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let chapter = cached_chapter(&state, "retry", 10, unix_now()).await;
		let opds = state.config.config_dir.join("opds-cbz");
		std::fs::create_dir_all(opds.join(chapter.to_string())).expect("invalid cache dir");
		std::fs::write(opds.join(chapter.to_string()).join("archive"), b"remain")
			.expect("remaining data");
		let target = opds.join(chapter.to_string());
		let result = clear_inner(
			&state.pool,
			&state.config.page_blobs_dir(),
			&opds,
			Some(&target),
			vec![chapter],
		)
		.await;
		assert!(
			result.is_err(),
			"directory where an archive file belongs surfaces an error"
		);
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
				.bind(chapter)
				.fetch_one(&state.pool)
				.await
				.expect("rows"),
			0
		);
		let partial_stats = stats(
			&state.pool,
			&state.config.page_blobs_dir(),
			&opds,
			DEFAULT_MAX_BYTES,
		)
		.await
		.expect("actual stats");
		assert_eq!(partial_stats.used_bytes, b"remain".len() as i64);
		assert!(partial_stats.last_error.is_some());
		std::fs::remove_dir_all(opds.join(chapter.to_string())).expect("repair cache path");
		let (_, used, _) = clear(&state.pool, &state.config.page_blobs_dir(), &opds)
			.await
			.expect("retry clear");
		assert_eq!(used, 0);
		assert_eq!(
			stats(
				&state.pool,
				&state.config.page_blobs_dir(),
				&opds,
				DEFAULT_MAX_BYTES
			)
			.await
			.expect("cleared stats")
			.last_error,
			None
		);
	}
	#[tokio::test]
	async fn zero_budget_collects_idle_working_data_despite_legacy_disabled_switch() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		sqlx::query("INSERT INTO setting(key,value) VALUES ('cache.chapterMaxBytes','0'),('cache.chapterLruEnabled','false')")
			.execute(&state.pool).await.expect("zero budget with legacy false");
		let idle = cached_chapter(&state, "idle-zero", 10, unix_now() - 900).await;
		let recent = cached_chapter(&state, "recent-zero", 10, unix_now()).await;
		let active = cached_chapter(&state, "active-zero", 10, unix_now() - 900).await;
		let active_lock = state.chapter_cache_lock(active).await;
		let lease = active_lock.read().await;
		let opds = state.config.opds_cbz_dir();
		std::fs::create_dir_all(&opds).expect("opds dir");
		std::fs::write(opds.join(idle.to_string()), b"archive").expect("working archive");
		let product = state.config.downloads_dir.join("kept.cbz");
		assert!(product.starts_with(tmp.path()), "test artifacts must stay under their TempDir");
		std::fs::create_dir_all(&state.config.downloads_dir).expect("downloads dir");
		std::fs::write(&product, b"finished product").expect("finished product");
		sqlx::query("INSERT INTO download(chapter_id,user_id,status,format,path) VALUES (?,1,'finished','cbz','kept.cbz')")
			.bind(idle).execute(&state.pool).await.expect("finished row");
		enforce_limit(&state).await.expect("zero budget collector");
		let pages: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT chapter_id FROM page ORDER BY chapter_id")
			.fetch_all(&state.pool).await.expect("remaining pages");
		assert!(!pages.contains(&idle), "zero budget must evict idle page records");
		assert!(pages.contains(&recent), "recent working data retains its grace period");
		assert!(pages.contains(&active), "an active operation retains its working data");
		assert!(!opds.join(idle.to_string()).exists(), "idle OPDS working archives are reclaimed");
		assert!(product.exists(), "finished downloads are outside the cache budget");
		drop(lease);
		enforce_limit(&state).await.expect("collect released chapter");
		assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id=?")
			.bind(active).fetch_one(&state.pool).await.expect("active pages"), 0);
	}

}
