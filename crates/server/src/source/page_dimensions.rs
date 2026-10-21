use crate::error::ApiResult;
use crate::model::{ChapterRow, MangaRow, PageRow};
use crate::state::AppState;
use image::ImageReader;
use serde_json::Value;
use sqlx::SqlitePool;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

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
		let bytes = if let Some(name) = content.get("Image").and_then(Value::as_str) {
			match super::page_image_blob(state, chapter_id, name).await {
				Ok(bytes) => bytes,
				Err(_) => continue,
			}
		} else if let Some(entry) = content.get("Local").and_then(Value::as_str) {
			let (chapter, manga, local_dirs) = context.as_ref().expect("local context initialized");
			match crate::local::read_page_bytes(local_dirs, &state.local_zip, &manga.key, &chapter.key, entry).await {
				Ok(bytes) => bytes,
				Err(_) => continue,
			}
		} else if let Some(entry) = content.get("OpdsZip").and_then(Value::as_str) {
			let (chapter, manga, _) = context.as_ref().expect("opds context initialized");
			match crate::opds_source::read_cbz_page(state, manga, chapter, entry).await {
				Ok((bytes, _)) => bytes,
				Err(_) => continue,
			}
		} else {
			continue;
		};
		let update = PageDimensionUpdate::new(row, image_dimensions(&bytes));
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
}
