use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterRow, MangaRow, chapter_from_row, manga_from_row, unix_now};
use crate::state::AppState;
use aidoku::{Chapter, Manga, PageContext};
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOutcome {
	pub installed: Vec<String>,
	pub updated: Vec<String>,
	pub skipped: Vec<SkippedSource>,
}

#[derive(Debug, Serialize)]
pub struct SkippedSource {
	pub dir: String,
	pub reason: String,
}

/// 设计 §7.1：source.key 只允许这组字符（保留前缀 local/komga/kavita/suwayomi）
fn valid_source_key(key: &str) -> bool {
	!key.is_empty()
		&& key
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// 目录挂载扫描：sources_dir 一级子目录里有 source.json + main.wasm 即视为源包
pub async fn scan_sources_dir(pool: &SqlitePool, dir: &Path) -> ApiResult<ScanOutcome> {
	let mut outcome = ScanOutcome {
		installed: Vec::new(),
		updated: Vec::new(),
		skipped: Vec::new(),
	};
	let mut read_dir = match tokio::fs::read_dir(dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(outcome),
		Err(e) => return Err(ApiError::from(sqlx::Error::Io(e))),
	};
	let mut dirs = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::from(sqlx::Error::Io(e)))?
	{
		let is_dir = entry
			.file_type()
			.await
			.map(|t| t.is_dir())
			.unwrap_or(false);
		if is_dir {
			dirs.push(entry.path());
		}
	}
	dirs.sort();
	for path in dirs {
		let dir_name = path
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_else(|| path.display().to_string());
		match scan_one(pool, &path).await {
			Ok((key, is_update)) => {
				if is_update {
					outcome.updated.push(key);
				} else {
					outcome.installed.push(key);
				}
			}
			Err(reason) => outcome.skipped.push(SkippedSource {
				dir: dir_name,
				reason,
			}),
		}
	}
	Ok(outcome)
}

/// 成功返回 (key, 是否更新既有源)
async fn scan_one(pool: &SqlitePool, path: &Path) -> Result<(String, bool), String> {
	let json_path = path.join("source.json");
	let wasm_path = path.join("main.wasm");
	if !json_path.is_file() || !wasm_path.is_file() {
		return Err("missing source.json or main.wasm".into());
	}
	let raw = tokio::fs::read(&json_path)
		.await
		.map_err(|e| format!("read source.json: {e}"))?;
	let doc: serde_json::Value =
		serde_json::from_slice(&raw).map_err(|e| format!("parse source.json: {e}"))?;
	let info = doc.get("info").ok_or("missing info")?;
	let key = info
		.get("id")
		.and_then(|v| v.as_str())
		.ok_or("missing info.id")?
		.to_string();
	if !valid_source_key(&key) {
		return Err(format!("invalid source key {key:?}"));
	}
	let name = info
		.get("name")
		.and_then(|v| v.as_str())
		.unwrap_or(&key)
		.to_string();
	let version = info.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
	let base_url = info
		.get("url")
		.and_then(|v| v.as_str())
		.map(str::to_string);
	let content_rating = info
		.get("contentRating")
		.and_then(|v| v.as_i64())
		.unwrap_or(0);
	let languages = info
		.get("languages")
		.cloned()
		.filter(|v| v.is_array())
		.unwrap_or_else(|| serde_json::json!([]))
		.to_string();
	let icon_path = path
		.join("icon.png")
		.is_file()
		.then(|| path.join("icon.png").display().to_string());
	let wasm = wasm_path.display().to_string();
	let now = unix_now();

	let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
		.bind(&key)
		.fetch_optional(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
	if let Some((id,)) = existing {
		// 老源保留 enabled 与 source_setting，只刷新元数据
		sqlx::query(
			"UPDATE source SET name = ?, version = ?, languages = ?, content_rating = ?,
			 base_url = ?, wasm_path = ?, icon_path = ?, updated_at = ? WHERE id = ?",
		)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(wasm)
		.bind(icon_path)
		.bind(now)
		.bind(id)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
		Ok((key, true))
	} else {
		sqlx::query(
			"INSERT INTO source (key, name, version, languages, content_rating, base_url,
			 wasm_path, icon_path, enabled, installed_at, updated_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
		)
		.bind(&key)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(wasm)
		.bind(icon_path)
		.bind(now)
		.bind(now)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
		Ok((key, false))
	}
}

/// 按 (source_id, key) upsert 详情字段；时间戳由调用方按需打点
pub async fn upsert_manga(
	pool: &SqlitePool,
	source_id: i64,
	manga: &Manga,
	initialized: bool,
	fetched_details: bool,
	fetched_chapters: bool,
) -> ApiResult<i64> {
	let now = unix_now();
	let authors = serde_json::to_string(&manga.authors.clone().unwrap_or_default())?;
	let artists = serde_json::to_string(&manga.artists.clone().unwrap_or_default())?;
	let tags = serde_json::to_string(&manga.tags.clone().unwrap_or_default())?;
	sqlx::query(
		"INSERT INTO manga (source_id, key, title, authors, artists, description, tags, status,
		 content_rating, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (source_id, key) DO UPDATE SET
		 title = excluded.title, authors = excluded.authors, artists = excluded.artists,
		 description = excluded.description, tags = excluded.tags, status = excluded.status,
		 content_rating = excluded.content_rating, cover_url = excluded.cover_url,
		 url = excluded.url, initialized = excluded.initialized,
		 update_strategy = excluded.update_strategy,
		 next_update_time = excluded.next_update_time",
	)
	.bind(source_id)
	.bind(&manga.key)
	.bind(&manga.title)
	.bind(authors)
	.bind(artists)
	.bind(&manga.description)
	.bind(tags)
	.bind(manga.status as i64)
	.bind(manga.content_rating as i64)
	.bind(&manga.cover)
	.bind(&manga.url)
	.bind(initialized)
	.bind(manga.update_strategy as i64)
	.bind(manga.next_update_time)
	.bind(if fetched_details { Some(now) } else { None })
	.bind(if fetched_chapters { Some(now) } else { None })
	.execute(pool)
	.await?;

	let (id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(&manga.key)
		.fetch_one(pool)
		.await?;
	if fetched_details {
		sqlx::query("UPDATE manga SET last_fetched_at = ? WHERE id = ?")
			.bind(now)
			.bind(id)
			.execute(pool)
			.await?;
	}
	if fetched_chapters {
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(now)
			.bind(id)
			.execute(pool)
			.await?;
	}
	Ok(id)
}

/// 设计 §7.1 章节对账：source_order 取源返回下标；消失的章节有 download 行则锁定保留，否则删除
pub async fn reconcile_chapters(
	pool: &SqlitePool,
	manga_id: i64,
	chapters: &[Chapter],
) -> ApiResult<()> {
	let now = unix_now();
	let mut seen = HashSet::with_capacity(chapters.len());
	for (index, chapter) in chapters.iter().enumerate() {
		seen.insert(chapter.key.clone());
		let scanlators = serde_json::to_string(&chapter.scanlators.clone().unwrap_or_default())?;
		sqlx::query(
			"INSERT INTO chapter (manga_id, key, title, number, volume, scanlators, language, url,
			 date_uploaded, source_order, locked, fetched_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
			 ON CONFLICT (manga_id, key) DO UPDATE SET
			 title = excluded.title, number = excluded.number, volume = excluded.volume,
			 scanlators = excluded.scanlators, language = excluded.language, url = excluded.url,
			 date_uploaded = excluded.date_uploaded, source_order = excluded.source_order,
			 locked = excluded.locked, fetched_at = excluded.fetched_at",
		)
		.bind(manga_id)
		.bind(&chapter.key)
		.bind(&chapter.title)
		.bind(chapter.chapter_number.map(|n| n as f64))
		.bind(chapter.volume_number.map(|n| n as f64))
		.bind(scanlators)
		.bind(&chapter.language)
		.bind(&chapter.url)
		.bind(chapter.date_uploaded)
		.bind(index as i64)
		.bind(chapter.locked)
		.bind(now)
		.execute(pool)
		.await?;
	}
	let existing: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM chapter WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_all(pool)
		.await?;
	for (chapter_id, key) in existing {
		if seen.contains(&key) {
			continue;
		}
		let has_download: Option<(i64,)> =
			sqlx::query_as("SELECT 1 FROM download WHERE chapter_id = ? LIMIT 1")
				.bind(chapter_id)
				.fetch_optional(pool)
				.await?;
		if has_download.is_some() {
			sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?")
				.bind(chapter_id)
				.execute(pool)
				.await?;
		} else {
			sqlx::query("DELETE FROM chapter WHERE id = ?")
				.bind(chapter_id)
				.execute(pool)
				.await?;
		}
	}
	Ok(())
}

pub async fn manga_row(pool: &SqlitePool, id: i64) -> ApiResult<MangaRow> {
	let row: Option<MangaRow> = sqlx::query_as(
		"SELECT id, source_id, key, title, authors, artists, description, tags, status,
		 content_rating, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at FROM manga WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("manga not found"))
}

pub async fn chapter_row(pool: &SqlitePool, id: i64) -> ApiResult<ChapterRow> {
	let row: Option<ChapterRow> = sqlx::query_as(
		"SELECT id, manga_id, key, title, number, volume, scanlators, language, url,
		 date_uploaded, source_order, page_count, locked, fetched_at FROM chapter WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("chapter not found"))
}

/// 读 page 缓存；miss 或 refresh 时从源拉 get_page_list 并整批落缓存
pub async fn load_pages(
	state: &Arc<AppState>,
	chapter_id: i64,
	refresh: bool,
) -> ApiResult<Vec<(i64, String)>> {
	if !refresh {
		let cached: Vec<(i64, String)> =
			sqlx::query_as("SELECT idx, content FROM page WHERE chapter_id = ? ORDER BY idx ASC")
				.bind(chapter_id)
				.fetch_all(&state.pool)
				.await?;
		if !cached.is_empty() {
			return Ok(cached);
		}
	}
	let chapter = chapter_row(&state.pool, chapter_id).await?;
	let manga = manga_row(&state.pool, chapter.manga_id).await?;
	let handle = state.get_handle(manga.source_id).await?;
	let pages = handle
		.get_page_list(manga_from_row(&manga), chapter_from_row(&chapter))
		.await?;
	{
		let mut tx = state.pool.begin().await?;
		sqlx::query("DELETE FROM page WHERE chapter_id = ?")
			.bind(chapter_id)
			.execute(&mut *tx)
			.await?;
		for (index, page) in pages.iter().enumerate() {
			let stored = serde_json::json!({
				"content": serde_json::to_value(&page.content)?,
				"thumbnail": page.thumbnail,
				"hasDescription": page.has_description,
				"description": page.description,
			});
			sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, ?, ?)")
				.bind(chapter_id)
				.bind(index as i64)
				.bind(serde_json::to_string(&stored)?)
				.execute(&mut *tx)
				.await?;
		}
		sqlx::query("UPDATE chapter SET page_count = ? WHERE id = ?")
			.bind(pages.len() as i64)
			.bind(chapter_id)
			.execute(&mut *tx)
			.await?;
		tx.commit().await?;
	}
	let stored: Vec<(i64, String)> =
		sqlx::query_as("SELECT idx, content FROM page WHERE chapter_id = ? ORDER BY idx ASC")
			.bind(chapter_id)
			.fetch_all(&state.pool)
			.await?;
	Ok(stored)
}

async fn read_body(response: reqwest::Response) -> ApiResult<(Vec<u8>, Option<String>)> {
	let mime = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.map(str::to_string);
	let bytes = response
		.bytes()
		.await
		.map_err(|e| ApiError::bad_gateway(format!("read upstream body failed: {e}")))?;
	Ok((bytes.to_vec(), mime))
}

fn fetch_err(e: crate::routed::FetchError) -> ApiError {
	ApiError::bad_gateway(format!("upstream request failed: {e}"))
}

/// 裸 GET，不经过源的 image_request 改造
pub async fn http_get(state: &AppState, url: &str) -> ApiResult<(Vec<u8>, Option<String>)> {
	let url = reqwest::Url::parse(url).map_err(|_| ApiError::bad_request("invalid upstream url"))?;
	let response = state.http.get(&url).await.map_err(fetch_err)?;
	read_body(response).await
}

/// 按源特性拉图：有 image_request 用源给的 spec 发请求，否则裸 GET
pub async fn fetch_remote(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
	context: Option<PageContext>,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	let handle = state.get_handle(source_id).await?;
	if !handle.features().image_request {
		return http_get(state, url).await;
	}
	let spec = handle.get_image_request(url.to_string(), context).await?;
	let spec_url =
		reqwest::Url::parse(&spec.url).map_err(|_| ApiError::bad_request("invalid image url"))?;
	let mut builder = state
		.http
		.request(spec.method.into(), &spec_url)
		.map_err(fetch_err)?;
	for (name, value) in &spec.headers {
		builder = builder.header(name, value);
	}
	if let Some(body) = spec.body {
		builder = builder.body(body);
	}
	let response = builder
		.send()
		.await
		.map_err(|e| ApiError::bad_gateway(format!("upstream request failed: {e}")))?;
	if !response.status().is_success() {
		return Err(ApiError::bad_gateway(format!(
			"upstream returned status {}",
			response.status()
		)));
	}
	read_body(response).await
}
