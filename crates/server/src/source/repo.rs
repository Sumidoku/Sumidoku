//! Aidoku 式源仓库：index JSON 的拉取/缓存/解析，跨 repo 浏览与 .aix 安装。
//! schema 对齐上游 Aidoku app（Aidoku/Core/Sources/SourceList/）：index 为
//! {name, feedbackURL?, sources:[ExternalSourceInfo]}；version 是递增整数，
//! downloadURL/iconURL 可按 RFC3986 相对 index URL 解析；lang/nsfw/file/icon
//! 是旧版废弃字段（file→sources/<file>，icon→icons/<icon>），解析时一并兼容

use crate::error::{ApiError, ApiResult};
use crate::model::{SourceRow, unix_now};
use crate::source::routed::RoutedClients;
use crate::state::AppState;
use aidoku_wasm::package::WASM_PAYLOAD_MAX;
use reqwest::Url;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// 真实 index 约百 KiB，4 MiB 已留足余量
const INDEX_MAX: u64 = 4 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 对外归一：拉取/解析失败不区分原因，细节只进日志
const INDEX_FETCH_MSG: &str = "failed to fetch repository index";
const PACKAGE_FETCH_MSG: &str = "failed to download source package";

#[derive(Debug, sqlx::FromRow)]
pub struct SourceRepoRow {
	pub id: i64,
	pub url: String,
	pub name: Option<String>,
	pub index_json: Option<String>,
	pub fetch_error: Option<String>,
	pub added_at: i64,
	pub fetched_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoIndex {
	name: Option<String>,
	sources: Vec<IndexSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexSource {
	id: String,
	name: Option<String>,
	version: i64,
	// Aidoku 的 URL 字段全大写缩写，camelCase 推导不出
	#[serde(rename = "iconURL")]
	icon_url: Option<String>,
	#[serde(rename = "downloadURL")]
	download_url: Option<String>,
	languages: Option<Vec<String>>,
	content_rating: Option<i64>,
	alt_names: Option<Vec<String>>,
	#[serde(rename = "baseURL")]
	base_url: Option<String>,
	lang: Option<String>,
	nsfw: Option<i64>,
	file: Option<String>,
	icon: Option<String>,
}

/// 解析后的仓库源条目：URL 已相对 repo 地址解析成绝对地址
#[derive(Debug, Clone)]
pub struct RepoEntry {
	pub key: String,
	pub name: String,
	pub version: i64,
	pub languages: Vec<String>,
	pub content_rating: i64,
	pub icon_url: Option<String>,
	pub download_url: Option<String>,
	pub base_url: Option<String>,
	pub alt_names: Vec<String>,
}

pub struct ParsedIndex {
	pub name: Option<String>,
	pub entries: Vec<RepoEntry>,
}

fn resolve_url(base: &Url, raw: Option<&str>, legacy: Option<&str>, legacy_dir: &str) -> Option<String> {
	let raw = raw.map(str::to_string).or_else(|| {
		legacy.map(|file| format!("{legacy_dir}{file}"))
	})?;
	base.join(&raw).ok().map(|u| u.to_string())
}

/// 解析并校验 index 原文；URL 相对 base 解析。sources 数组缺失视为坏 index
pub fn parse_index(raw: &[u8], base: &Url) -> Result<ParsedIndex, String> {
	let index: RepoIndex = serde_json::from_slice(raw).map_err(|e| format!("parse index: {e}"))?;
	let entries = index
		.sources
		.into_iter()
		.map(|s| RepoEntry {
			name: s.name.clone().unwrap_or_else(|| s.id.clone()),
			key: s.id,
			version: s.version,
			languages: s
				.languages
				.or_else(|| s.lang.map(|l| vec![l]))
				.unwrap_or_default(),
			content_rating: s.content_rating.or(s.nsfw).unwrap_or(0),
			icon_url: resolve_url(base, s.icon_url.as_deref(), s.icon.as_deref(), "icons/"),
			download_url: resolve_url(base, s.download_url.as_deref(), s.file.as_deref(), "sources/"),
			base_url: s.base_url,
			alt_names: s.alt_names.unwrap_or_default(),
		})
		.collect();
	Ok(ParsedIndex {
		name: index.name,
		entries,
	})
}

/// repo URL 只许 http/https：self-hosted 用户会指内网地址，不做 IP 判定
pub fn parse_repo_url(raw: &str) -> ApiResult<Url> {
	let url = Url::parse(raw).map_err(|_| ApiError::bad_request("invalid repository url"))?;
	if !matches!(url.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("repository url must be http or https"));
	}
	Ok(url)
}

/// 带总超时与大小上限的 GET；错误细节只给日志，对外一律归一
async fn fetch_capped(http: &RoutedClients, url: &Url, max: u64) -> Result<Vec<u8>, String> {
	let fetch = async {
		let mut response = http.get(url).await.map_err(|e| format!("request: {e}"))?;
		if let Some(len) = response.content_length()
			&& len > max
		{
			return Err(format!("body exceeds {max} bytes"));
		}
		let mut out = Vec::new();
		while let Some(chunk) = response.chunk().await.map_err(|e| format!("read body: {e}"))? {
			if out.len() as u64 + chunk.len() as u64 > max {
				return Err(format!("body exceeds {max} bytes"));
			}
			out.extend_from_slice(&chunk);
		}
		Ok(out)
	};
	match tokio::time::timeout(FETCH_TIMEOUT, fetch).await {
		Ok(result) => result,
		Err(_) => Err("request timed out".into()),
	}
}

async fn fetch_index(state: &AppState, url: &Url) -> Result<(String, ParsedIndex), String> {
	let raw = fetch_capped(&state.http, url, INDEX_MAX).await?;
	let parsed = parse_index(&raw, url)?;
	Ok((String::from_utf8_lossy(&raw).into_owned(), parsed))
}

pub async fn repo_row(pool: &SqlitePool, id: i64) -> ApiResult<SourceRepoRow> {
	let row: Option<SourceRepoRow> = sqlx::query_as(
		"SELECT id, url, name, index_json, fetch_error, added_at, fetched_at FROM source_repo WHERE id = ?",
	)
	.bind(id)
	.fetch_optional(pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("repository not found"))
}

pub async fn list_repos(pool: &SqlitePool) -> ApiResult<Vec<SourceRepoRow>> {
	let rows = sqlx::query_as(
		"SELECT id, url, name, index_json, fetch_error, added_at, fetched_at FROM source_repo ORDER BY id",
	)
	.fetch_all(pool)
	.await?;
	Ok(rows)
}

pub async fn add_repo(state: &Arc<AppState>, url: Url) -> ApiResult<SourceRepoRow> {
	let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM source_repo WHERE url = ?")
		.bind(url.as_str())
		.fetch_optional(&state.pool)
		.await?;
	if existing.is_some() {
		return Err(ApiError::duplicate("repository already added"));
	}
	let (raw, parsed) = fetch_index(state, &url).await.map_err(|detail| {
		tracing::warn!(url = %url, "add repository failed: {detail}");
		ApiError::bad_gateway(INDEX_FETCH_MSG)
	})?;
	let now = unix_now();
	let result = sqlx::query(
		"INSERT INTO source_repo (url, name, index_json, added_at, fetched_at) VALUES (?, ?, ?, ?, ?)",
	)
	.bind(url.as_str())
	.bind(&parsed.name)
	.bind(&raw)
	.bind(now)
	.bind(now)
	.execute(&state.pool)
	.await?;
	repo_row(&state.pool, result.last_insert_rowid()).await
}

pub async fn delete_repo(state: &Arc<AppState>, id: i64) -> ApiResult<()> {
	repo_row(&state.pool, id).await?;
	sqlx::query("DELETE FROM source_repo WHERE id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	Ok(())
}

/// 刷新即重拉 index 更新缓存；失败时缓存保留，错误落到 fetch_error 供界面标 stale
pub async fn refresh_repo(state: &Arc<AppState>, id: i64) -> ApiResult<SourceRepoRow> {
	let row = repo_row(&state.pool, id).await?;
	let url = parse_repo_url(&row.url)?;
	match fetch_index(state, &url).await {
		Ok((raw, parsed)) => {
			sqlx::query(
				"UPDATE source_repo SET name = ?, index_json = ?, fetch_error = NULL, fetched_at = ? WHERE id = ?",
			)
			.bind(&parsed.name)
			.bind(&raw)
			.bind(unix_now())
			.bind(id)
			.execute(&state.pool)
			.await?;
			repo_row(&state.pool, id).await
		}
		Err(detail) => {
			tracing::warn!(url = %row.url, "refresh repository failed: {detail}");
			sqlx::query("UPDATE source_repo SET fetch_error = ? WHERE id = ?")
				.bind(INDEX_FETCH_MSG)
				.bind(id)
				.execute(&state.pool)
				.await?;
			Err(ApiError::bad_gateway(INDEX_FETCH_MSG))
		}
	}
}

/// 定时入口：逐仓刷新 index。失败的仓保留旧缓存、fetch_error 落库（见 refresh_repo），
/// 这里只记日志继续刷下一个，互不阻塞
pub async fn refresh_all(state: &Arc<AppState>) {
	for repo in list_repos(&state.pool).await.unwrap_or_default() {
		if let Err(e) = refresh_repo(state, repo.id).await {
			tracing::warn!(repo = repo.id, url = %repo.url, "scheduled repository refresh failed: {e:?}");
		}
	}
}

/// 周期刷新任务：interval 首 tick 立即完成，首跑等满一个周期
pub async fn run_periodic_refresh(state: Arc<AppState>, interval_hours: u64) {
	let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));
	interval.tick().await;
	loop {
		interval.tick().await;
		refresh_all(&state).await;
	}
}

/// 各 key 在所有 repo 缓存里的最新版本与所在 repo；update 检测的唯一口径
/// （Aidoku 语义：整数 version 严格大于已装即算有更新）
pub async fn latest_versions(pool: &SqlitePool) -> ApiResult<HashMap<String, (i64, i64)>> {
	let repos = list_repos(pool).await?;
	let mut latest: HashMap<String, (i64, i64)> = HashMap::new();
	for repo in repos {
		let Some(raw) = &repo.index_json else { continue };
		let Ok(base) = Url::parse(&repo.url) else { continue };
		let parsed = match parse_index(raw.as_bytes(), &base) {
			Ok(parsed) => parsed,
			Err(e) => {
				tracing::warn!(repo = repo.id, "skip unparsable cached index: {e}");
				continue;
			}
		};
		for entry in parsed.entries {
			let replace = latest
				.get(&entry.key)
				.is_none_or(|&(version, _)| entry.version > version);
			if replace {
				latest.insert(entry.key, (entry.version, repo.id));
			}
		}
	}
	Ok(latest)
}

pub fn cached_entries(repo: &SourceRepoRow) -> Vec<RepoEntry> {
	let Some(raw) = &repo.index_json else { return Vec::new() };
	let Ok(base) = Url::parse(&repo.url) else { return Vec::new() };
	match parse_index(raw.as_bytes(), &base) {
		Ok(parsed) => parsed.entries,
		Err(e) => {
			tracing::warn!(repo = repo.id, "skip unparsable cached index: {e}");
			Vec::new()
		}
	}
}

#[derive(Debug, Default)]
pub struct BrowseFilter {
	pub lang: Vec<String>,
	pub query: String,
	/// 内容分级白名单；None 不过滤，Some(空集) 表示一个分级都不显示
	pub content_ratings: Option<Vec<i64>>,
}

/// Aidoku SourceContentRating 三档白名单匹配；None 不过滤，Some(空集) 全过滤
fn content_rating_matches(rating: i64, want: Option<&[i64]>) -> bool {
	want.is_none_or(|set| set.contains(&rating))
}

fn lang_matches(entry_langs: &[String], want: &[String]) -> bool {
	if want.is_empty() {
		return true;
	}
	// multi/All 源（如 mangadex）按 Aidoku 惯例匹配任意语言过滤
	entry_langs.iter().any(|l| {
		let l = l.to_ascii_lowercase();
		l == "multi" || l == "all" || want.contains(&l)
	})
}

fn query_matches(entry: &RepoEntry, query: &str) -> bool {
	if query.is_empty() {
		return true;
	}
	let q = query.to_lowercase();
	entry.name.to_lowercase().contains(&q)
		|| entry.key.to_lowercase().contains(&q)
		|| entry.alt_names.iter().any(|n| n.to_lowercase().contains(&q))
}

pub struct BrowseItem {
	pub repo_id: i64,
	pub repo_name: Option<String>,
	pub entry: RepoEntry,
	pub installed_id: Option<i64>,
	pub installed_version: Option<i64>,
}

/// 跨 repo 浏览：只读缓存，repo 不可达不影响结果；过滤口径与已装源列表一致
pub async fn browse_sources(pool: &SqlitePool, filter: &BrowseFilter) -> ApiResult<Vec<BrowseItem>> {
	let installed: Vec<(i64, String, i64)> = sqlx::query_as("SELECT id, key, version FROM source")
		.fetch_all(pool)
		.await?;
	let installed: HashMap<String, (i64, i64)> =
		installed.into_iter().map(|(id, key, v)| (key, (id, v))).collect();
	let mut out = Vec::new();
	for repo in list_repos(pool).await? {
		for entry in cached_entries(&repo) {
			if !lang_matches(&entry.languages, &filter.lang)
				|| !query_matches(&entry, &filter.query)
				|| !content_rating_matches(entry.content_rating, filter.content_ratings.as_deref())
			{
				continue;
			}
			let (installed_id, installed_version) = installed
				.get(&entry.key)
				.map(|&(id, v)| (Some(id), Some(v)))
				.unwrap_or((None, None));
			out.push(BrowseItem {
				repo_id: repo.id,
				repo_name: repo.name.clone(),
				installed_id,
				installed_version,
				entry,
			});
		}
	}
	out.sort_by(|a, b| {
		a.entry
			.name
			.to_lowercase()
			.cmp(&b.entry.name.to_lowercase())
			.then_with(|| a.entry.key.cmp(&b.entry.key))
			.then_with(|| a.repo_id.cmp(&b.repo_id))
	});
	Ok(out)
}

/// 下载 .aix 落 sources 目录并走扫描同款注册路径；失败清理落盘文件，不留残包
pub async fn install_from_repo(state: &Arc<AppState>, repo_id: i64, key: &str) -> ApiResult<SourceRow> {
	let row = repo_row(&state.pool, repo_id).await?;
	let entry = cached_entries(&row)
		.into_iter()
		.find(|e| e.key == key)
		.ok_or_else(|| ApiError::not_found("source not found in repository"))?;
	if !crate::source::valid_source_key(key) {
		return Err(ApiError::bad_request("invalid source key"));
	}
	let Some(download_url) = &entry.download_url else {
		return Err(ApiError::bad_gateway(PACKAGE_FETCH_MSG));
	};
	let url = parse_repo_url(download_url)?;
	let bytes = fetch_capped(&state.http, &url, WASM_PAYLOAD_MAX)
		.await
		.map_err(|detail| {
			tracing::warn!(url = %url, "download package failed: {detail}");
			ApiError::bad_gateway(PACKAGE_FETCH_MSG)
		})?;

	let path = state.config.sources_dir().join(format!("{key}.aix"));
	let tmp = state.config.sources_dir().join(format!(".{key}.aix.tmp"));
	tokio::fs::write(&tmp, &bytes)
		.await
		.map_err(|e| ApiError::from(sqlx::Error::Io(e)))?;
	tokio::fs::rename(&tmp, &path)
		.await
		.map_err(|e| ApiError::from(sqlx::Error::Io(e)))?;
	let registered = crate::source::register_aix(state, &path).await;
	match registered {
		Ok((registered_key, _)) if registered_key == key => {}
		Ok((registered_key, _)) => {
			let _ = tokio::fs::remove_file(&path).await;
			let _ = tokio::fs::remove_file(path.with_extension("png")).await;
			tracing::warn!(key, registered_key, "package manifest id mismatch");
			return Err(ApiError::bad_gateway(PACKAGE_FETCH_MSG));
		}
		Err(reason) => {
			let _ = tokio::fs::remove_file(&path).await;
			let _ = tokio::fs::remove_file(path.with_extension("png")).await;
			tracing::warn!(key, "install package failed: {reason}");
			return Err(ApiError::bad_gateway(PACKAGE_FETCH_MSG));
		}
	}

	// 标记安装来源仓库（目录扫描侧载保持 NULL），供 missingFromRepos 判定
	sqlx::query("UPDATE source SET repo_id = ? WHERE key = ?")
		.bind(repo_id)
		.bind(key)
		.execute(&state.pool)
		.await?;
	let source: SourceRow = sqlx::query_as(
		"SELECT id, key, name, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id FROM source WHERE key = ?",
	)
	.bind(key)
	.fetch_one(&state.pool)
	.await?;
	// 更新场景下踢掉在跑的实例，下一次调用按新包懒加载
	state.evict_source(source.id).await;
	Ok(source)
}

/// 单源更新的完整结果：新行 + 跨 breaking 的 key 迁移报告
pub struct SourceUpdate {
	pub source: SourceRow,
	pub migration: crate::source::rekey::RekeyReport,
}

/// 单源更新：取所有 repo 缓存里该 key 的最新版，严格大于已装才动手；
/// 跨过新包 manifest 的 breakingChangeVersion 时做 key 迁移（见 rekey 模块）
pub async fn update_source(state: &Arc<AppState>, source_id: i64) -> ApiResult<SourceUpdate> {
	let row: Option<(String, i64)> = sqlx::query_as("SELECT key, version FROM source WHERE id = ?")
		.bind(source_id)
		.fetch_optional(&state.pool)
		.await?;
	let Some((key, version)) = row else {
		return Err(ApiError::not_found("source not found"));
	};
	let latest = latest_versions(&state.pool).await?;
	match latest.get(&key) {
		Some(&(new_version, repo_id)) if new_version > version => {
			let source = install_from_repo(state, repo_id, &key).await?;
			let migration = crate::source::rekey::migrate_after_update(state, source.id, version).await?;
			Ok(SourceUpdate { source, migration })
		}
		_ => Err(ApiError::no_update(format!("source {key} is up to date"))),
	}
}

pub struct UpdateAllOutcome {
	pub updated: Vec<String>,
	pub failed: Vec<(String, String)>,
	/// 跨 breaking 但源无自迁移能力的 key：界面引导走 flow A 同源迁移
	pub migration_required: Vec<String>,
	/// 原地重键里被跳过的行（映射失败/撞键），按源归组
	pub migration_warnings: Vec<(String, String)>,
}

pub async fn update_all(state: &Arc<AppState>) -> ApiResult<UpdateAllOutcome> {
	let sources: Vec<(String, i64)> = sqlx::query_as("SELECT key, version FROM source")
		.fetch_all(&state.pool)
		.await?;
	let latest = latest_versions(&state.pool).await?;
	let mut outcome = UpdateAllOutcome {
		updated: Vec::new(),
		failed: Vec::new(),
		migration_required: Vec::new(),
		migration_warnings: Vec::new(),
	};
	for (key, version) in sources {
		let Some(&(new_version, repo_id)) = latest.get(&key) else {
			continue;
		};
		if new_version <= version {
			continue;
		}
		let source = match install_from_repo(state, repo_id, &key).await {
			Ok(source) => source,
			Err(e) => {
				tracing::warn!(key, "update-all install failed: {e:?}");
				outcome.failed.push((key, e.message().to_string()));
				continue;
			}
		};
		outcome.updated.push(key.clone());
		match crate::source::rekey::migrate_after_update(state, source.id, version).await {
			Ok(report) => {
				if report.required {
					outcome.migration_required.push(key.clone());
				}
				for warning in report.warnings {
					outcome.migration_warnings.push((key.clone(), warning));
				}
			}
			// 重键失败不回滚已完成的更新，降级为该源的警告
			Err(e) => {
				tracing::warn!(key, "post-update key migration failed: {e:?}");
				outcome.migration_warnings.push((key.clone(), e.message().to_string()));
			}
		}
	}
	Ok(outcome)
}
