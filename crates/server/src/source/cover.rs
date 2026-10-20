use crate::api::{RefreshQuery, wants_refresh};
use crate::error::{ApiError, ApiResult};
use crate::model::manga_from_row;
use crate::source::{fetch_cover_image, manga_row};
use crate::state::AppState;
use axum::Json;
use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

const DEFAULT_MIME: &str = "image/jpeg";
const COVER_MAX_AGE: u64 = 86400;

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
	pub url: String,
}

/// A source image host's hotlink protection (Referer etc.) can only be satisfied by the server per the source's
/// declaration, so a direct <img> always gets 403; this takes the same fetch_cover_image as the details-page cover:
/// a source-declared cover processor returns processed bytes (an encrypted image host is otherwise ciphertext the
/// browser cannot decode). Not cached — browse/home covers are numerous and rarely reused
pub async fn get_source_image(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<ImageQuery>,
) -> ApiResult<Response> {
	let parsed = reqwest::Url::parse(&query.url).map_err(|_| ApiError::bad_request("invalid image url"))?;
	if !matches!(parsed.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("unsupported image url scheme"));
	}
	let (bytes, mime) = fetch_cover_image(&state, id, &query.url).await?;
	let mime = mime.unwrap_or_else(|| DEFAULT_MIME.into());
	Ok(bytes_response(bytes, &mime, COVER_MAX_AGE))
}

pub async fn get_cover(
	State(state): State<Arc<AppState>>,
	Path(id): Path<i64>,
	Query(query): Query<RefreshQuery>,
) -> ApiResult<Response> {
	let manga = manga_row(&state.pool, id).await?;
	let covers_dir = state.config.covers_dir.clone();
	let cache_path = covers_dir.join(id.to_string());
	if !wants_refresh(&query)
		&& let Some((bytes, mime)) = read_fresh_cache(&cache_path, manga.last_fetched_at).await
	{
		return Ok(bytes_response(bytes, &mime, COVER_MAX_AGE));
	}
	let (bytes, mime) = if crate::source::source_is_local(&state.pool, manga.source_id).await? {
		crate::local::resolve_cover(&state.config.local_dir(), &state.local_zip, &manga.key)
			.await?
			.ok_or_else(|| ApiError::not_found("cover not available"))?
	} else {
		let Some(url) = manga.cover_url.clone() else {
			return Err(ApiError::not_found("cover not available"));
		};
		let (bytes, mime) = fetch_cover_image(&state, manga.source_id, &url).await?;
		(bytes, mime.unwrap_or_else(|| DEFAULT_MIME.into()))
	};
	match write_cache(&covers_dir, id, &bytes, &mime).await {
		Ok(()) => enforce_cache_limit(&covers_dir, state.config.cover_cache_max),
		Err(e) => tracing::warn!(manga_id = id, "cover cache write failed: {e}"),
	}
	Ok(bytes_response(bytes, &mime, COVER_MAX_AGE))
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
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?")
		.bind(&body.url)
		.bind(id)
		.execute(&state.pool)
		.await?;
	invalidate_cache(&state.config.covers_dir, id).await;
	let row = manga_row(&state.pool, id).await?;
	Ok(Json(crate::api::manga_json(&state, user.id, &row).await?))
}

/// Deletes the body and its sidecar together; missing files are not an error when nothing was cached yet
async fn invalidate_cache(covers_dir: &FsPath, id: i64) {
	let cache_path = covers_dir.join(id.to_string());
	let _ = tokio::fs::remove_file(sidecar_path(&cache_path)).await;
	let _ = tokio::fs::remove_file(cache_path).await;
}

fn sidecar_path(cache_path: &FsPath) -> PathBuf {
	cache_path.with_file_name(format!("{}.content-type", cache_path.file_name().unwrap_or_default().to_string_lossy()))
}

/// Cache-hit condition: the file exists and the manga details were not re-fetched after the cache write (last_fetched_at <= mtime)
async fn read_fresh_cache(cache_path: &FsPath, last_fetched_at: Option<i64>) -> Option<(Vec<u8>, String)> {
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
	let bytes = tokio::fs::read(cache_path).await.ok()?;
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
) -> std::io::Result<()> {
	let tmp = covers_dir.join(format!(".{id}.tmp"));
	tokio::fs::write(&tmp, bytes).await?;
	tokio::fs::rename(&tmp, covers_dir.join(id.to_string())).await?;
	let tmp = covers_dir.join(format!(".{id}.content-type.tmp"));
	tokio::fs::write(&tmp, mime).await?;
	tokio::fs::rename(&tmp, covers_dir.join(format!("{id}.content-type"))).await?;
	Ok(())
}

/// LRU: when the cache file count exceeds the cap, delete oldest by mtime first (sidecar included)
fn enforce_cache_limit(covers_dir: &FsPath, max: usize) {
	let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
	let Ok(read_dir) = std::fs::read_dir(covers_dir) else {
		return;
	};
	for entry in read_dir.flatten() {
		let name = entry.file_name();
		// Only count the cover bodies (file name is a bare manga id); sidecars and tmp files are handled along with their body
		if name.to_str().and_then(|n| n.parse::<i64>().ok()).is_none() {
			continue;
		}
		let Ok(metadata) = entry.metadata() else {
			continue;
		};
		let Ok(mtime) = metadata.modified() else {
			continue;
		};
		entries.push((entry.path(), mtime));
	}
	if entries.len() <= max {
		return;
	}
	entries.sort_by_key(|(_, mtime)| *mtime);
	let excess = entries.len() - max;
	for (path, _) in entries.into_iter().take(excess) {
		let _ = std::fs::remove_file(sidecar_path(&path));
		let _ = std::fs::remove_file(path);
	}
}
