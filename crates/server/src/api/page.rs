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
	/// Target width for a requested downsampled variant; absent/0 = pass the original bytes through
	width: Option<u32>,
}

pub async fn get_page(
	State(state): State<Arc<AppState>>,
	Path((id, index)): Path<(i64, i64)>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Response> {
	// A finished download product is a preferred page source: text entries come from the artifact
	// with no source access, and image entries are read from it when the materialized blob is
	// missing, so downloaded chapters stay readable offline. Image entries still prefer the
	// materialized blob when it exists: a whole-chapter refresh rewrites the blob directory and
	// those fresh bytes must win over the older product. Falls back to the page cache/source path
	// when there is no usable product (or the entry read fails).
	if let Some((bytes, entry)) = crate::download::read_product_page(&state, id, index as usize).await? {
		if crate::download::is_text_entry(&entry) {
			return Ok(Json(json!({ "type": "text", "text": String::from_utf8_lossy(&bytes) })).into_response());
		}
		let blob_missing = !tokio::fs::try_exists(
			state
				.config
				.page_blobs_dir()
				.join(id.to_string())
				.join(format!("{index}.png")),
		)
		.await
		.unwrap_or(false);
		if !blob_missing {
			// The blob wins: fall through to the page cache/source path below.
		} else {
			let mut bucket = query.width.filter(|w| *w > 0).map(bucket_width);
			if let Some(b) = bucket {
				match read_variant(&state.config.page_blobs_dir(), id, index, b).await {
					Some(VariantHit::Png(bytes)) => return Ok(bytes_response(bytes, "image/png", PAGE_MAX_AGE)),
					// A pass hit = this bucket does not resize (narrow image/undecodable); skip the variant logic and take the original path
					Some(VariantHit::PassThrough) => bucket = None,
					None => {}
				}
			}
			if let Some(b) = bucket {
				// Decode+resize is CPU-heavy work, moved to a blocking thread; the original bytes ride the closure back for the pass-through branch
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
						Ok(bytes_response(original, mime_for(&entry), PAGE_MAX_AGE))
					}
				};
			}
			return Ok(bytes_response(bytes, mime_for(&entry), PAGE_MAX_AGE));
		}
	}

	let chapter = chapter_row(&state.pool, id).await?;
	// a trashed chapter's files are gone: refuse reads at the API boundary, not only in the web UI
	if chapter.trashed_at.is_some() {
		return Err(ApiError::not_found("chapter is in the trash"));
	}
	let pages = load_pages(&state, id, false).await?;
	let Some(page) = pages.iter().find(|page| page.idx == index) else {
		return Err(ApiError::not_found("page not found"));
	};
	let stored: Value = serde_json::from_str(&page.content)?;
	let content = stored.get("content").cloned().unwrap_or(Value::Null);
	if let Some(text) = content.get("Text").and_then(Value::as_str) {
		return Ok(Json(json!({ "type": "text", "text": text })).into_response());
	}

	let mut bucket = query.width.filter(|w| *w > 0).map(bucket_width);
	if let Some(b) = bucket {
		match read_variant(&state.config.page_blobs_dir(), id, index, b).await {
			Some(VariantHit::Png(bytes)) => return Ok(bytes_response(bytes, "image/png", PAGE_MAX_AGE)),
			// A pass hit = this bucket does not resize (narrow image/undecodable); skip the variant logic and take the original path
			Some(VariantHit::PassThrough) => bucket = None,
			None => {}
		}
	}

	let (bytes, mime) = page_image_bytes(&state, &chapter, &content).await?;
	if let Some(b) = bucket {
		// Decode+resize is CPU-heavy work, moved to a blocking thread; the original bytes ride the closure back for the pass-through branch
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

/// Fetches the original bytes of an image page's content (Image materialized blob / Local local file / Zip unpack / Url source fetch), sharing the download path's sources
async fn page_image_bytes(state: &Arc<AppState>, chapter: &ChapterRow, content: &Value) -> ApiResult<(Vec<u8>, String)> {
	if let Some(name) = content.get("Image").and_then(Value::as_str) {
		let bytes = page_image_blob(state, chapter.id, name).await?;
		return Ok((bytes, "image/png".to_string()));
	}
	let manga = manga_row(&state.pool, chapter.manga_id).await?;
	if let Some(entry) = content.get("Local").and_then(Value::as_str) {
		return crate::local::read_page(
			&state.local_dirs(manga.source_id).await?,
			&state.local_zip,
			&manga.key,
			&chapter.key,
			entry,
		)
		.await;
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::api::{get_pages, RefreshQuery};
	use axum::body::to_bytes;
	use axum::extract::Extension;
	use axum::http::header;
	use std::path::Path;

	async fn page_db() -> (Arc<AppState>, i64, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let config = crate::config::Config {
			config_dir: tmp.path().to_path_buf(),
			database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
			covers_dir: tmp.path().join("covers"),
			downloads_dir: tmp.path().join("downloads"),
			..crate::config::Config::default()
		};
		let pool = crate::db::create_pool(&config.database_url).await.expect("pool");
		let source_id: i64 = sqlx::query_scalar(
			"INSERT INTO source (key, name, wasm_path) VALUES ('page-source', 'Source', '') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("source row");
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key) VALUES (?, 'manga') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("manga row");
		let chapter_id: i64 =
			sqlx::query_scalar("INSERT INTO chapter (manga_id, key) VALUES (?, 'chapter') RETURNING id")
				.bind(manga_id)
				.fetch_one(&pool)
				.await
				.expect("chapter row");
		let (defaults_tx, _defaults_rx) = tokio::sync::mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = tokio::sync::mpsc::unbounded_channel();
		let state = AppState::new(pool, config, defaults_tx, cookie_tx);
		assert!(state.downloads.set(crate::download::DownloadManager::new(&state)).is_ok());
		(state, manga_id, chapter_id, tmp)
	}

	/// Writes a CBZ-style archive (ComicInfo.xml first, then the given pages) with the same layout
	/// the downloader produces, so the product readers see real artifacts.
	fn write_test_cbz(path: &Path, pages: &[(&str, &[u8])]) {
		use std::io::Write;
		let file = std::fs::File::create(path).expect("create cbz");
		let mut archive = zip::ZipWriter::new(file);
		let options = zip::write::SimpleFileOptions::default();
		archive.start_file("ComicInfo.xml", options).expect("start comic info");
		archive.write_all(b"<ComicInfo/>").expect("write comic info");
		for (name, bytes) in pages {
			archive.start_file(*name, options).expect("start page");
			archive.write_all(bytes).expect("write page");
		}
		archive.finish().expect("finish cbz");
	}

	async fn insert_finished_product(state: &AppState, chapter_id: i64, format: &str, path: &str) {
		sqlx::query(
			"INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', ?, ?)",
		)
		.bind(chapter_id)
		.bind(format)
		.bind(path)
		.execute(&state.pool)
		.await
		.expect("finished download row");
	}

	async fn get_page_bytes(
		state: &Arc<AppState>,
		chapter_id: i64,
		index: i64,
	) -> (axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>) {
		let resp = get_page(State(state.clone()), Path((chapter_id, index)), Query(PageQuery { width: None }))
			.await
			.expect("get_page");
		let status = resp.status();
		let headers = resp.headers().clone();
		let body = to_bytes(resp.into_body(), 1024 * 1024).await.expect("body");
		(status, headers, body.to_vec())
	}

	#[tokio::test]
	async fn finished_cbz_product_serves_page_bytes_without_page_rows() {
		let (state, _manga_id, chapter_id, _tmp) = page_db().await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(&dir.join("ch.cbz"), &[("000.png", b"\x89PNG-DL")]);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/ch.cbz").await;
		// No page rows at all: the bytes can only come from the product, which also proves the
		// offline case (no page cache, no source access).
		let (status, headers, body) = get_page_bytes(&state, chapter_id, 0).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		assert_eq!(
			headers.get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap()),
			Some("image/png")
		);
		assert_eq!(body, b"\x89PNG-DL");
	}

	#[tokio::test]
	async fn finished_cbz_text_page_returns_text() {
		let (state, _manga_id, chapter_id, _tmp) = page_db().await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(&dir.join("ch.cbz"), &[("000.txt", b"chapter one text")]);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/ch.cbz").await;
		let (status, _, body) = get_page_bytes(&state, chapter_id, 0).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		let value: Value = serde_json::from_slice(&body).expect("text json");
		assert_eq!(value["type"], "text");
		assert_eq!(value["text"], "chapter one text");
	}

	#[tokio::test]
	async fn finished_folder_product_serves_page_bytes() {
		let (state, _manga_id, chapter_id, _tmp) = page_db().await;
		let dir = state.config.downloads_dir.join("Source/Manga/ch");
		std::fs::create_dir_all(&dir).expect("product dir");
		std::fs::write(dir.join("ComicInfo.xml"), b"<ComicInfo/>").expect("comic info");
		std::fs::write(dir.join("000.png"), b"\x89PNG-FOLDER").expect("page");
		insert_finished_product(&state, chapter_id, "folder", "Source/Manga/ch").await;
		let (status, _, body) = get_page_bytes(&state, chapter_id, 0).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		assert_eq!(body, b"\x89PNG-FOLDER");
	}

	#[tokio::test]
	async fn missing_product_falls_back_to_page_rows() {
		let (state, _manga_id, chapter_id, _tmp) = page_db().await;
		// Page rows + materialized blob exist, but the finished download row's artifact is gone
		// from disk: the reader must fall back to the page cache/source path.
		let blob_dir = state.config.page_blobs_dir().join(chapter_id.to_string());
		std::fs::create_dir_all(&blob_dir).expect("blob dir");
		std::fs::write(blob_dir.join("0.png"), b"BLOB-BYTES").expect("blob");
		sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, 0, ?)")
			.bind(chapter_id)
			.bind(serde_json::to_string(&json!({ "content": { "Image": "0.png" } })).expect("encode"))
			.execute(&state.pool)
			.await
			.expect("page row");
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/gone.cbz").await;
		let (status, _, body) = get_page_bytes(&state, chapter_id, 0).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		assert_eq!(body, b"BLOB-BYTES");
	}

	#[tokio::test]
	async fn get_pages_builds_list_from_product_without_source() {
		let (state, _manga_id, chapter_id, _tmp) = page_db().await;
		let dir = state.config.downloads_dir.join("Source/Manga");
		std::fs::create_dir_all(&dir).expect("product dir");
		write_test_cbz(
			&dir.join("ch.cbz"),
			&[("000.png", b"a"), ("001.png", b"b"), ("002.txt", b"hello")],
		);
		insert_finished_product(&state, chapter_id, "cbz", "Source/Manga/ch.cbz").await;
		let resp = get_pages(
			State(state.clone()),
			Extension(state.auth.admin_user()),
			Path(chapter_id),
			Query(RefreshQuery { refresh: None }),
		)
		.await
		.expect("get_pages");
		let value: Value = resp.0;
		let list = value.as_array().expect("array");
		assert_eq!(list.len(), 3);
		assert_eq!(list[0]["index"], 0);
		assert_eq!(list[0]["content"]["Downloaded"], "000.png");
		assert_eq!(list[2]["content"]["Text"], "hello");
	}

	#[tokio::test]
	async fn finished_product_reads_absolute_path_after_move() {
		let (state, _manga_id, chapter_id, tmp) = page_db().await;
		// A download.move_path target outside downloads_dir: the row stores the absolute path
		// and product_path must use it as-is (see download/mod.rs product_path).
		let moved = tmp.path().join("library/Series/第 3 话.cbz");
		std::fs::create_dir_all(moved.parent().expect("parent")).expect("moved dir");
		write_test_cbz(
			&moved,
			&[("000.png", b"\x89PNG-MOVED"), ("001.png", b"\x89PNG-MOVED-2")],
		);
		insert_finished_product(&state, chapter_id, "cbz", &moved.to_string_lossy()).await;
		let (status, _, body) = get_page_bytes(&state, chapter_id, 0).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		assert_eq!(body, b"\x89PNG-MOVED");
		let (status, _, body) = get_page_bytes(&state, chapter_id, 1).await;
		assert_eq!(status, axum::http::StatusCode::OK);
		assert_eq!(body, b"\x89PNG-MOVED-2");
		let resp = get_pages(
			State(state.clone()),
			Extension(state.auth.admin_user()),
			Path(chapter_id),
			Query(RefreshQuery { refresh: None }),
		)
		.await
		.expect("get_pages");
		let list = resp.0.as_array().expect("array");
		assert_eq!(list.len(), 2);
		assert_eq!(list[0]["content"]["Downloaded"], "000.png");
		assert_eq!(list[1]["content"]["Downloaded"], "001.png");
	}
}
