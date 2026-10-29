//! Persistent accounting and clearing for derived chapter page caches.

use crate::error::ApiResult;
use crate::model::unix_now;
use serde::Serialize;
use sqlx::SqlitePool;
use std::path::Path;

pub const DEFAULT_MAX_BYTES: i64 = 2 * 1024 * 1024 * 1024;
pub const MIN_MAX_BYTES: i64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterCacheStats {
	pub enabled: bool,
	pub max_bytes: i64,
	pub used_bytes: i64,
	pub chapter_count: i64,
	pub last_error: Option<String>,
	pub cache_revision: i64,
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

pub async fn publish_chapter(
	pool: &SqlitePool,
	blobs: &Path,
	opds: &Path,
	chapter_id: i64,
) -> ApiResult<()> {
	let pages: i64 = sqlx::query_scalar(
		"SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM page WHERE chapter_id=?",
	)
	.bind(chapter_id)
	.fetch_one(pool)
	.await?;
	let disk_bytes =
		chapter_disk_bytes(blobs.to_path_buf(), opds.to_path_buf(), chapter_id).await?;
	let mut tx = crate::db::begin_write(pool).await?;
	let revision: i64 = sqlx::query_scalar(
		"UPDATE chapter_cache_meta SET revision=revision+1 WHERE id=1 RETURNING revision",
	)
	.fetch_one(&mut *tx)
	.await?;
	let bytes = pages.saturating_add(disk_bytes);
	sqlx::query("INSERT INTO chapter_cache_state(chapter_id,bytes,accessed_at,revision) VALUES (?,?,?,?) ON CONFLICT(chapter_id) DO UPDATE SET bytes=excluded.bytes, accessed_at=excluded.accessed_at, revision=excluded.revision, last_error=NULL")
        .bind(chapter_id).bind(bytes).bind(unix_now()).bind(revision).execute(&mut *tx).await?;
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

fn chapter_disk_bytes_blocking(blobs: &Path, opds: &Path, id: i64) -> std::io::Result<i64> {
	let blob_bytes = tree_bytes(&blobs.join(id.to_string()))?;
	let path = opds.join(id.to_string());
	let archive_bytes = match std::fs::symlink_metadata(&path) {
		Ok(meta) if meta.file_type().is_symlink() => 0,
		Ok(meta) if meta.is_dir() => tree_bytes(&path)?,
		Ok(meta) => meta.len().min(i64::MAX as u64) as i64,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
		Err(error) => return Err(error),
	};
	Ok(blob_bytes.saturating_add(archive_bytes))
}

async fn chapter_disk_bytes(
	blobs: std::path::PathBuf,
	opds: std::path::PathBuf,
	id: i64,
) -> ApiResult<i64> {
	tokio::task::spawn_blocking(move || chapter_disk_bytes_blocking(&blobs, &opds, id))
		.await
		.map_err(|error| {
			crate::error::ApiError::internal(format!("chapter cache scan failed: {error}"))
		})?
		.map_err(|error| {
			crate::error::ApiError::internal(format!("chapter cache scan failed: {error}"))
		})
}

/// Reconcile indexed cache sizes at startup and before management responses. The durable rows
/// retain LRU order and revisions; file sizes are refreshed to account for partial writes/orphans.
pub async fn settings(pool: &SqlitePool) -> ApiResult<(bool, i64)> {
	let rows: Vec<(String, String)> = sqlx::query_as(
		"SELECT key, value FROM setting WHERE key IN ('cache.chapterLruEnabled','cache.chapterMaxBytes')",
	)
	.fetch_all(pool)
	.await?;
	let mut enabled = true;
	let mut max_bytes = DEFAULT_MAX_BYTES;
	for (key, raw) in rows {
		if key == "cache.chapterLruEnabled" {
			enabled = serde_json::from_str(&raw).unwrap_or(true);
		}
		if key == "cache.chapterMaxBytes" {
			max_bytes = serde_json::from_str::<i64>(&raw)
				.unwrap_or(DEFAULT_MAX_BYTES)
				.max(MIN_MAX_BYTES);
		}
	}
	Ok((enabled, max_bytes))
}

pub async fn touch(pool: &SqlitePool, chapter_id: i64) -> ApiResult<()> {
	let _ = sqlx::query("INSERT INTO chapter_cache_state(chapter_id, bytes, accessed_at) VALUES (?, 0, ?) ON CONFLICT(chapter_id) DO UPDATE SET accessed_at=excluded.accessed_at")
		.bind(chapter_id).bind(unix_now()).execute(pool).await?;
	Ok(())
}

pub async fn write_variant(
	state: &std::sync::Arc<crate::state::AppState>,
	chapter_id: i64,
	index: i64,
	bucket: u32,
	png: Option<&[u8]>,
) -> ApiResult<()> {
	let lock = state.chapter_variant_lock(chapter_id, index, bucket).await;
	let _write = lock.lock().await;
	let blobs = state.config.page_blobs_dir();
	let paths = crate::source::downsample::variant_paths(&blobs, chapter_id, index, bucket);
	let mut before = 0i64;
	for path in &paths {
		if let Ok(meta) = tokio::fs::symlink_metadata(path).await
			&& meta.file_type().is_file()
		{
			before = before.saturating_add(meta.len().min(i64::MAX as u64) as i64);
		}
	}
	crate::source::downsample::write_variant(&blobs, chapter_id, index, bucket, png)
		.await
		.map_err(|error| {
			crate::error::ApiError::internal(format!("write page variant failed: {error}"))
		})?;
	let mut after = 0i64;
	for path in &paths {
		if let Ok(meta) = tokio::fs::symlink_metadata(path).await
			&& meta.file_type().is_file()
		{
			after = after.saturating_add(meta.len().min(i64::MAX as u64) as i64);
		}
	}
	let _ = sqlx::query(
		"UPDATE chapter_cache_state SET bytes=max(0,bytes+?),accessed_at=? WHERE chapter_id=?",
	)
	.bind(after - before)
	.bind(unix_now())
	.bind(chapter_id)
	.execute(&state.pool)
	.await?;
	Ok(())
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
	let bytes = pages
		.saturating_add(chapter_disk_bytes(blobs.to_path_buf(), opds.to_path_buf(), id).await?);
	let _ = sqlx::query("INSERT INTO chapter_cache_state(chapter_id, bytes, accessed_at) VALUES (?, ?, ?) ON CONFLICT(chapter_id) DO UPDATE SET bytes=excluded.bytes")
		.bind(id).bind(bytes).bind(unix_now()).execute(pool).await?;
	Ok(())
}

pub async fn stats(
	pool: &SqlitePool,
	_blobs: &Path,
	_opds: &Path,
	enabled: bool,
	max_bytes: i64,
) -> ApiResult<ChapterCacheStats> {
	let (used, count, last_error): (i64, i64, Option<String>) = sqlx::query_as(
		"SELECT coalesce(sum(bytes),0), coalesce(sum(bytes > 0),0), max(last_error) FROM chapter_cache_state"
	).fetch_one(pool).await?;
	let revision: i64 = sqlx::query_scalar("SELECT revision FROM chapter_cache_meta WHERE id=1")
		.fetch_one(pool)
		.await?;
	Ok(ChapterCacheStats {
		enabled,
		max_bytes: max_bytes.max(MIN_MAX_BYTES),
		used_bytes: used,
		chapter_count: count,
		last_error,
		cache_revision: revision,
	})
}

/// Startup reconciliation charges historical page rows, blob variants and OPDS archives, then
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
	clear_inner(pool, blobs, opds, None).await
}

async fn clear_inner(
	pool: &SqlitePool,
	blobs: &Path,
	opds: &Path,
	fail_on: Option<&Path>,
) -> ApiResult<(i64, i64, i64)> {
	let current = stats(pool, blobs, opds, true, DEFAULT_MAX_BYTES).await?;
	let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM chapter WHERE pages_analyzed_at IS NOT NULL UNION SELECT chapter_id FROM chapter_cache_state WHERE bytes > 0 OR last_error IS NOT NULL UNION SELECT chapter_id FROM page")
            .fetch_all(pool)
            .await?;
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
		sqlx::query("INSERT INTO chapter_cache_state(chapter_id,bytes,accessed_at,revision,last_error) VALUES (?,0,?,?,NULL) ON CONFLICT(chapter_id) DO UPDATE SET bytes=0, revision=excluded.revision, accessed_at=excluded.accessed_at, last_error=NULL")
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
	let after = stats(pool, blobs, opds, true, DEFAULT_MAX_BYTES).await?;
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

/// Enforce the configured soft limit oldest-first. The global read lease excludes a manual clear;
/// a per-chapter write lease skips only assets currently used by a reader or download.
pub async fn enforce_limit(state: &std::sync::Arc<crate::state::AppState>) -> ApiResult<()> {
	let (enabled, max_bytes) = settings(&state.pool).await?;
	enforce_limit_with_budget(state, enabled, max_bytes).await
}

async fn enforce_limit_with_budget(
	state: &std::sync::Arc<crate::state::AppState>,
	enabled: bool,
	max_bytes: i64,
) -> ApiResult<()> {
	if !enabled {
		return Ok(());
	}
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
			"UPDATE chapter_cache_state SET bytes=0, revision=?, last_error=? WHERE chapter_id=?",
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

	async fn app_state(tmp: &tempfile::TempDir) -> std::sync::Arc<crate::state::AppState> {
		let config = crate::config::Config {
			config_dir: tmp.path().to_path_buf(),
			database_url: format!("sqlite://{}/gate.db?mode=rwc", tmp.path().display()),
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
		std::fs::create_dir_all(blobs.join(chapter.to_string()).join("variants")).expect("blobs");
		std::fs::write(blobs.join(chapter.to_string()).join("0.png"), b"image").expect("image");
		std::fs::write(
			blobs.join(chapter.to_string()).join("variants/1.png"),
			b"variant",
		)
		.expect("variant");
		std::fs::create_dir_all(&opds).expect("opds");
		std::fs::write(opds.join(chapter.to_string()), b"archive cache").expect("archive");
		reconcile_chapter(&pool, &blobs, &opds, chapter)
			.await
			.expect("reconcile");
		let before = stats(&pool, &blobs, &opds, true, DEFAULT_MAX_BYTES)
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
		enforce_limit_with_budget(&state, true, 20)
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
	async fn lru_touch_disabled_setting_and_recent_grace_are_respected() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let state = app_state(&tmp).await;
		let touched = cached_chapter(&state, "touched", 10, unix_now() - 900).await;
		let older = cached_chapter(&state, "older", 10, unix_now() - 800).await;
		touch(&state.pool, touched).await.expect("touch");
		enforce_limit_with_budget(&state, false, 0)
			.await
			.expect("disabled");
		assert_eq!(
			sqlx::query_scalar::<_, i64>("SELECT count(*) FROM page WHERE chapter_id IN (?,?)")
				.bind(touched)
				.bind(older)
				.fetch_one(&state.pool)
				.await
				.expect("rows"),
			2
		);
		enforce_limit_with_budget(&state, true, 10)
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
		enforce_limit_with_budget(&state, true, 0)
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
	async fn reconciliation_counts_unicode_page_json_blobs_variants_and_opds_and_removes_orphans() {
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
		std::fs::create_dir_all(blobs.join(chapter.to_string()).join("variants/512"))
			.expect("blob tree");
		std::fs::write(blobs.join(chapter.to_string()).join("0.png"), b"original")
			.expect("original");
		std::fs::write(
			blobs.join(chapter.to_string()).join("variants/512/0.png"),
			b"variant",
		)
		.expect("variant");
		std::fs::create_dir_all(&opds).expect("opds");
		std::fs::write(opds.join(chapter.to_string()), b"cbz").expect("cbz");
		std::fs::create_dir_all(blobs.join("999999")).expect("orphan");
		std::fs::write(blobs.join("999999/0.png"), b"orphan").expect("orphan bytes");
		reconcile_all(&state.pool, &blobs, &opds)
			.await
			.expect("startup reconciliation");
		let expected = content.as_bytes().len() as i64
			+ b"original".len() as i64
			+ b"variant".len() as i64
			+ b"cbz".len() as i64;
		let stats = stats(&state.pool, &blobs, &opds, true, DEFAULT_MAX_BYTES)
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
			true,
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
				true,
				DEFAULT_MAX_BYTES
			)
			.await
			.expect("cleared stats")
			.last_error,
			None
		);
	}
}
