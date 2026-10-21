use crate::error::ApiResult;
use crate::model::{ChapterRow, MangaRow, PageRow};
use crate::state::AppState;
use image::ImageReader;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// How many leading bytes to read when measuring image dimensions. Most image headers (IHDR, SOF, etc.)
/// fit comfortably inside this window; only pathological headers (e.g. very large JPEG EXIF) fall back to a full read.
const DIMENSION_HEAD_BYTES: usize = 64 * 1024;

pub struct PageDimensionUpdate {
	idx: i64,
	content: String,
	width: Option<i64>,
	height: Option<i64>,
}

impl PageDimensionUpdate {
	pub fn new(row: &PageRow, dimensions: Option<(i64, i64)>) -> Self {
		let (width, height) = match dimensions {
			Some((width, height)) => (Some(width), Some(height)),
			None => (None, None),
		};
		Self {
			idx: row.idx,
			content: row.content.clone(),
			width,
			height,
		}
	}
}

pub fn image_dimensions(bytes: &[u8]) -> Option<(i64, i64)> {
	let (width, height) = ImageReader::new(Cursor::new(bytes))
		.with_guessed_format()
		.ok()?
		.into_dimensions()
		.ok()?;
	Some((i64::from(width), i64::from(height)))
}

/// Measures dimensions from a head read, falling back to a full read only when the head is inconclusive
/// and was truncated at the head-bytes limit. A failed head read still attempts the full read so transient
/// errors stay retryable while valid-but-short files avoid the second IO.
///
/// Returns:
/// - `Some(Some(dims))`: bytes were read and dimensions parsed.
/// - `Some(None)`: bytes were read but are not a valid image; the row should be marked checked with nulls.
/// - `None`: no bytes could be read; the caller should skip the row so it stays retryable.
async fn measure_with_fallback<Fut>(
	head: ApiResult<Vec<u8>>,
	full: impl FnOnce() -> Fut,
) -> Option<Option<(i64, i64)>>
where
	Fut: std::future::Future<Output = ApiResult<Vec<u8>>>,
{
	match head {
		Ok(head) => {
			if let Some(dims) = image_dimensions(&head) {
				return Some(Some(dims));
			}
			if head.len() >= DIMENSION_HEAD_BYTES {
				match full().await {
					Ok(full) => Some(image_dimensions(&full)),
					Err(_) => None,
				}
			} else {
				Some(None)
			}
		}
		Err(_) => match full().await {
			Ok(full) => Some(image_dimensions(&full)),
			Err(_) => None,
		},
	}
}

pub async fn persist(pool: &SqlitePool, chapter_id: i64, updates: &[PageDimensionUpdate]) -> ApiResult<()> {
	persist_with_guard(pool, chapter_id, updates, true).await
}

pub async fn persist_downloaded(pool: &SqlitePool, chapter_id: i64, updates: &[PageDimensionUpdate]) -> ApiResult<()> {
	persist_with_guard(pool, chapter_id, updates, false).await
}

async fn persist_with_guard(
	pool: &SqlitePool,
	chapter_id: i64,
	updates: &[PageDimensionUpdate],
	unchecked_only: bool,
) -> ApiResult<()> {
	if updates.is_empty() {
		return Ok(());
	}
	let mut tx = crate::db::begin_write(pool).await?;
	for update in updates {
		// A refresh can replace a row while its old bytes are being read; the content guard keeps the old measurement off the new page
		let sql = if unchecked_only {
			"UPDATE page SET width = ?, height = ?, dimensions_checked = 1
			 WHERE chapter_id = ? AND idx = ? AND dimensions_checked = 0 AND content = ?"
		} else {
			"UPDATE page SET width = ?, height = ?, dimensions_checked = 1
			 WHERE chapter_id = ? AND idx = ? AND content = ?"
		};
		sqlx::query(sql)
			.bind(update.width)
			.bind(update.height)
			.bind(chapter_id)
			.bind(update.idx)
			.bind(&update.content)
			.execute(&mut *tx)
			.await?;
	}
	tx.commit().await?;
	Ok(())
}

/// Tracks chapters whose dimensions are being measured in the background so concurrent page-list
/// requests do not start duplicate work.
static IN_FLIGHT: std::sync::LazyLock<Mutex<HashSet<i64>>> =
	std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

struct InFlightGuard(i64);

impl Drop for InFlightGuard {
	fn drop(&mut self) {
		let mut guard = IN_FLIGHT.lock().expect("dimension in-flight lock");
		guard.remove(&self.0);
	}
}

/// Starts a background task that measures unchecked page dimensions and persists them. The caller
/// returns immediately; a subsequent page-list load will pick up the stored values. Duplicate tasks
/// for the same chapter are suppressed while one is already running.
pub fn spawn_backfill_local(state: &Arc<AppState>, chapter_id: i64, pages: Vec<PageRow>) {
	{
		let mut guard = IN_FLIGHT.lock().expect("dimension in-flight lock");
		if !guard.insert(chapter_id) {
			return;
		}
	}
	let state = state.clone();
	tokio::spawn(async move {
		let _guard = InFlightGuard(chapter_id);
		if let Err(e) = backfill_local(&state, chapter_id, pages).await {
			tracing::warn!(chapter_id, "background page dimension backfill failed: {e:?}");
		}
	});
}

pub async fn backfill_local(
	state: &Arc<AppState>,
	chapter_id: i64,
	mut pages: Vec<PageRow>,
) -> ApiResult<Vec<PageRow>> {
	let mut updates = Vec::new();
	let mut context: Option<(ChapterRow, MangaRow, Vec<PathBuf>)> = None;
	for row in &mut pages {
		if row.dimensions_checked {
			continue;
		}
		let Ok(stored) = serde_json::from_str::<Value>(&row.content) else {
			continue;
		};
		let content = stored.get("content").cloned().unwrap_or(Value::Null);
		if (content.get("Local").is_some() || content.get("OpdsZip").is_some()) && context.is_none() {
			let chapter = super::chapter_row(&state.pool, chapter_id).await?;
			let manga = super::manga_row(&state.pool, chapter.manga_id).await?;
			let local_dirs = state.local_dirs().await?;
			context = Some((chapter, manga, local_dirs));
		}
		let dimensions = if let Some(name) = content.get("Image").and_then(Value::as_str) {
			let head = super::page_image_blob_head(state, chapter_id, name, DIMENSION_HEAD_BYTES).await;
			measure_with_fallback(head, || super::page_image_blob(state, chapter_id, name)).await
		} else if let Some(entry) = content.get("Local").and_then(Value::as_str) {
			let (chapter, manga, local_dirs) = context.as_ref().expect("local context initialized");
			let head = crate::local::read_page_head(
				local_dirs,
				&state.local_zip,
				&manga.key,
				&chapter.key,
				entry,
				DIMENSION_HEAD_BYTES,
			)
			.await;
			let local_zip = state.local_zip.clone();
			measure_with_fallback(head, || {
				let local_dirs = local_dirs.clone();
				let manga_key = manga.key.clone();
				let chapter_key = chapter.key.clone();
				let entry = entry.to_string();
				async move {
					crate::local::read_page_bytes(&local_dirs, &local_zip, &manga_key, &chapter_key, &entry).await
				}
			})
			.await
		} else if let Some(entry) = content.get("OpdsZip").and_then(Value::as_str) {
			let (chapter, manga, _) = context.as_ref().expect("opds context initialized");
			let head = crate::opds_source::read_cbz_page_head(state, manga, chapter, entry, DIMENSION_HEAD_BYTES).await;
			let state = state.clone();
			let entry = entry.to_string();
			measure_with_fallback(head, || async {
				crate::opds_source::read_cbz_page(&state, manga, chapter, &entry)
					.await
					.map(|(bytes, _)| bytes)
			})
			.await
		} else {
			continue;
		};
		let Some(dimensions) = dimensions else {
			continue;
		};
		let update = PageDimensionUpdate::new(row, dimensions);
		row.width = update.width;
		row.height = update.height;
		row.dimensions_checked = true;
		updates.push(update);
	}
	persist(&state.pool, chapter_id, &updates).await?;
	Ok(pages)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::error::ApiError;

	fn png(width: u32, height: u32) -> Vec<u8> {
		let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
		let mut out = Vec::new();
		image::DynamicImage::ImageRgba8(image)
			.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
			.expect("encode png");
		out
	}

	#[test]
	fn reads_valid_image_header() {
		let bytes = png(37, 61);
		assert_eq!(image_dimensions(&bytes), Some((37, 61)));
	}

	#[test]
	fn invalid_image_bytes_have_no_dimensions() {
		assert_eq!(image_dimensions(b"not an image"), None);
	}

	#[tokio::test]
	async fn measures_from_head_when_possible() {
		let bytes = png(37, 61);
		let head = Ok(bytes.clone());
		let full = || async { panic!("full read should not be needed") };
		assert_eq!(measure_with_fallback(head, full).await, Some(Some((37, 61))));
	}

	#[tokio::test]
	async fn falls_back_to_full_when_head_is_truncated_and_invalid() {
		let head = Ok(vec![0u8; DIMENSION_HEAD_BYTES]);
		let full = || async { Ok(png(37, 61)) };
		assert_eq!(measure_with_fallback(head, full).await, Some(Some((37, 61))));
	}

	#[tokio::test]
	async fn short_invalid_head_is_checked_with_nulls() {
		let head = Ok(b"not an image".to_vec());
		let full = || async { panic!("full read should not be needed for short invalid head") };
		assert_eq!(measure_with_fallback(head, full).await, Some(None));
	}

	#[tokio::test]
	async fn failed_head_falls_back_to_full() {
		let head = Err(ApiError::internal("boom"));
		let full = || async { Ok(png(37, 61)) };
		assert_eq!(measure_with_fallback(head, full).await, Some(Some((37, 61))));
	}

	#[tokio::test]
	async fn failed_head_and_full_stay_retryable() {
		let head = Err(ApiError::internal("boom"));
		let full = || async { Err(ApiError::internal("also boom")) };
		assert_eq!(measure_with_fallback(head, full).await, None);
	}
}
