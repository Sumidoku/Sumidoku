pub mod cover;
pub mod routed;

use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterRow, MangaRow, chapter_from_row, json_to_default_value, manga_from_row, unix_now,
};
use crate::state::AppState;
use aidoku::{Chapter, Manga, PageContext};
use aidoku_wasm::DefaultValue;
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

/// source.key 只允许这组字符（保留前缀 local/komga/kavita/suwayomi）
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
	let (id, updated) = if let Some((id,)) = existing {
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
		(id, true)
	} else {
		let result = sqlx::query(
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
		(result.last_insert_rowid(), false)
	};
	seed_source_defaults(pool, id, path, &doc).await?;
	Ok((key, updated))
}

/// 对齐 Aidoku 安装源时的 register(defaults:) 语义：播种语言列表与 settings.json 默认值。
/// 源运行时会 defaults.get 这些键（如 mangadex 的 contentRating），缺了直接报错；
/// INSERT OR IGNORE 保证不覆盖用户已改的值
async fn seed_source_defaults(
	pool: &SqlitePool,
	source_id: i64,
	dir: &Path,
	doc: &serde_json::Value,
) -> Result<(), String> {
	let mut seeds: Vec<(String, DefaultValue)> = Vec::new();
	if let Some((key, langs)) = default_languages(doc) {
		seeds.push((key, langs));
	}
	// allowsBaseUrlSelect 时 App 会合成 base url 设置，默认第一个镜像
	// （urls = info.url + info.urls）；copymanga 等源 defaults.get("url") 必读
	let allows_select = doc
		.get("config")
		.and_then(|c| c.get("allowsBaseUrlSelect"))
		.and_then(|v| v.as_bool())
		.unwrap_or(false);
	if allows_select {
		let info = doc.get("info");
		let mut urls: Vec<&str> = info
			.and_then(|i| i.get("url"))
			.and_then(|v| v.as_str())
			.into_iter()
			.collect();
		if let Some(more) = info.and_then(|i| i.get("urls")).and_then(|v| v.as_array()) {
			urls.extend(more.iter().filter_map(|u| u.as_str()));
		}
		if urls.len() > 1 {
			seeds.push(("url".to_string(), DefaultValue::String(urls[0].to_string())));
		}
	}
	if let Ok(raw) = tokio::fs::read(dir.join("settings.json")).await {
		match serde_json::from_slice::<serde_json::Value>(&raw) {
			Ok(settings) => collect_setting_defaults(&settings, &mut seeds),
			Err(e) => tracing::warn!(source_id, "skip unparsable settings.json: {e}"),
		}
	}
	for (key, value) in seeds {
		let raw = serde_json::to_string(&value).map_err(|e| format!("encode default: {e}"))?;
		sqlx::query("INSERT OR IGNORE INTO source_setting (source_id, key, value) VALUES (?, ?, ?)")
			.bind(source_id)
			.bind(key)
			.bind(raw)
			.execute(pool)
			.await
			.map_err(|e| format!("db: {e}"))?;
	}
	Ok(())
}

/// 多语言源的语言设置种子：>1 种语言才存在该设置；键与值类型随 languageSelectType
/// （multiple → languages: StringArray，single → language: String）。
/// App 按设备语言取交集定默认，服务端没有设备语言，退化为 isDefault 否则第一个
fn default_languages(doc: &serde_json::Value) -> Option<(String, DefaultValue)> {
	let langs = doc.get("info")?.get("languages")?.as_array()?;
	let mut codes: Vec<(&str, bool)> = Vec::new();
	for lang in langs {
		match lang {
			serde_json::Value::String(code) => codes.push((code, false)),
			serde_json::Value::Object(_) => {
				let code = lang
					.get("code")
					.or_else(|| lang.get("value"))
					.and_then(|v| v.as_str());
				if let Some(code) = code {
					let is_default = lang
						.get("isDefault")
						.and_then(|v| v.as_bool())
						.unwrap_or(false);
					codes.push((code, is_default));
				}
			}
			_ => {}
		}
	}
	if codes.len() < 2 {
		return None;
	}
	let mut picked: Vec<&str> = codes
		.iter()
		.filter(|(_, is_default)| *is_default)
		.map(|(code, _)| *code)
		.collect();
	if picked.is_empty() {
		picked.push(codes[0].0);
	}
	let single = doc
		.get("config")
		.and_then(|c| c.get("languageSelectType"))
		.and_then(|v| v.as_str())
		== Some("single");
	if single {
		Some(("language".to_string(), DefaultValue::String(picked[0].to_string())))
	} else {
		Some((
			"languages".to_string(),
			DefaultValue::StringArray(picked.into_iter().map(str::to_string).collect()),
		))
	}
}

fn collect_setting_defaults(
	items: &serde_json::Value,
	out: &mut Vec<(String, DefaultValue)>,
) {
	let Some(items) = items.as_array() else { return };
	for item in items {
		if item.get("type").and_then(|v| v.as_str()) == Some("group") {
			if let Some(children) = item.get("items") {
				collect_setting_defaults(children, out);
			}
			continue;
		}
		// Aidoku 的 select/segment 无显式默认时落第一个候选值
		let mut default = item.get("default");
		if default.is_none()
			&& matches!(
				item.get("type").and_then(|v| v.as_str()),
				Some("select") | Some("segment")
			)
		{
			default = item
				.get("values")
				.and_then(|v| v.as_array())
				.and_then(|v| v.first());
		}
		let (Some(key), Some(default)) =
			(item.get("key").and_then(|v| v.as_str()), default)
		else {
			continue;
		};
		// Null 种下后 defaults.get 一样是无效值，不浪费一行
		let value = json_to_default_value(default);
		if !matches!(value, DefaultValue::Null) {
			out.push((key.to_string(), value));
		}
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

/// 章节对账：source_order 取源返回下标；消失的章节有 download 行则锁定保留，否则删除。
/// 返回本次对账新插入的 chapter id（更新检测据此判定新增章节）
pub async fn reconcile_chapters(
	pool: &SqlitePool,
	manga_id: i64,
	chapters: &[Chapter],
) -> ApiResult<Vec<i64>> {
	let now = unix_now();
	let existing: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM chapter WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_all(pool)
		.await?;
	let known: HashSet<&String> = existing.iter().map(|(_, key)| key).collect();
	let mut seen = HashSet::with_capacity(chapters.len());
	let mut new_keys = Vec::new();
	for (index, chapter) in chapters.iter().enumerate() {
		if seen.insert(chapter.key.clone()) && !known.contains(&chapter.key) {
			new_keys.push(chapter.key.clone());
		}
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
	let mut new_ids = Vec::with_capacity(new_keys.len());
	for key in new_keys {
		let (id,): (i64,) = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
			.bind(manga_id)
			.bind(&key)
			.fetch_one(pool)
			.await?;
		new_ids.push(id);
	}
	Ok(new_ids)
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

fn fetch_err(e: crate::source::routed::FetchError) -> ApiError {
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

const COVER_DEFAULT_MIME: &str = "image/jpeg";

/// OPDS 封面取字节：流程同 cover.rs（新鲜磁盘缓存优先，miss 回源拉取并写缓存）
pub async fn cover_bytes(state: &Arc<AppState>, manga_id: i64) -> ApiResult<(Vec<u8>, Option<String>)> {
	let manga = manga_row(&state.pool, manga_id).await?;
	let covers_dir = state.config.covers_dir.clone();
	let cache_path = covers_dir.join(manga_id.to_string());
	if let Some((bytes, mime)) = cover_cache_read(&cache_path, manga.last_fetched_at).await {
		return Ok((bytes, Some(mime)));
	}
	let Some(url) = manga.cover_url.clone() else {
		return Err(ApiError::not_found("cover not available"));
	};
	let (bytes, mime) = fetch_remote(state, manga.source_id, &url, None).await?;
	let mime = mime.unwrap_or_else(|| COVER_DEFAULT_MIME.into());
	match cover_cache_write(&covers_dir, manga_id, &bytes, &mime).await {
		Ok(()) => cover_cache_enforce_limit(&covers_dir, state.config.cover_cache_max),
		Err(e) => tracing::warn!(manga_id, "cover cache write failed: {e}"),
	}
	Ok((bytes, Some(mime)))
}

fn cover_sidecar_path(cache_path: &Path) -> std::path::PathBuf {
	cache_path.with_file_name(format!(
		"{}.content-type",
		cache_path.file_name().unwrap_or_default().to_string_lossy()
	))
}

/// 缓存命中条件：文件存在且 manga 详情没在缓存写入之后重新拉过（last_fetched_at <= mtime）
async fn cover_cache_read(cache_path: &Path, last_fetched_at: Option<i64>) -> Option<(Vec<u8>, String)> {
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
	let mime = tokio::fs::read_to_string(cover_sidecar_path(cache_path))
		.await
		.ok()
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| COVER_DEFAULT_MIME.into());
	Some((bytes, mime))
}

/// 先写 tmp 再 rename，避免并发读到写了一半的缓存
async fn cover_cache_write(covers_dir: &Path, id: i64, bytes: &[u8], mime: &str) -> std::io::Result<()> {
	let tmp = covers_dir.join(format!(".{id}.tmp"));
	tokio::fs::write(&tmp, bytes).await?;
	tokio::fs::rename(&tmp, covers_dir.join(id.to_string())).await?;
	let tmp = covers_dir.join(format!(".{id}.content-type.tmp"));
	tokio::fs::write(&tmp, mime).await?;
	tokio::fs::rename(&tmp, covers_dir.join(format!("{id}.content-type"))).await?;
	Ok(())
}

/// LRU：缓存文件数超上限时按 mtime 最旧先删（连同 sidecar）
fn cover_cache_enforce_limit(covers_dir: &Path, max: usize) {
	let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
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
		let _ = std::fs::remove_file(cover_sidecar_path(&path));
		let _ = std::fs::remove_file(path);
	}
}

/// OPDS-PSE 页图取字节：流程同 page.rs（Url 走 fetch_remote、Zip 解包；Text/Image 报错）
pub async fn page_content(state: &Arc<AppState>, chapter_id: i64, index: i64) -> ApiResult<(Vec<u8>, Option<String>)> {
	let chapter = chapter_row(&state.pool, chapter_id).await?;
	let pages = load_pages(state, chapter_id, false).await?;
	let Some((_, content)) = pages.iter().find(|(idx, _)| *idx == index) else {
		return Err(ApiError::not_found("page not found"));
	};
	let stored: serde_json::Value = serde_json::from_str(content)?;
	let content = stored.get("content").cloned().unwrap_or(serde_json::Value::Null);
	if content.get("Text").is_some() {
		return Err(ApiError::bad_request("text page content cannot be streamed as image"));
	}
	// Image 是源内 canvas 句柄，M4 process_*_image 落地前无法在服务端还原成字节
	if content.get("Image").is_some() {
		return Err(ApiError::unsupported_501("image page content is not supported yet"));
	}
	let manga = manga_row(&state.pool, chapter.manga_id).await?;
	if let Some(zip) = content.get("Zip").and_then(serde_json::Value::as_array) {
		let Some(zip_url) = zip.first().and_then(serde_json::Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let Some(entry_path) = zip.get(1).and_then(serde_json::Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let (zip_bytes, _) = http_get(state, zip_url).await?;
		let entry = zip_entry_bytes(&zip_bytes, entry_path)?;
		return Ok((entry, Some(page_mime_for(entry_path).to_string())));
	}
	if let Some(url) = content.get("Url").and_then(serde_json::Value::as_array) {
		let Some(url_str) = url.first().and_then(serde_json::Value::as_str) else {
			return Err(ApiError::bad_request("invalid url page content"));
		};
		let context: Option<PageContext> = match url.get(1) {
			Some(value) if !value.is_null() => Some(serde_json::from_value(value.clone())?),
			_ => None,
		};
		return fetch_remote(state, manga.source_id, url_str, context).await;
	}
	Err(ApiError::bad_request("unknown page content"))
}

fn zip_entry_bytes(zip_bytes: &[u8], entry_path: &str) -> ApiResult<Vec<u8>> {
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

fn page_mime_for(path: &str) -> &'static str {
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
