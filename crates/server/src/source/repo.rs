//! Aidoku-style source repositories: fetching/caching/parsing of index JSON, cross-repo browsing and .aix installs.
//! The schema mirrors the upstream Aidoku app (Aidoku/Core/Sources/SourceList/): an index is
//! {name, feedbackURL?, sources:[ExternalSourceInfo]}; version is a monotonically increasing integer;
//! downloadURL/iconURL resolve per RFC3986 against the index URL; lang/nsfw/file/icon
//! are legacy deprecated fields (file→sources/<file>, icon→icons/<icon>), accepted for compatibility when parsing

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

/// A real index is about a hundred KiB, so 4 MiB leaves ample headroom
const INDEX_MAX: u64 = 4 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Unified outward message: fetch/parse failures are not differentiated; details go to the logs only
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
	// Aidoku's URL fields are all-caps abbreviations that camelCase cannot derive
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

/// A parsed repo source entry: URLs already resolved to absolute against the repo address
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

/// Parses and validates raw index text; URLs resolve against base. A missing sources array means a bad index
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

/// Repo URLs allow only http/https: self-hosted users will point at intranet addresses, so there is no IP-based check
pub fn parse_repo_url(raw: &str) -> ApiResult<Url> {
	let url = Url::parse(raw).map_err(|_| ApiError::bad_request("invalid repository url"))?;
	if !matches!(url.scheme(), "http" | "https") {
		return Err(ApiError::bad_request("repository url must be http or https"));
	}
	Ok(url)
}

/// GET with an overall timeout and a size cap; error details go to the logs, outward messages stay unified
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
	let id = result.last_insert_rowid();
	tracing::info!(repo = id, entries = parsed.entries.len(), "source repository added");
	repo_row(&state.pool, id).await
}

pub async fn delete_repo(state: &Arc<AppState>, id: i64) -> ApiResult<()> {
	repo_row(&state.pool, id).await?;
	sqlx::query("DELETE FROM source_repo WHERE id = ?")
		.bind(id)
		.execute(&state.pool)
		.await?;
	Ok(())
}

/// Refreshing re-pulls the index and updates the cache; on failure the cache is kept and the error lands in fetch_error for the UI to mark stale
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
			let row = repo_row(&state.pool, id).await?;
			tracing::info!(repo = id, entries = parsed.entries.len(), "source repository refreshed");
			Ok(row)
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

/// Scheduled entry point: refreshes each repo's index in turn. A failed repo keeps its old cache with fetch_error
/// recorded (see refresh_repo); here we only log and move to the next, never blocking each other
pub async fn refresh_all(state: &Arc<AppState>) {
	for repo in list_repos(&state.pool).await.unwrap_or_default() {
		if let Err(e) = refresh_repo(state, repo.id).await {
			tracing::warn!(repo = repo.id, url = %repo.url, "scheduled repository refresh failed: {e:?}");
		}
	}
}

/// Periodic refresh task: interval's first tick completes immediately, so the first run waits a full period
pub async fn run_periodic_refresh(state: Arc<AppState>, interval_hours: u64) {
	let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));
	interval.tick().await;
	loop {
		interval.tick().await;
		refresh_all(&state).await;
	}
}

/// Each key's newest version across all repo caches and the repo holding it; the single source of truth for update
/// detection (Aidoku semantics: an integer version strictly greater than installed counts as an update)
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
	/// Content-rating whitelist; None means no filtering, Some(empty set) hides every rating
	pub content_ratings: Option<Vec<i64>>,
}

/// Aidoku SourceContentRating three-tier whitelist matching; None means no filtering, Some(empty set) filters everything out
fn content_rating_matches(rating: i64, want: Option<&[i64]>) -> bool {
	want.is_none_or(|set| set.contains(&rating))
}

fn lang_matches(entry_langs: &[String], want: &[String]) -> bool {
	if want.is_empty() {
		return true;
	}
	// multi/All sources (e.g. mangadex) match any language filter, per Aidoku convention
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

/// Cross-repo browsing: reads the cache only, so an unreachable repo does not affect results; filter conventions match the installed-source list
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

/// Downloads the .aix into the sources directory and takes the same registration path as the scan; failures clean up the landed file, leaving no partial package
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

	// Marks the repo this install came from (directory-scan sideloads stay NULL), feeding the missingFromRepos check
	sqlx::query("UPDATE source SET repo_id = ? WHERE key = ?")
		.bind(repo_id)
		.bind(key)
		.execute(&state.pool)
		.await?;
	let source: SourceRow = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source WHERE key = ?",
	)
	.bind(key)
	.fetch_one(&state.pool)
	.await?;
	// On update, kick out the running instance so the next call lazily loads the new package
	state.evict_source(source.id).await;
	tracing::info!(repo = repo_id, key, version = source.version, bytes = bytes.len(), "source package installed");
	Ok(source)
}

/// The complete result of a single-source update: the new row + the cross-breaking key migration report
pub struct SourceUpdate {
	pub source: SourceRow,
	pub migration: crate::source::rekey::RekeyReport,
}

/// Single-source update: takes the key's newest version across all repo caches, acting only when strictly greater
/// than installed; when crossing the new package manifest's breakingChangeVersion, runs a key migration (see the rekey module)
pub async fn update_source(state: &Arc<AppState>, source_id: i64, user_id: i64) -> ApiResult<SourceUpdate> {
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
			let migration = crate::source::rekey::migrate_after_update(state, source.id, version, user_id).await?;
			Ok(SourceUpdate { source, migration })
		}
		_ => Err(ApiError::no_update(format!("source {key} is up to date"))),
	}
}

pub struct UpdateAllOutcome {
	pub updated: Vec<String>,
	pub failed: Vec<(String, String)>,
	/// Keys that crossed a breaking change but whose source has no self-migration capability: the UI guides the user through flow A same-source migration
	pub migration_required: Vec<String>,
	/// Rows skipped during in-place rekeying (mapping failed/key collision), grouped by source
	pub migration_warnings: Vec<(String, String)>,
}

pub async fn update_all(state: &Arc<AppState>, user_id: i64) -> ApiResult<UpdateAllOutcome> {
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
		match crate::source::rekey::migrate_after_update(state, source.id, version, user_id).await {
			Ok(report) => {
				if report.required {
					outcome.migration_required.push(key.clone());
				}
				for warning in report.warnings {
					outcome.migration_warnings.push((key.clone(), warning));
				}
			}
			// A rekey failure does not roll back the completed update; it degrades to a warning on that source
			Err(e) => {
				tracing::warn!(key, "post-update key migration failed: {e:?}");
				outcome.migration_warnings.push((key.clone(), e.message().to_string()));
			}
		}
	}
	tracing::info!(
		updated = outcome.updated.len(),
		failed = outcome.failed.len(),
		migration_required = outcome.migration_required.len(),
		migration_warnings = outcome.migration_warnings.len(),
		"source update-all finished"
	);
	Ok(outcome)
}
