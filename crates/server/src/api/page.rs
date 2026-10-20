use crate::source::cover::bytes_response;
use crate::error::{ApiError, ApiResult};
use crate::model::ChapterRow;
use crate::source::downsample::{VariantHit, bucket_width, downsample_png, read_variant, write_variant};
use crate::source::{chapter_row, fetch_page_image, http_get, load_pages, manga_row, page_image_blob};
use crate::state::AppState;
use aidoku::PageContext;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

const PAGE_MAX_AGE: u64 = 3600;

#[derive(Deserialize)]
pub struct PageQuery {
	/// 请求降采样变体的目标宽度；缺省/0 = 原图字节直通
	width: Option<u32>,
}

pub async fn get_page(
	State(state): State<Arc<AppState>>,
	Path((id, index)): Path<(i64, i64)>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Response> {
	let chapter = chapter_row(&state.pool, id).await?;
	let pages = load_pages(&state, id, false).await?;
	let Some((_, content)) = pages.iter().find(|(idx, _)| *idx == index) else {
		return Err(ApiError::not_found("page not found"));
	};
	let stored: Value = serde_json::from_str(content)?;
	let content = stored.get("content").cloned().unwrap_or(Value::Null);
	if let Some(text) = content.get("Text").and_then(Value::as_str) {
		return Ok(Json(json!({ "type": "text", "text": text })).into_response());
	}

	let mut bucket = query.width.filter(|w| *w > 0).map(bucket_width);
	if let Some(b) = bucket {
		match read_variant(&state.config.page_blobs_dir(), id, index, b).await {
			Some(VariantHit::Png(bytes)) => return Ok(bytes_response(bytes, "image/png", PAGE_MAX_AGE)),
			// pass 命中 = 该桶不缩（窄图/解不了码），跳变体逻辑走原路径
			Some(VariantHit::PassThrough) => bucket = None,
			None => {}
		}
	}

	let (bytes, mime) = page_image_bytes(&state, &chapter, &content).await?;
	if let Some(b) = bucket {
		// 解码+缩放是 CPU 重活，挪阻塞线程；原字节随闭包带回供直通分支用
		let (resized, original) = tokio::task::spawn_blocking(move || {
			let resized = downsample_png(&bytes, b);
			(resized, bytes)
		})
		.await
		.map_err(|e| ApiError::internal(format!("downsample task failed: {e}")))?;
		let blobs_dir = state.config.page_blobs_dir();
		return match resized {
			Some(png) => {
				if let Err(e) = write_variant(&blobs_dir, id, index, b, Some(&png)).await {
					tracing::warn!(chapter_id = id, index, "page variant write failed: {e}");
				}
				Ok(bytes_response(png, "image/png", PAGE_MAX_AGE))
			}
			None => {
				if let Err(e) = write_variant(&blobs_dir, id, index, b, None).await {
					tracing::warn!(chapter_id = id, index, "page variant marker write failed: {e}");
				}
				Ok(bytes_response(original, &mime, PAGE_MAX_AGE))
			}
		};
	}
	Ok(bytes_response(bytes, &mime, PAGE_MAX_AGE))
}

/// 图像页内容取原图字节（Image 物化 blob / Local 本地文件 / Zip 解包 / Url 回源），与下载路径同源
async fn page_image_bytes(state: &Arc<AppState>, chapter: &ChapterRow, content: &Value) -> ApiResult<(Vec<u8>, String)> {
	if let Some(name) = content.get("Image").and_then(Value::as_str) {
		let bytes = page_image_blob(state, chapter.id, name).await?;
		return Ok((bytes, "image/png".to_string()));
	}
	let manga = manga_row(&state.pool, chapter.manga_id).await?;
	if let Some(entry) = content.get("Local").and_then(Value::as_str) {
		let bytes = crate::local::read_page(
			&state.config.local_dir(),
			&state.local_zip,
			&manga.key,
			&chapter.key,
			entry,
		)
		.await?;
		return Ok((bytes, crate::local::image_mime(entry).to_string()));
	}
	if let Some(entry) = content.get("OpdsZip").and_then(Value::as_str) {
		return crate::opds_source::read_cbz_page(state, &manga, chapter, entry).await;
	}
	if let Some(zip) = content.get("Zip").and_then(Value::as_array) {
		let Some(zip_url) = zip.first().and_then(Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let Some(entry_path) = zip.get(1).and_then(Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let (zip_bytes, _) = http_get(state, manga.source_id, zip_url).await?;
		let entry = extract_zip_entry(&zip_bytes, entry_path)?;
		return Ok((entry, mime_for(entry_path).to_string()));
	}
	if let Some(url) = content.get("Url").and_then(Value::as_array) {
		let Some(url_str) = url.first().and_then(Value::as_str) else {
			return Err(ApiError::bad_request("invalid url page content"));
		};
		let context: Option<PageContext> = match url.get(1) {
			Some(value) if !value.is_null() => Some(serde_json::from_value(value.clone())?),
			_ => None,
		};
		let (bytes, mime) = fetch_page_image(state, manga.source_id, url_str, context).await?;
		return Ok((bytes, mime.unwrap_or_else(|| "application/octet-stream".into())));
	}
	Err(ApiError::bad_request("unknown page content"))
}

fn extract_zip_entry(zip_bytes: &[u8], entry_path: &str) -> ApiResult<Vec<u8>> {
	use std::io::Read;
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
		.map_err(|e| ApiError::bad_gateway(format!("invalid zip archive: {e}")))?;
	let mut entry = archive
		.by_name(entry_path)
		.map_err(|_| ApiError::not_found("zip entry not found"))?;
	let mut out = Vec::with_capacity(entry.size() as usize);
	entry
		.read_to_end(&mut out)
		.map_err(|e| ApiError::bad_gateway(format!("read zip entry failed: {e}")))?;
	Ok(out)
}

fn mime_for(path: &str) -> &'static str {
	let ext = path
		.rsplit('.')
		.next()
		.map(str::to_ascii_lowercase)
		.unwrap_or_default();
	match ext.as_str() {
		"jpg" | "jpeg" => "image/jpeg",
		"png" => "image/png",
		"webp" => "image/webp",
		"gif" => "image/gif",
		"bmp" => "image/bmp",
		"avif" => "image/avif",
		_ => "application/octet-stream",
	}
}
