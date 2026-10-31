use crate::api::{RefreshQuery, wants_refresh};
use crate::error::{ApiError, ApiResult};
use crate::model::manga_from_row;
use crate::source::{fetch_cover_image, manga_row};
use crate::state::AppState;
use axum::Json;
use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

const DEFAULT_MIME: &str = "image/jpeg";
const COVER_MAX_AGE: u64 = 86400;
/// A `?v=` cover URL changes whenever the manga's cover URL does, so the browser may keep it far longer
const VERSIONED_COVER_MAX_AGE: u64 = 30 * 86400;
/// Proxied source images are addressed by their own URL, which a source changes when the image changes
const SOURCE_IMAGE_MAX_AGE: u64 = 7 * 86400;

/// Local-source covers are user-owned files at original resolution (a cover.* sidecar or a chapter's first
/// page), far larger than any display slot; cap the longest edge — the cached artifact is the downscaled JPEG
pub const LOCAL_COVER_MAX_EDGE: u32 = 600;
/// Freshness alone cannot prove cached bytes came through the local downsample pipeline.
const LOCAL_COVER_CACHE_VARIANT: &str = "local-longest-edge-600-v1";

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
	pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct CoverQuery {
	pub refresh: Option<String>,
	/// Cache-busting version the web UI appends (the manga's cover URL); the server only reads its presence
	pub v: Option<String>,
}

/// A source image host's hotlink protection (Referer etc.) can only be satisfied by the server per the source's
/// declaration, so a direct <img> always gets 403; this takes the same fetch_cover_image as the details-page cover:
/// a source-declared cover processor returns processed bytes (an encrypted image host is otherwise ciphertext the
/// browser cannot decode). Not cached — browse/home covers are numerous and rarely reused
pub async fn get_source_image(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<ImageQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let parsed = reqwest::Url::parse(&query.url).map_err(|_| ApiError::bad_request("invalid image url"))?;
	if !matches!(parsed.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("unsupported image url scheme"));
	}
	let (bytes, mime) = fetch_cover_image(&state, id, &query.url).await?;
	let mime = mime.unwrap_or_else(|| DEFAULT_MIME.into());
	Ok(validated_response(&headers, bytes, &mime, SOURCE_IMAGE_MAX_AGE))
}

pub async fn get_cover(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<CoverQuery>,
	headers: HeaderMap,
) -> ApiResult<Response> {
	let manga = manga_row(&state.pool, id).await?;
	let is_local = crate::source::source_is_local(&state.pool, manga.source_id).await?;
	// A local cover can change on disk without its cover field changing, so its version marker proves nothing
	let max_age = if query.v.is_some() && !is_local { VERSIONED_COVER_MAX_AGE } else { COVER_MAX_AGE };
	// A matched cover (tracker or manual) replaces the disk cover of a local entry, and its bytes never went
	// through the local downsample pipeline, so it must not carry the local cache marker either
	let external = is_local && external_cover_lock(&state, manga.id, manga.source_id).await?;
	let cache_variant = (is_local && !external).then_some(LOCAL_COVER_CACHE_VARIANT);
	let covers_dir = state.config.covers_dir.clone();
	let cache_path = covers_dir.join(id.to_string());
	if !wants_refresh(&RefreshQuery { refresh: query.refresh.clone() })
		&& let Some((bytes, mime)) = read_fresh_cache(&cache_path, manga.last_fetched_at, cache_variant).await
	{
		return Ok(validated_response(&headers, bytes, &mime, max_age));
	}
	let (bytes, mime) = if is_local && !external {
		let (bytes, mime) = crate::internal::local::resolve_cover(
			&state.local_dirs(manga.source_id).await?,
			&state.local_zip,
			&manga.key,
			&state.local_exclusions(manga.source_id).await?,
		)
		.await?
		.ok_or_else(|| ApiError::not_found("cover not available"))?;
		// Decode+resize is CPU-heavy work, moved to a blocking thread; an image
		// within the cap or an undecodable image passes through at its original bytes
		let (resized, original) = tokio::task::spawn_blocking(move || {
			let resized = crate::source::downsample::downsample_jpeg(&bytes, LOCAL_COVER_MAX_EDGE);
			(resized, bytes)
		})
		.await
		.map_err(|e| ApiError::internal(format!("cover downsample task failed: {e}")))?;
		match resized {
			Some(jpeg) => (jpeg, "image/jpeg".into()),
			None => (original, mime),
		}
	} else {
		let Some(url) = manga.cover_url.clone() else {
			return Err(ApiError::not_found("cover not available"));
		};
		// A cover the user matched from a tracker or typed in by hand may live on a host the owning source never
		// described, so it is fetched with a bare GET instead of the source's image_request/cover processor chain
		let bypass = if is_local {
			external
		} else {
			external_cover_lock(&state, manga.id, manga.source_id).await?
		};
		let (bytes, mime) = if bypass {
			crate::source::fetch_direct_image(&state, &url).await?
		} else {
			fetch_cover_image(&state, manga.source_id, &url).await?
		};
		(bytes, mime.unwrap_or_else(|| DEFAULT_MIME.into()))
	};
	match write_cache(&covers_dir, id, &bytes, &mime, cache_variant).await {
		Ok(()) => enforce_cache_limit(&state.pool, &covers_dir, state.config.cover_cache_max).await,
		Err(e) => tracing::warn!(manga_id = id, "cover cache write failed: {e}"),
	}
	Ok(validated_response(&headers, bytes, &mime, max_age))
}

/// Whether the cover column is owned by someone other than the entry's own
/// source (a tracker match or a manual edit), i.e. the URL did not come from a
/// source detail fetch and must bypass that source's image rules
async fn external_cover_lock(state: &AppState, manga_id: i64, source_id: i64) -> ApiResult<bool> {
	let mut conn = state.pool.acquire().await?;
	let locks = crate::metadata::load(&mut conn, manga_id).await?;
	Ok(locks
		.get(crate::metadata::Field::Cover.as_str())
		.is_some_and(|lock| lock.provider != crate::metadata::source_reference(source_id)))
}

/// Strong validator over the served bytes, so an expired browser entry revalidates with a 304 instead of a
/// full download; a cover served from a fresh fetch matches as long as the source returned the same bytes
fn etag(bytes: &[u8]) -> String {
	use sha2::Digest;
	let digest = sha2::Sha256::digest(bytes);
	let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
	format!("\"{hex}\"")
}

fn if_none_match_hits(headers: &HeaderMap, tag: &str) -> bool {
	headers.get_all(header::IF_NONE_MATCH).iter().filter_map(|value| value.to_str().ok()).flat_map(|value| value.split(',')).any(|candidate| {
		let candidate = candidate.trim();
		candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == tag
	})
}

fn validated_response(headers: &HeaderMap, bytes: Vec<u8>, mime: &str, max_age: u64) -> Response {
	let tag = etag(&bytes);
	if if_none_match_hits(headers, &tag) {
		return Response::builder()
			.status(StatusCode::NOT_MODIFIED)
			.header(header::ETAG, &tag)
			.header(header::CACHE_CONTROL, format!("private, max-age={max_age}"))
			.body(Body::empty())
			.expect("not-modified response");
	}
	let mut response = bytes_response(bytes, mime, max_age);
	response.headers_mut().insert(header::ETAG, tag.parse().expect("hex etag is a valid header value"));
	response
}

pub(crate) fn bytes_response(bytes: Vec<u8>, mime: &str, max_age: u64) -> Response {
	Response::builder()
		.status(StatusCode::OK)
		.header(header::CONTENT_TYPE, mime)
		.header(header::CACHE_CONTROL, format!("private, max-age={max_age}"))
		.body(Body::from(bytes))
		.expect("bytes response")
}

/// Alternate-cover list: calls the source export with the library manga as input; a source without the export gets the customary 501
pub async fn get_alternate_covers(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
	let manga = manga_row(&state.pool, id).await?;
	let handle = state.get_handle(manga.source_id).await?;
	if !handle.features().alternate_covers {
		return Err(ApiError::unsupported_501("source does not support alternate covers"));
	}
	let covers = handle.get_alternate_covers(manga_from_row(&manga)).await?;
	Ok(Json(json!(covers)))
}

#[derive(Debug, Deserialize)]
pub struct SelectCoverRequest {
	pub url: String,
}

/// Selects a cover: only writes cover_url and invalidates the disk cache. last_fetched_at is untouched (that is
/// source-refresh semantics) — the cache judges freshness by it, and without clearing the old bytes the old cover would keep being served after the switch, so the selection would never take effect
pub async fn select_cover(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Json(body): Json<SelectCoverRequest>,
) -> ApiResult<Json<Value>> {
	manga_row(&state.pool, id).await?;
	let parsed = reqwest::Url::parse(&body.url).map_err(|_| ApiError::bad_request("invalid cover url"))?;
	if !matches!(parsed.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("unsupported cover url scheme"));
	}
	let mut write = crate::db::begin_write(&state.pool).await?;
	crate::store::manga::set_cover_url(&mut write, id, &body.url).await?;
	write.commit().await?;
	invalidate_cache(&state.config.covers_dir, id).await;
	let row = manga_row(&state.pool, id).await?;
	Ok(Json(crate::api::manga_json(&state, user.id, &row).await?))
}

/// Deletes the body and its sidecars together; missing files are not an error when nothing was cached yet
pub(crate) async fn invalidate_cache(covers_dir: &FsPath, id: i64) {
	let cache_path = covers_dir.join(id.to_string());
	let _ = tokio::fs::remove_file(sidecar_path(&cache_path)).await;
	let _ = tokio::fs::remove_file(variant_path(&cache_path)).await;
	let _ = tokio::fs::remove_file(cache_path).await;
}

/// Purge paths (local trash) drop the derived cover cache with the row
pub(crate) async fn invalidate_cover(covers_dir: &FsPath, id: i64) {
	invalidate_cache(covers_dir, id).await;
}

fn sidecar_path(cache_path: &FsPath) -> PathBuf {
	cache_path.with_file_name(format!("{}.content-type", cache_path.file_name().unwrap_or_default().to_string_lossy()))
}

fn variant_path(cache_path: &FsPath) -> PathBuf {
	cache_path.with_file_name(format!("{}.variant", cache_path.file_name().unwrap_or_default().to_string_lossy()))
}

/// Cache-hit condition: the file exists and the manga details were not re-fetched after the cache write (last_fetched_at <= mtime)
async fn read_fresh_cache(
	cache_path: &FsPath,
	last_fetched_at: Option<i64>,
	required_variant: Option<&str>,
) -> Option<(Vec<u8>, String)> {
	let metadata = tokio::fs::metadata(cache_path).await.ok()?;
	let mtime = metadata
		.modified()
		.ok()?
		.duration_since(std::time::UNIX_EPOCH)
		.ok()?
		.as_secs() as i64;
	if let Some(fetched) = last_fetched_at
		&& fetched > mtime
	{
		return None;
	}
	if let Some(expected) = required_variant {
		let actual = tokio::fs::read_to_string(variant_path(cache_path)).await.ok()?;
		if actual.trim() != expected {
			return None;
		}
	}
	let bytes = tokio::fs::read(cache_path).await.ok()?;
	// The eviction order is mtime, so a hit moves the cover to the young end; the new mtime is still
	// at or after last_fetched_at, so freshness is unchanged
	touch(cache_path).await;
	let mime = tokio::fs::read_to_string(sidecar_path(cache_path))
		.await
		.ok()
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| DEFAULT_MIME.into());
	Some((bytes, mime))
}

/// Writes tmp first then renames, so a concurrent read never sees a half-written cache
async fn write_cache(
	covers_dir: &FsPath,
	id: i64,
	bytes: &[u8],
	mime: &str,
	variant: Option<&str>,
) -> std::io::Result<()> {
	let tmp = covers_dir.join(format!(".{id}.tmp"));
	tokio::fs::write(&tmp, bytes).await?;
	tokio::fs::rename(&tmp, covers_dir.join(id.to_string())).await?;
	let tmp = covers_dir.join(format!(".{id}.content-type.tmp"));
	tokio::fs::write(&tmp, mime).await?;
	tokio::fs::rename(&tmp, covers_dir.join(format!("{id}.content-type"))).await?;
	if let Some(variant) = variant {
		let tmp = covers_dir.join(format!(".{id}.variant.tmp"));
		tokio::fs::write(&tmp, variant).await?;
		tokio::fs::rename(&tmp, covers_dir.join(format!("{id}.variant"))).await?;
	}
	Ok(())
}

/// LRU over mtime (a cache hit touches it): when the cache file count exceeds the cap, delete the least recently
/// used first (sidecars included). Library covers are what the library page shows on every visit, so covers of
/// manga outside the library go first and a library cover is evicted only when the library alone exceeds the cap
async fn enforce_cache_limit(pool: &sqlx::SqlitePool, covers_dir: &FsPath, max: usize) {
	let library: std::collections::HashSet<i64> = match sqlx::query_scalar::<_, i64>("SELECT DISTINCT manga_id FROM manga_user WHERE in_library = 1")
		.fetch_all(pool)
		.await
	{
		Ok(ids) => ids.into_iter().collect(),
		Err(e) => {
			tracing::warn!("cover cache eviction skipped, library ids unreadable: {e}");
			return;
		}
	};
	let covers_dir = covers_dir.to_path_buf();
	let _ = tokio::task::spawn_blocking(move || evict_over_cap(&covers_dir, max, &library)).await;
}

fn evict_over_cap(covers_dir: &FsPath, max: usize, library: &std::collections::HashSet<i64>) {
	let mut entries: Vec<(PathBuf, bool, std::time::SystemTime)> = Vec::new();
	let Ok(read_dir) = std::fs::read_dir(covers_dir) else {
		return;
	};
	for entry in read_dir.flatten() {
		let name = entry.file_name();
		// Only count the cover bodies (file name is a bare manga id); sidecars and tmp files are handled along with their body
		let Some(id) = name.to_str().and_then(|n| n.parse::<i64>().ok()) else {
			continue;
		};
		let Ok(metadata) = entry.metadata() else {
			continue;
		};
		let Ok(mtime) = metadata.modified() else {
			continue;
		};
		entries.push((entry.path(), library.contains(&id), mtime));
	}
	if entries.len() <= max {
		return;
	}
	// false sorts first: non-library covers are evicted before any library cover, each group oldest first
	entries.sort_by_key(|(_, in_library, mtime)| (*in_library, *mtime));
	let excess = entries.len() - max;
	for (path, _, _) in entries.into_iter().take(excess) {
		let _ = std::fs::remove_file(sidecar_path(&path));
		let _ = std::fs::remove_file(variant_path(&path));
		let _ = std::fs::remove_file(path);
	}
}

/// Best effort: a cover that cannot be touched only loses its recency, never its bytes
async fn touch(path: &FsPath) {
	let path = path.to_path_buf();
	let _ = tokio::task::spawn_blocking(move || {
		std::fs::File::options().write(true).open(&path).and_then(|file| file.set_modified(std::time::SystemTime::now()))
	})
	.await;
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverCacheStats {
	pub entries: i64,
	pub used_bytes: i64,
	pub max_entries: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverCacheClearResponse {
	pub cleared_bytes: i64,
	pub cleared_entries: i64,
}

/// Disk usage of the cover cache: entries are cover bodies (bare numeric file names);
/// bytes count every regular file, sidecars included, so the number matches real usage.
fn cover_cache_disk_stats(dir: &FsPath) -> std::io::Result<(i64, i64)> {
	let read = match std::fs::read_dir(dir) {
		Ok(read) => read,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
		Err(error) => return Err(error),
	};
	let mut entries = 0i64;
	let mut bytes = 0i64;
	for entry in read {
		let entry = entry?;
		if !entry.file_type()?.is_file() {
			continue;
		}
		bytes = bytes.saturating_add(entry.metadata()?.len().min(i64::MAX as u64) as i64);
		if entry.file_name().to_str().and_then(|name| name.parse::<i64>().ok()).is_some() {
			entries += 1;
		}
	}
	Ok((entries, bytes))
}

/// Removes every cached cover (bodies, sidecars, stale tmp files); manga rows are untouched,
/// so a cover simply re-fetches on its next view. One removal failure does not abort the sweep.
fn clear_cover_cache_dir(dir: &FsPath) -> std::io::Result<(i64, i64)> {
	let read = match std::fs::read_dir(dir) {
		Ok(read) => read,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
		Err(error) => return Err(error),
	};
	let mut entries = 0i64;
	let mut bytes = 0i64;
	let mut first_error: Option<std::io::Error> = None;
	for entry in read {
		let entry = entry?;
		if entry.file_type()?.is_dir() {
			continue;
		}
		let size = entry.metadata()?.len().min(i64::MAX as u64) as i64;
		match std::fs::remove_file(entry.path()) {
			Ok(()) => {
				bytes = bytes.saturating_add(size);
				if entry.file_name().to_str().and_then(|name| name.parse::<i64>().ok()).is_some() {
					entries += 1;
				}
			}
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
			Err(error) => {
				if first_error.is_none() {
					first_error = Some(error);
				}
			}
		}
	}
	if let Some(error) = first_error {
		return Err(error);
	}
	Ok((entries, bytes))
}

/// Cover cache usage for the settings page. The cap is the configured entry count
/// (`cover_cache_max`); there is no byte budget for covers.
pub async fn get_cover_cache_stats(
	State(state): State<Arc<AppState>>,
) -> ApiResult<Json<CoverCacheStats>> {
	let dir = state.config.covers_dir.clone();
	let max_entries = state.config.cover_cache_max.min(i64::MAX as usize) as i64;
	let (entries, used_bytes) = tokio::task::spawn_blocking(move || cover_cache_disk_stats(&dir))
		.await
		.map_err(|error| ApiError::internal(format!("cover cache scan task failed: {error}")))?
		.map_err(|error| ApiError::internal(format!("cover cache scan failed: {error}")))?;
	Ok(Json(CoverCacheStats {
		entries,
		used_bytes,
		max_entries,
	}))
}

pub async fn clear_cover_cache(
	State(state): State<Arc<AppState>>,
) -> ApiResult<Json<CoverCacheClearResponse>> {
	let dir = state.config.covers_dir.clone();
	let (cleared_entries, cleared_bytes) =
		tokio::task::spawn_blocking(move || clear_cover_cache_dir(&dir))
			.await
			.map_err(|error| ApiError::internal(format!("cover cache clear task failed: {error}")))?
			.map_err(|error| ApiError::internal(format!("cover cache clear failed: {error}")))?;
	Ok(Json(CoverCacheClearResponse {
		cleared_bytes,
		cleared_entries,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;
	use std::time::{Duration, UNIX_EPOCH};

	fn seed(dir: &FsPath, id: i64, age_secs: u64) -> PathBuf {
		let path = dir.join(id.to_string());
		std::fs::write(&path, b"img").unwrap();
		std::fs::write(sidecar_path(&path), "image/jpeg").unwrap();
		let at = filetime::FileTime::from_system_time(UNIX_EPOCH + Duration::from_secs(1_700_000_000 - age_secs));
		filetime::set_file_mtime(&path, at).unwrap();
		path
	}

	fn cached(dir: &FsPath) -> Vec<i64> {
		let mut ids: Vec<i64> =
			std::fs::read_dir(dir).unwrap().flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok())).collect();
		ids.sort();
		ids
	}

	#[test]
	fn eviction_drops_non_library_covers_before_older_library_ones() {
		let tmp = tempfile::tempdir().unwrap();
		// 1 and 2 are library covers and the oldest of all; 3..=5 are browsed-only
		for (id, age) in [(1, 500), (2, 400), (3, 300), (4, 200), (5, 100)] {
			seed(tmp.path(), id, age);
		}
		let library = HashSet::from([1, 2]);
		evict_over_cap(tmp.path(), 3, &library);
		assert_eq!(cached(tmp.path()), [1, 2, 5]);
		assert!(!sidecar_path(&tmp.path().join("3")).exists(), "sidecars leave with their body");

		// a library larger than the cap still loses its least recently used covers, after every browsed one
		evict_over_cap(tmp.path(), 1, &library);
		assert_eq!(cached(tmp.path()), [2]);
	}

	#[tokio::test]
	async fn eviction_reads_library_membership_from_the_real_schema() {
		let tmp = tempfile::tempdir().unwrap();
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display())).await.unwrap();
		let covers = tmp.path().join("covers");
		std::fs::create_dir(&covers).unwrap();
		let user: i64 = sqlx::query_scalar("INSERT INTO user (username, password_hash) VALUES ('reader', 'x') RETURNING id")
			.fetch_one(&pool)
			.await
			.unwrap();
		let source: i64 = sqlx::query_scalar("INSERT INTO source (key, name, wasm_path) VALUES ('en.test', 'Test', '') RETURNING id")
			.fetch_one(&pool)
			.await
			.unwrap();
		for id in [1_i64, 2, 3] {
			sqlx::query("INSERT INTO manga (id, source_id, key, title) VALUES (?, ?, ?, 'Title')")
				.bind(id)
				.bind(source)
				.bind(id.to_string())
				.execute(&pool)
				.await
				.unwrap();
		}
		sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (1, ?, 1)").bind(user).execute(&pool).await.unwrap();
		sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (2, ?, 0)").bind(user).execute(&pool).await.unwrap();
		// The library cover is the oldest, so only the membership query can save it
		for (id, age) in [(1, 300), (2, 200), (3, 100)] {
			seed(&covers, id, age);
		}
		enforce_cache_limit(&pool, &covers, 2).await;
		assert_eq!(cached(&covers), [1, 3]);
	}

	#[tokio::test]
	async fn a_cache_hit_refreshes_recency() {
		let tmp = tempfile::tempdir().unwrap();
		let old = seed(tmp.path(), 1, 500);
		seed(tmp.path(), 2, 100);
		assert!(read_fresh_cache(&old, Some(1_000), None).await.is_some());
		evict_over_cap(tmp.path(), 1, &HashSet::new());
		assert_eq!(cached(tmp.path()), [1], "the cover just served must outlive the untouched one");
	}

	#[test]
	fn cover_cache_stats_counts_bodies_and_every_file_byte() {
		let tmp = tempfile::tempdir().unwrap();
		seed(tmp.path(), 1, 100);
		seed(tmp.path(), 2, 50);
		std::fs::write(tmp.path().join("1.variant"), "v1").unwrap();
		std::fs::write(tmp.path().join(".3.tmp"), b"partial").unwrap();
		let (entries, bytes) = cover_cache_disk_stats(tmp.path()).unwrap();
		assert_eq!(entries, 2, "tmp files are not covers");
		assert_eq!(bytes, 2 * (3 + "image/jpeg".len() as i64) + 2 + 7);
	}

	#[test]
	fn clear_cover_cache_removes_every_file_and_reports_usage() {
		let tmp = tempfile::tempdir().unwrap();
		seed(tmp.path(), 1, 100);
		std::fs::write(tmp.path().join("1.variant"), "v1").unwrap();
		std::fs::write(tmp.path().join(".2.tmp"), b"partial").unwrap();
		let (entries, bytes) = clear_cover_cache_dir(tmp.path()).unwrap();
		assert_eq!(entries, 1);
		assert_eq!(bytes, 3 + "image/jpeg".len() as i64 + 2 + 7);
		assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
		assert_eq!(clear_cover_cache_dir(tmp.path()).unwrap(), (0, 0));
		assert_eq!(cover_cache_disk_stats(tmp.path()).unwrap(), (0, 0));
	}

	#[test]
	fn if_none_match_accepts_lists_weak_tags_and_wildcard() {
		let tag = etag(b"img");
		let headers = |value: &str| {
			let mut map = HeaderMap::new();
			map.insert(header::IF_NONE_MATCH, value.parse().unwrap());
			map
		};
		assert!(if_none_match_hits(&headers(&tag), &tag));
		assert!(if_none_match_hits(&headers(&format!("\"other\", W/{tag}")), &tag));
		assert!(if_none_match_hits(&headers("*"), &tag));
		assert!(!if_none_match_hits(&headers("\"other\""), &tag));
		assert!(!if_none_match_hits(&HeaderMap::new(), &tag));
		assert_ne!(etag(b"img"), etag(b"img2"));
	}

	#[test]
	fn not_modified_carries_validator_and_no_body() {
		let tag = etag(b"img");
		let mut headers = HeaderMap::new();
		headers.insert(header::IF_NONE_MATCH, tag.parse().unwrap());
		let response = validated_response(&headers, b"img".to_vec(), "image/jpeg", 60);
		assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
		assert_eq!(response.headers()[header::ETAG], tag.as_str());
		assert_eq!(response.headers()[header::CACHE_CONTROL], "private, max-age=60");
		let fresh = validated_response(&HeaderMap::new(), b"img".to_vec(), "image/jpeg", 60);
		assert_eq!(fresh.status(), StatusCode::OK);
		assert_eq!(fresh.headers()[header::ETAG], tag.as_str());
	}
}
