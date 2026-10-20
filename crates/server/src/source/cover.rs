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

/// 源图床的防盗链（Referer 等）只有服务端能按源声明带上，<img> 直连必 403；
/// 与详情页封面同一条 fetch_cover_image：源声明了 cover 处理器就返回处理后的字节
/// （加密图床不处理就是浏览器解不出的密文）。不落缓存——浏览/home 封面量大且复用率低
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

/// 备选封面列表：以库里的 manga 为入参调源导出；源没导出按惯例 501
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

/// 选定封面：只写 cover_url 并让磁盘缓存失效。不碰 last_fetched_at（那是源刷新语义）——
/// 缓存按它判新，不清旧字节的话换完后旧封面继续被 serve，选择永远不生效
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

/// 本体与 sidecar 一起删；还没缓存过时缺文件不算错误
async fn invalidate_cache(covers_dir: &FsPath, id: i64) {
	let cache_path = covers_dir.join(id.to_string());
	let _ = tokio::fs::remove_file(sidecar_path(&cache_path)).await;
	let _ = tokio::fs::remove_file(cache_path).await;
}

fn sidecar_path(cache_path: &FsPath) -> PathBuf {
	cache_path.with_file_name(format!("{}.content-type", cache_path.file_name().unwrap_or_default().to_string_lossy()))
}

/// 缓存命中条件：文件存在且 manga 详情没在缓存写入之后重新拉过（last_fetched_at <= mtime）
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

/// 先写 tmp 再 rename，避免并发读到写了一半的缓存
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

/// LRU：缓存文件数超上限时按 mtime 最旧先删（连同 sidecar）
fn enforce_cache_limit(covers_dir: &FsPath, max: usize) {
	let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
	let Ok(read_dir) = std::fs::read_dir(covers_dir) else {
		return;
	};
	for entry in read_dir.flatten() {
		let name = entry.file_name();
		// 只数封面本体（文件名是纯 manga id），sidecar 与 tmp 随本体连带处理
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
