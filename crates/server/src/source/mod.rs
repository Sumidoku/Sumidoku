pub mod cover;
pub mod rekey;
pub mod repo;
pub mod routed;

use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterRow, MangaRow, chapter_from_row, json_to_default_value, manga_from_row, unix_now,
};
use crate::state::AppState;
use aidoku::{Chapter, Manga, PageContext};
use aidoku_wasm::{DefaultValue, SourceHandle};
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
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
pub(crate) fn valid_source_key(key: &str) -> bool {
	!key.is_empty()
		&& key
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// 目录挂载扫描：一级子目录（source.json + main.wasm）与 .aix 包（zip，Payload/ 布局）都视为源包
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
	let mut candidates = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::from(sqlx::Error::Io(e)))?
	{
		let file_type = entry.file_type().await;
		let is_aix = entry
			.path()
			.extension()
			.is_some_and(|ext| ext.eq_ignore_ascii_case("aix"));
		if file_type.map(|t| t.is_dir()).unwrap_or(false) || is_aix {
			candidates.push(entry.path());
		}
	}
	candidates.sort();
	for path in candidates {
		let dir_name = path
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_else(|| path.display().to_string());
		let result = if path.is_dir() {
			scan_one_dir(pool, &path).await
		} else {
			register_aix(pool, &path).await
		};
		match result {
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

/// 清单解析结果与载荷位置；目录源与 .aix 共用同一注册路径
struct SourcePackage {
	doc: serde_json::Value,
	wasm_path: String,
	icon_path: Option<String>,
	settings: Option<serde_json::Value>,
}

async fn scan_one_dir(pool: &SqlitePool, path: &Path) -> Result<(String, bool), String> {
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
	let settings = read_dir_settings(path).await;
	let icon_path = path
		.join("icon.png")
		.is_file()
		.then(|| path.join("icon.png").display().to_string());
	register_source(
		pool,
		SourcePackage {
			doc,
			wasm_path: wasm_path.display().to_string(),
			icon_path,
			settings,
		},
	)
	.await
}

/// .aix 注册的统一入口：目录扫描与仓库安装共用；成功返回 (key, 是否更新既有源)
pub(crate) async fn register_aix(pool: &SqlitePool, path: &Path) -> Result<(String, bool), String> {
	let path_buf = path.to_path_buf();
	// zip 解析是同步 IO，丢给 blocking 线程池；包内容只进内存，不落地解压
	let (doc, settings, icon) = tokio::task::spawn_blocking(move || read_aix_package(&path_buf))
		.await
		.map_err(|e| format!("join: {e}"))??;
	// 包内嵌图标落盘到包旁，供 /sources/{id}/icon 读取（Aidoku 从解包目录读 icon.png）
	let icon_path = write_aix_icon(path, icon).await;
	register_source(
		pool,
		SourcePackage {
			doc,
			wasm_path: path.display().to_string(),
			icon_path,
			settings,
		},
	)
	.await
}

/// 把 .aix 的 `Payload/icon.png` 写到包旁的 `<name>.png`；无图标时清掉遗留文件。
/// 写失败只告警，不阻断注册
async fn write_aix_icon(aix_path: &Path, icon: Option<Vec<u8>>) -> Option<String> {
	let icon_path = aix_path.with_extension("png");
	let Some(bytes) = icon else {
		let _ = tokio::fs::remove_file(&icon_path).await;
		return None;
	};
	if let Err(e) = tokio::fs::write(&icon_path, &bytes).await {
		tracing::warn!(path = %aix_path.display(), "write source icon failed: {e}");
		return None;
	}
	Some(icon_path.display().to_string())
}

type AixPackage = (serde_json::Value, Option<serde_json::Value>, Option<Vec<u8>>);

fn read_aix_package(path: &Path) -> Result<AixPackage, String> {
	use aidoku_wasm::package::{
		ICON_ENTRY, ICON_ENTRY_MAX, JSON_ENTRY_MAX, MANIFEST_ENTRY, SETTINGS_ENTRY, WASM_ENTRY,
		WASM_PAYLOAD_MAX, read_optional_zip_entry, read_zip_entry,
	};
	let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
	let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("invalid archive: {e}"))?;
	let raw = read_zip_entry(&mut archive, MANIFEST_ENTRY, JSON_ENTRY_MAX)?;
	let doc: serde_json::Value =
		serde_json::from_slice(&raw).map_err(|e| format!("parse {MANIFEST_ENTRY}: {e}"))?;
	// 载荷必须真实读得出（顺带校验 CRC），扫描不留存字节
	read_zip_entry(&mut archive, WASM_ENTRY, WASM_PAYLOAD_MAX)?;
	// 图标可选：老包或纯 wasm 包可能没有
	let icon = read_optional_zip_entry(&mut archive, ICON_ENTRY, ICON_ENTRY_MAX)?;
	let settings = read_zip_entry(&mut archive, SETTINGS_ENTRY, JSON_ENTRY_MAX)
		.ok()
		.and_then(|raw| match serde_json::from_slice(&raw) {
			Ok(settings) => Some(settings),
			Err(e) => {
				tracing::warn!(path = %path.display(), "skip unparsable settings.json: {e}");
				None
			}
		});
	Ok((doc, settings, icon))
}

async fn read_dir_settings(path: &Path) -> Option<serde_json::Value> {
	let raw = tokio::fs::read(path.join("settings.json")).await.ok()?;
	match serde_json::from_slice(&raw) {
		Ok(settings) => Some(settings),
		Err(e) => {
			tracing::warn!(path = %path.display(), "skip unparsable settings.json: {e}");
			None
		}
	}
}

/// 成功返回 (key, 是否更新既有源)
async fn register_source(pool: &SqlitePool, package: SourcePackage) -> Result<(String, bool), String> {
	let doc = &package.doc;
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
	// Aidoku 的 breakingChangeVersion 在 manifest 的 config 节（SourceInfo.Configuration）
	let breaking_change_version = doc
		.get("config")
		.and_then(|c| c.get("breakingChangeVersion"))
		.and_then(|v| v.as_i64())
		.unwrap_or(0);
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
	let icon_path = package.icon_path;
	let wasm = package.wasm_path;
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
			 base_url = ?, breaking_change_version = ?, wasm_path = ?, icon_path = ?, updated_at = ? WHERE id = ?",
		)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(breaking_change_version)
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
			 breaking_change_version, wasm_path, icon_path, enabled, installed_at, updated_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
		)
		.bind(&key)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(breaking_change_version)
		.bind(wasm)
		.bind(icon_path)
		.bind(now)
		.bind(now)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
		(result.last_insert_rowid(), false)
	};
	seed_source_defaults(pool, id, package.settings.as_ref(), doc).await?;
	Ok((key, updated))
}

/// 对齐 Aidoku 安装源时的 register(defaults:) 语义：播种语言列表与 settings.json 默认值。
/// 源运行时会 defaults.get 这些键（如 mangadex 的 contentRating），缺了直接报错
pub(crate) async fn seed_source_defaults(
	pool: &SqlitePool,
	source_id: i64,
	settings: Option<&serde_json::Value>,
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
		let urls = source_urls(doc);
		if urls.len() > 1 {
			seeds.push(("url".to_string(), DefaultValue::String(urls[0].clone())));
		}
	}
	if let Some(settings) = settings {
		collect_setting_defaults(settings, &mut seeds);
	}
	seed_defaults(pool, source_id, seeds).await
}

/// 成功 `get_settings` 后调：把源在代码里声明的默认值播种进库，对齐 AidokuRunner
/// 在 getSettings() 里 `register(defaults:)` 的行为
pub(crate) async fn seed_dynamic_defaults(
	pool: &SqlitePool,
	source_id: i64,
	settings: &[serde_json::Value],
) -> Result<(), String> {
	let mut seeds = Vec::new();
	collect_dynamic_defaults(settings, &mut seeds);
	seed_defaults(pool, source_id, seeds).await
}

/// 播种默认值：行不存在直接种；已存在时仅当现存值仍等于上次播种的默认才写新默认
/// （UserDefaults register(defaults:) 的物化版——没设过的键永远解析到当前默认），
/// 用户改过的值保留。迁移前入库的行 seeded_value 为 NULL，一律按用户值保守处理
async fn seed_defaults(
	pool: &SqlitePool,
	source_id: i64,
	seeds: Vec<(String, DefaultValue)>,
) -> Result<(), String> {
	for (key, value) in seeds {
		let raw = serde_json::to_string(&value).map_err(|e| format!("encode default: {e}"))?;
		sqlx::query(
			"INSERT INTO source_setting (source_id, key, value, seeded_value) VALUES (?, ?, ?, ?)
			 ON CONFLICT (source_id, key) DO UPDATE SET
			 value = CASE WHEN source_setting.value IS source_setting.seeded_value THEN excluded.value
			              ELSE source_setting.value END,
			 seeded_value = excluded.seeded_value",
		)
		.bind(source_id)
		.bind(&key)
		.bind(&raw)
		.bind(&raw)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
	}
	Ok(())
}

/// 源声明的 base url 列表：info.url 在前，info.urls 按顺序追加（对齐 AidokuRunner 的 urls 组装）
pub(crate) fn source_urls(doc: &serde_json::Value) -> Vec<String> {
	let info = doc.get("info");
	let mut urls: Vec<String> = info
		.and_then(|i| i.get("url"))
		.and_then(|v| v.as_str())
		.map(str::to_string)
		.into_iter()
		.collect();
	if let Some(more) = info.and_then(|i| i.get("urls")).and_then(|v| v.as_array()) {
		urls.extend(more.iter().filter_map(|u| u.as_str()).map(str::to_string));
	}
	urls
}

/// 源声明的语言列表：(code, isDefault)，按声明顺序；字符串项无 isDefault 标记
pub(crate) fn source_languages(doc: &serde_json::Value) -> Vec<(String, bool)> {
	let Some(langs) = doc
		.get("info")
		.and_then(|i| i.get("languages"))
		.and_then(|v| v.as_array())
	else {
		return Vec::new();
	};
	let mut codes: Vec<(String, bool)> = Vec::new();
	for lang in langs {
		match lang {
			serde_json::Value::String(code) => codes.push((code.clone(), false)),
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
					codes.push((code.to_string(), is_default));
				}
			}
			_ => {}
		}
	}
	codes
}

/// languageSelectType == "single" 时语言设置是单选（键 language），否则多选（键 languages）
pub(crate) fn language_select_single(doc: &serde_json::Value) -> bool {
	doc.get("config")
		.and_then(|c| c.get("languageSelectType"))
		.and_then(|v| v.as_str())
		== Some("single")
}

/// 服务端没有设备语言，默认退化为 isDefault 优先，都没有取第一个
/// （AidokuRunner 按设备语言取交集，差异记录在 docs/architecture.md）
pub(crate) fn default_language_pick(langs: &[(String, bool)]) -> Vec<String> {
	let picked: Vec<String> = langs
		.iter()
		.filter(|(_, is_default)| *is_default)
		.map(|(code, _)| code.clone())
		.collect();
	if picked.is_empty() {
		langs
			.first()
			.map(|(code, _)| vec![code.clone()])
			.unwrap_or_default()
	} else {
		picked
	}
}

/// 多语言源的语言设置种子：>1 种语言才存在该设置；键与值类型随 languageSelectType
/// （multiple → languages: StringArray，single → language: String）。
fn default_languages(doc: &serde_json::Value) -> Option<(String, DefaultValue)> {
	let langs = source_languages(doc);
	if langs.len() < 2 {
		return None;
	}
	let picked = default_language_pick(&langs);
	if language_select_single(doc) {
		Some(("language".to_string(), DefaultValue::String(picked[0].clone())))
	} else {
		Some(("languages".to_string(), DefaultValue::StringArray(picked)))
	}
}

fn collect_setting_defaults(
	items: &serde_json::Value,
	out: &mut Vec<(String, DefaultValue)>,
) {
	let Some(items) = items.as_array() else { return };
	for item in items {
		let ty = item.get("type").and_then(|v| v.as_str());
		// group 与 page 都是容器：AidokuRunner 的 loadSettingsDefaults 两者都递归
		if matches!(ty, Some("group") | Some("page")) {
			if let Some(children) = item.get("items") {
				collect_setting_defaults(children, out);
			}
			continue;
		}
		// 对齐 AidokuRunner loadSettingsDefaults：显式 default 优先（null 视同缺失）；
		// 缺失时 select/picker 落第一个候选值，switch 落 false，其余不播种
		let default = item
			.get("default")
			.filter(|v| !v.is_null())
			.cloned()
			.or_else(|| match ty {
				Some("select") | Some("picker") => item
					.get("values")
					.and_then(|v| v.as_array())
					.and_then(|v| v.first())
					.cloned(),
				Some("switch") => Some(serde_json::Value::Bool(false)),
				_ => None,
			});
		let (Some(key), Some(default)) =
			(item.get("key").and_then(|v| v.as_str()), default)
		else {
			continue;
		};
		// Null 种下后 defaults.get 一样是无效值，不浪费一行
		let value = json_to_default_value(&default);
		if !matches!(value, DefaultValue::Null) {
			out.push((key.to_string(), value));
		}
	}
}

/// 动态设置（`get_settings` 导出的嵌套 shape）的默认值，规则同 `collect_setting_defaults`：
/// group/page 递归，select/picker 无显式默认落第一个候选值，其余只在有 default 时取
fn collect_dynamic_defaults(
	items: &[serde_json::Value],
	out: &mut Vec<(String, DefaultValue)>,
) {
	for item in items {
		let Some(value) = item.get("value") else { continue };
		if let Some(container) = value.get("Group").or_else(|| value.get("Page")) {
			if let Some(children) = container.get("items").and_then(|v| v.as_array()) {
				collect_dynamic_defaults(children, out);
			}
			continue;
		}
		let Some(key) = item.get("key").and_then(|v| v.as_str()) else {
			continue;
		};
		let Some(default) = dynamic_default(value) else {
			continue;
		};
		let value = json_to_default_value(&default);
		if !matches!(value, DefaultValue::Null) {
			out.push((key.to_string(), value));
		}
	}
}

/// 一个动态设置项的默认值；无显式默认时 select/picker 退化为第一个候选值
fn dynamic_default(value: &serde_json::Value) -> Option<serde_json::Value> {
	let (kind, body) = value.as_object()?.iter().next()?;
	if let Some(default) = body.get("default").filter(|v| !v.is_null()) {
		return Some(default.clone());
	}
	match kind.as_str() {
		"Select" | "Picker" => body
			.get("values")
			.and_then(|v| v.as_array())
			.and_then(|v| v.first())
			.cloned(),
		_ => None,
	}
}

/// 按 (source_id, key) upsert 详情字段；时间戳由调用方按需打点。
/// 收连接而不是连接池：迁移等场景要把 upsert 包进调用方自己的事务
pub async fn upsert_manga(
	conn: &mut sqlx::SqliteConnection,
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
		 content_rating, viewer, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT (source_id, key) DO UPDATE SET
		 title = excluded.title, authors = excluded.authors, artists = excluded.artists,
		 description = excluded.description, tags = excluded.tags, status = excluded.status,
		 content_rating = excluded.content_rating, viewer = excluded.viewer,
		 cover_url = excluded.cover_url,
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
	.bind(manga.viewer as i64)
	.bind(&manga.cover)
	.bind(&manga.url)
	.bind(initialized)
	.bind(manga.update_strategy as i64)
	.bind(manga.next_update_time)
	.bind(if fetched_details { Some(now) } else { None })
	.bind(if fetched_chapters { Some(now) } else { None })
	.execute(&mut *conn)
	.await?;

	let (id,): (i64,) = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(source_id)
		.bind(&manga.key)
		.fetch_one(&mut *conn)
		.await?;
	if fetched_details {
		sqlx::query("UPDATE manga SET last_fetched_at = ? WHERE id = ?")
			.bind(now)
			.bind(id)
			.execute(&mut *conn)
			.await?;
	}
	if fetched_chapters {
		sqlx::query("UPDATE manga SET chapters_last_fetched_at = ? WHERE id = ?")
			.bind(now)
			.bind(id)
			.execute(&mut *conn)
			.await?;
	}
	Ok(id)
}

/// 章节对账：source_order 取源返回下标；消失的章节有 download 行则锁定保留，否则删除。
/// 返回本次对账新插入的 chapter id（更新检测据此判定新增章节）
pub async fn reconcile_chapters(
	conn: &mut sqlx::SqliteConnection,
	manga_id: i64,
	chapters: &[Chapter],
) -> ApiResult<Vec<i64>> {
	let now = unix_now();
	// 源恰好一种语言时给缺语言的章节补上（对齐 AidokuRunner）
	let fill_language: Option<String> =
		sqlx::query_as::<_, (sqlx::types::Json<Vec<String>>,)>(
			"SELECT s.languages FROM manga m JOIN source s ON s.id = m.source_id WHERE m.id = ?",
		)
		.bind(manga_id)
		.fetch_optional(&mut *conn)
		.await?
		.and_then(|(languages,)| match languages.0.as_slice() {
			[only] => Some(only.clone()),
			_ => None,
		});
	let existing: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM chapter WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_all(&mut *conn)
		.await?;
	let known: HashSet<&String> = existing.iter().map(|(_, key)| key).collect();
	let mut seen = HashSet::with_capacity(chapters.len());
	let mut new_keys = Vec::new();
	for (index, chapter) in chapters.iter().enumerate() {
		if seen.insert(chapter.key.clone()) && !known.contains(&chapter.key) {
			new_keys.push(chapter.key.clone());
		}
		let scanlators = serde_json::to_string(&chapter.scanlators.clone().unwrap_or_default())?;
		let language = chapter
			.language
			.as_ref()
			.filter(|l| !l.is_empty())
			.cloned()
			.or_else(|| fill_language.clone());
		sqlx::query(
			"INSERT INTO chapter (manga_id, key, title, number, volume, scanlators, language, thumbnail, url,
			 date_uploaded, source_order, locked, fetched_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
			 ON CONFLICT (manga_id, key) DO UPDATE SET
			 title = excluded.title, number = excluded.number, volume = excluded.volume,
			 scanlators = excluded.scanlators, language = excluded.language,
			 thumbnail = excluded.thumbnail, url = excluded.url,
			 date_uploaded = excluded.date_uploaded, source_order = excluded.source_order,
			 locked = excluded.locked, fetched_at = excluded.fetched_at",
		)
		.bind(manga_id)
		.bind(&chapter.key)
		.bind(&chapter.title)
		.bind(chapter.chapter_number.map(|n| n as f64))
		.bind(chapter.volume_number.map(|n| n as f64))
		.bind(scanlators)
		.bind(language)
		.bind(&chapter.thumbnail)
		.bind(&chapter.url)
		.bind(chapter.date_uploaded)
		.bind(index as i64)
		.bind(chapter.locked)
		.bind(now)
		.execute(&mut *conn)
		.await?;
	}
	for (chapter_id, key) in existing {
		if seen.contains(&key) {
			continue;
		}
		let has_download: Option<(i64,)> =
			sqlx::query_as("SELECT 1 FROM download WHERE chapter_id = ? LIMIT 1")
				.bind(chapter_id)
				.fetch_optional(&mut *conn)
				.await?;
		if has_download.is_some() {
			sqlx::query("UPDATE chapter SET locked = 1 WHERE id = ?")
				.bind(chapter_id)
				.execute(&mut *conn)
				.await?;
		} else {
			sqlx::query("DELETE FROM chapter WHERE id = ?")
				.bind(chapter_id)
				.execute(&mut *conn)
				.await?;
		}
	}
	let mut new_ids = Vec::with_capacity(new_keys.len());
	for key in new_keys {
		let (id,): (i64,) = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
			.bind(manga_id)
			.bind(&key)
			.fetch_one(&mut *conn)
			.await?;
		new_ids.push(id);
	}
	Ok(new_ids)
}

pub async fn manga_row(pool: &SqlitePool, id: i64) -> ApiResult<MangaRow> {
	let row: Option<MangaRow> = sqlx::query_as(
		"SELECT id, source_id, key, title, authors, artists, description, tags, status,
		 content_rating, viewer, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at FROM manga WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("manga not found"))
}

/// 列表端点批量取行：一条 IN 查询替代逐行 manga_row
pub async fn manga_rows_by_ids(pool: &SqlitePool, ids: &[i64]) -> ApiResult<HashMap<i64, MangaRow>> {
	if ids.is_empty() {
		return Ok(HashMap::new());
	}
	let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
	let sql = format!(
		"SELECT id, source_id, key, title, authors, artists, description, tags, status,
		 content_rating, viewer, cover_url, url, initialized, update_strategy, next_update_time,
		 last_fetched_at, chapters_last_fetched_at FROM manga WHERE id IN ({placeholders})"
	);
	let mut q = sqlx::query_as::<_, MangaRow>(&sql);
	for id in ids {
		q = q.bind(id);
	}
	Ok(q
		.fetch_all(pool)
		.await?
		.into_iter()
		.map(|row| (row.id, row))
		.collect())
}

pub async fn chapter_row(pool: &SqlitePool, id: i64) -> ApiResult<ChapterRow> {
	let row: Option<ChapterRow> = sqlx::query_as(
		"SELECT id, manga_id, key, title, number, volume, scanlators, language, thumbnail, url,
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
	let contents = materialize_image_pages(state, &handle, chapter_id, &pages).await?;
	{
		let mut tx = state.pool.begin().await?;
		sqlx::query("DELETE FROM page WHERE chapter_id = ?")
			.bind(chapter_id)
			.execute(&mut *tx)
			.await?;
		for (index, content) in contents.iter().enumerate() {
			let page = &pages[index];
			let stored = serde_json::json!({
				"content": content,
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

/// Image 页物化：RID 只在当前 actor 会话里有效，字节落 blob 文件后 JSON 只存
/// 文件名（{"Image": "<idx>.png"}），这是重启与 actor 重建后仍能取页的唯一形态。
/// 任一页取字节失败整章失败，blob 目录下轮的整章重写会清掉半成品
async fn materialize_image_pages(
	state: &Arc<AppState>,
	handle: &SourceHandle,
	chapter_id: i64,
	pages: &[aidoku::Page],
) -> ApiResult<Vec<serde_json::Value>> {
	let dir = state.config.page_blobs_dir().join(chapter_id.to_string());
	let _ = tokio::fs::remove_dir_all(&dir).await;
	let mut contents = Vec::with_capacity(pages.len());
	let mut dir_ready = false;
	for (index, page) in pages.iter().enumerate() {
		let mut content = serde_json::to_value(&page.content)?;
		if let Some(rid) = content.get("Image").and_then(serde_json::Value::as_i64) {
			let bytes = handle.take_page_image(rid as i32).await?;
			if !dir_ready {
				tokio::fs::create_dir_all(&dir)
					.await
					.map_err(|e| ApiError::internal(format!("create page blob dir: {e}")))?;
				dir_ready = true;
			}
			let name = format!("{index}.png");
			tokio::fs::write(dir.join(&name), &bytes)
				.await
				.map_err(|e| ApiError::internal(format!("write page blob: {e}")))?;
			content = serde_json::json!({"Image": name});
		}
		contents.push(content);
	}
	Ok(contents)
}

/// 物化过的 Image 页读字节：blob 文件名只由 load_pages 写入（<idx>.png），
/// 但仍按文件名基元校验一遍防路径逃逸
pub async fn page_image_blob(state: &Arc<AppState>, chapter_id: i64, name: &str) -> ApiResult<Vec<u8>> {
	if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') {
		return Err(ApiError::bad_request("invalid page blob name"));
	}
	let path = state
		.config
		.page_blobs_dir()
		.join(chapter_id.to_string())
		.join(name);
	tokio::fs::read(&path)
		.await
		.map_err(|_| ApiError::not_found("page blob not found"))
}

/// 启动清扫：chapter 行没了的 blob 目录整棵删掉（章节对账删除后只剩这里能回收）
pub async fn sweep_page_blobs(pool: &SqlitePool, dir: &Path) {
	let mut read_dir = match tokio::fs::read_dir(dir).await {
		Ok(read_dir) => read_dir,
		Err(_) => return,
	};
	while let Ok(Some(entry)) = read_dir.next_entry().await {
		let Some(id) = entry.file_name().to_str().and_then(|n| n.parse::<i64>().ok()) else {
			continue;
		};
		let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM chapter WHERE id = ? LIMIT 1")
			.bind(id)
			.fetch_optional(pool)
			.await
			.unwrap_or(None);
		if exists.is_none() {
			let _ = tokio::fs::remove_dir_all(entry.path()).await;
		}
	}
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
pub async fn http_get(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	let url = reqwest::Url::parse(url).map_err(|_| ApiError::bad_request("invalid upstream url"))?;
	let handle = state.get_handle(source_id).await?;
	let accept_language = handle.accept_language();
	let response = state
		.http
		.get_with(&url, Some(accept_language.as_str()))
		.await
		.map_err(fetch_err)?;
	read_body(response).await
}

/// 图像取回的完整结果：字节之外带状态与头，process_*_image 构造 ImageResponse 要用
pub struct RemoteImage {
	pub bytes: Vec<u8>,
	pub mime: Option<String>,
	pub status: u16,
	pub response_headers: Vec<(String, String)>,
	pub request_headers: Vec<(String, String)>,
}

fn flatten_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
	let mut flat: Vec<(String, String)> = headers
		.iter()
		.filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
		.collect();
	flat.sort();
	flat.dedup();
	flat
}

/// 按源特性拉图：有 image_request 用源给的 spec 发请求，否则裸 GET
pub async fn fetch_remote(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
	context: Option<PageContext>,
) -> ApiResult<RemoteImage> {
	let handle = state.get_handle(source_id).await?;
	let accept_language = handle.accept_language();
	if !handle.features().image_request {
		let parsed = reqwest::Url::parse(url).map_err(|_| ApiError::bad_request("invalid upstream url"))?;
		let response = state
			.http
			.get_with(&parsed, Some(accept_language.as_str()))
			.await
			.map_err(fetch_err)?;
		let status = response.status().as_u16();
		let response_headers = flatten_headers(response.headers());
		let (bytes, mime) = read_body(response).await?;
		return Ok(RemoteImage {
			bytes,
			mime,
			status,
			response_headers,
			request_headers: Vec::new(),
		});
	}
	let spec = handle.get_image_request(url.to_string(), context).await?;
	let spec_url =
		reqwest::Url::parse(&spec.url).map_err(|_| ApiError::bad_request("invalid image url"))?;
	let mut builder = state
		.http
		.request_with(
			spec.method.into(),
			&spec_url,
			&spec.headers,
			Some(accept_language.as_str()),
		)
		.map_err(fetch_err)?;
	if let Some(body) = &spec.body {
		builder = builder.body(body.clone());
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
	let status = response.status().as_u16();
	let response_headers = flatten_headers(response.headers());
	let (bytes, mime) = read_body(response).await?;
	Ok(RemoteImage {
		bytes,
		mime,
		status,
		response_headers,
		request_headers: spec.headers,
	})
}

/// 处理器输出统一编码成 PNG（无损且源侧解码链路已有）
const PROCESSED_MIME: &str = "image/png";

/// 页图拉取：fetch_remote 之后源有 page_image_processor 就过一道处理
pub async fn fetch_page_image(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
	context: Option<PageContext>,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	let handle = state.get_handle(source_id).await?;
	if !handle.features().page_image_processor {
		let image = fetch_remote(state, source_id, url, context).await?;
		return Ok((image.bytes, image.mime));
	}
	let image = fetch_remote(state, source_id, url, context.clone()).await?;
	let bytes = handle
		.process_page_image(process_input(image, url), context)
		.await?;
	Ok((bytes, Some(PROCESSED_MIME.into())))
}

/// 封面拉取：同上，走 cover_image_processor
pub async fn fetch_cover_image(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	let handle = state.get_handle(source_id).await?;
	if !handle.features().cover_image_processor {
		let image = fetch_remote(state, source_id, url, None).await?;
		return Ok((image.bytes, image.mime));
	}
	let image = fetch_remote(state, source_id, url, None).await?;
	let bytes = handle.process_cover_image(process_input(image, url)).await?;
	Ok((bytes, Some(PROCESSED_MIME.into())))
}

fn process_input(image: RemoteImage, url: &str) -> aidoku_wasm::ProcessImageInput {
	aidoku_wasm::ProcessImageInput {
		bytes: image.bytes,
		url: url.to_string(),
		status: image.status,
		response_headers: image.response_headers,
		request_headers: image.request_headers,
	}
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
	let (bytes, mime) = fetch_cover_image(state, manga.source_id, &url).await?;
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

/// OPDS-PSE 页图取字节：流程同 page.rs（Url 走 fetch_page_image、Zip 解包、Image 读物化 blob）
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
	if let Some(name) = content.get("Image").and_then(serde_json::Value::as_str) {
		let bytes = page_image_blob(state, chapter_id, name).await?;
		return Ok((bytes, Some(PROCESSED_MIME.into())));
	}
	let manga = manga_row(&state.pool, chapter.manga_id).await?;
	if let Some(zip) = content.get("Zip").and_then(serde_json::Value::as_array) {
		let Some(zip_url) = zip.first().and_then(serde_json::Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let Some(entry_path) = zip.get(1).and_then(serde_json::Value::as_str) else {
			return Err(ApiError::bad_request("invalid zip page content"));
		};
		let (zip_bytes, _) = http_get(state, manga.source_id, zip_url).await?;
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
		return fetch_page_image(state, manga.source_id, url_str, context).await;
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

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{Value, json};

	fn as_json(value: &DefaultValue) -> Value {
		crate::model::default_value_to_json(value)
	}

	fn flat_defaults(raw: &Value) -> std::collections::HashMap<String, Value> {
		let mut seeds = Vec::new();
		collect_setting_defaults(raw, &mut seeds);
		seeds
			.into_iter()
			.map(|(key, value)| (key, as_json(&value)))
			.collect()
	}

	fn dynamic_defaults(raw: &Value) -> std::collections::HashMap<String, Value> {
		let mut seeds = Vec::new();
		collect_dynamic_defaults(raw.as_array().expect("array"), &mut seeds);
		seeds
			.into_iter()
			.map(|(key, value)| (key, as_json(&value)))
			.collect()
	}

	#[test]
	fn flat_defaults_recurse_page_and_match_runner_rules() {
		let raw = json!([{"type": "group", "title": "G", "items": [
			{"type": "switch", "key": "plain", "title": "S"},
			{"type": "select", "key": "sel", "title": "Sel", "values": ["a", "b"]},
			{"type": "multi-select", "key": "multi", "title": "M", "values": ["x", "y"]},
			{"type": "page", "title": "P", "items": [
				{"type": "group", "items": [
					{"type": "switch", "key": "nested", "title": "N", "default": true}
				]}
			]},
			{"type": "picker", "key": "pick", "title": "Pk", "values": ["dark", "light"]},
			{"type": "segment", "key": "seg", "title": "Sg", "options": ["a", "b"]}
		]}]);
		let m = flat_defaults(&raw);
		assert_eq!(m.get("plain").cloned(), Some(json!(false)));
		assert_eq!(m.get("sel").cloned(), Some(json!("a")));
		assert!(!m.contains_key("multi"));
		assert_eq!(m.get("nested").cloned(), Some(json!(true)));
		assert_eq!(m.get("pick").cloned(), Some(json!("dark")));
		assert!(!m.contains_key("seg"));
	}

	#[test]
	fn dynamic_defaults_recurse_page_and_match_runner_rules() {
		let raw = json!([{"type": "group", "key": "g", "title": "G", "value": {"Group": {"footer": null, "items": [
			{"type": "select", "key": "sel", "title": "Sel", "value": {"Select": {"values": ["a", "b"], "titles": null, "auth_to_open": null, "default": null}}},
			{"type": "select", "key": "sel2", "title": "Sel2", "value": {"Select": {"values": ["a", "b"], "titles": null, "auth_to_open": null, "default": "b"}}},
			{"type": "multi-select", "key": "multi", "title": "M", "value": {"MultiSelect": {"values": ["x", "y"], "titles": null, "auth_to_open": null, "default": null}}},
			{"type": "toggle", "key": "tog", "title": "T", "value": {"Toggle": {"subtitle": null, "auth_to_disable": null, "default": false}}},
			{"type": "picker", "key": "pick", "title": "Pk", "value": {"Picker": {"values": ["dark", "light"], "titles": null, "default": null}}},
			{"type": "page", "key": "p", "title": "P", "value": {"Page": {"items": [
				{"type": "group", "key": "ig", "title": "I", "value": {"Group": {"footer": null, "items": [
					{"type": "switch", "key": "nested", "title": "N", "value": {"Toggle": {"subtitle": null, "auth_to_disable": null, "default": true}}}
				]}}}
			], "inline_title": null, "auth_to_open": null, "icon": null, "info": null}}},
			{"type": "segment", "key": "seg", "title": "Sg", "value": {"Segment": {"options": ["a", "b"], "default": null}}}
		]}}}]);
		let m = dynamic_defaults(&raw);
		assert_eq!(m.get("sel").cloned(), Some(json!("a")));
		assert_eq!(m.get("sel2").cloned(), Some(json!("b")));
		assert!(!m.contains_key("multi"));
		assert_eq!(m.get("tog").cloned(), Some(json!(false)));
		assert_eq!(m.get("pick").cloned(), Some(json!("dark")));
		assert_eq!(m.get("nested").cloned(), Some(json!(true)));
		assert!(!m.contains_key("seg"));
	}
}
