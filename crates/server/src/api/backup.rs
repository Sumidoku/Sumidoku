//! Full backup: export/import of the admin's library, categories, progress, settings, and source settings.
//! BackupDoc is the external contract (evolves via the version field); the .aib/.tachibk importers map onto it too

use crate::error::{ApiError, ApiResult};
use crate::model::{default_value_to_json, json_to_default_value, unix_now};
use crate::source::repo::{self, SourceRepoRow};
use crate::state::AppState;
use aidoku_wasm::DefaultValue;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub const BACKUP_VERSION: i64 = 1;
/// A large-library backup can reach tens of MB, so this route's body cap is raised
pub const BODY_MAX: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDoc {
	pub version: i64,
	#[serde(default)]
	pub created_at: i64,
	#[serde(default)]
	pub sources: Vec<BackupSource>,
	#[serde(default)]
	pub library: Vec<BackupManga>,
	#[serde(default)]
	pub categories: Vec<BackupCategory>,
	#[serde(default)]
	pub progress: Vec<BackupProgress>,
	#[serde(default)]
	pub settings: BTreeMap<String, Value>,
	#[serde(default)]
	pub source_settings: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSource {
	pub key: String,
	pub name: String,
	pub version: i64,
	#[serde(default)]
	pub repo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManga {
	pub source_key: String,
	pub key: String,
	#[serde(default)]
	pub title: String,
	#[serde(default)]
	pub authors: Vec<String>,
	#[serde(default)]
	pub artists: Vec<String>,
	#[serde(default)]
	pub description: Option<String>,
	#[serde(default)]
	pub tags: Vec<String>,
	#[serde(default)]
	pub status: i64,
	#[serde(default)]
	pub content_rating: i64,
	#[serde(default)]
	pub viewer: i64,
	#[serde(default)]
	pub cover_url: Option<String>,
	#[serde(default)]
	pub url: Option<String>,
	#[serde(default)]
	pub initialized: bool,
	#[serde(default)]
	pub update_strategy: i64,
	#[serde(default)]
	pub in_library: bool,
	#[serde(default)]
	pub in_library_at: Option<i64>,
	#[serde(default)]
	pub viewer_flags: i64,
	#[serde(default)]
	pub incognito: bool,
	#[serde(default)]
	pub sort_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMangaRef {
	pub source_key: String,
	pub manga_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupCategory {
	pub name: String,
	#[serde(default)]
	pub sort_order: i64,
	#[serde(default)]
	pub is_default: bool,
	/// Instance-local category ids are not portable, so `2` carries the exclusion on this named category
	/// and restore translates it to the target's category id.
	#[serde(default)]
	pub include_in_update: i64,
	#[serde(default)]
	pub include_in_download: i64,
	#[serde(default)]
	pub manga: Vec<BackupMangaRef>,
}

/// Chapter metadata snapshot: a fresh instance has no chapter rows yet, so restoring progress creates rows from it (existing rows are untouched; the source refresh overwrites)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupChapter {
	pub key: String,
	#[serde(default)]
	pub title: Option<String>,
	#[serde(default)]
	pub number: Option<f64>,
	#[serde(default)]
	pub volume: Option<f64>,
	#[serde(default)]
	pub scanlators: Vec<String>,
	#[serde(default)]
	pub language: Option<String>,
	#[serde(default)]
	pub thumbnail: Option<String>,
	#[serde(default)]
	pub url: Option<String>,
	#[serde(default)]
	pub date_uploaded: Option<i64>,
	#[serde(default)]
	pub source_order: i64,
	#[serde(default)]
	pub page_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
	pub source_key: String,
	pub manga_key: String,
	pub chapter: BackupChapter,
	#[serde(default)]
	pub last_page_read: i64,
	#[serde(default)]
	pub is_read: bool,
	#[serde(default)]
	pub is_bookmarked: bool,
	#[serde(default)]
	pub last_read_at: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct ExportMangaRow {
	source_key: String,
	key: String,
	title: String,
	authors: sqlx::types::Json<Vec<String>>,
	artists: sqlx::types::Json<Vec<String>>,
	description: Option<String>,
	tags: sqlx::types::Json<Vec<String>>,
	status: i64,
	content_rating: i64,
	viewer: i64,
	cover_url: Option<String>,
	url: Option<String>,
	initialized: bool,
	update_strategy: i64,
	in_library: bool,
	in_library_at: Option<i64>,
	viewer_flags: i64,
	incognito: bool,
	sort_key: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ExportProgressRow {
	source_key: String,
	manga_key: String,
	key: String,
	title: Option<String>,
	number: Option<f64>,
	volume: Option<f64>,
	scanlators: sqlx::types::Json<Vec<String>>,
	language: Option<String>,
	thumbnail: Option<String>,
	url: Option<String>,
	date_uploaded: Option<i64>,
	source_order: i64,
	page_count: i64,
	last_page_read: i64,
	is_read: bool,
	is_bookmarked: bool,
	last_read_at: Option<i64>,
}

/// Export-document construction shared by the GET handler and the scheduler, producing the admin's portable reader state
pub async fn build_backup_doc(pool: &SqlitePool) -> ApiResult<BackupDoc> {
	let user_id = crate::USER_ID;
	let source_rows: Vec<(String, String, i64)> =
		sqlx::query_as("SELECT key, name, version FROM source ORDER BY id")
			.fetch_all(pool)
			.await?;
	// There is no explicit FK for which repo a source belongs to, so use the first repo in the cached index containing the same key
	let repos: Vec<SourceRepoRow> = repo::list_repos(pool).await?;
	let mut repo_of: HashMap<String, String> = HashMap::new();
	for row in &repos {
		for entry in repo::cached_entries(row) {
			repo_of.entry(entry.key).or_insert_with(|| row.url.clone());
		}
	}
	let sources = source_rows
		.into_iter()
		.map(|(key, name, version)| BackupSource {
			repo_url: repo_of.get(&key).cloned(),
			key,
			name,
			version,
		})
		.collect();

	let manga_rows: Vec<ExportMangaRow> = sqlx::query_as(
		"SELECT s.key AS source_key, m.key, m.title, m.authors, m.artists, m.description, m.tags,
		 m.status, m.content_rating, m.viewer, m.cover_url, m.url, m.initialized, m.update_strategy,
		 mu.in_library, mu.in_library_at, mu.viewer_flags, mu.incognito, mu.sort_key
		 FROM manga m
		 JOIN source s ON s.id = m.source_id
		 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ?
		 ORDER BY m.id",
	)
	.bind(user_id)
	.fetch_all(pool)
	.await?;
	let library = manga_rows
		.into_iter()
		.map(|row| BackupManga {
			source_key: row.source_key,
			key: row.key,
			title: row.title,
			authors: row.authors.0,
			artists: row.artists.0,
			description: row.description,
			tags: row.tags.0,
			status: row.status,
			content_rating: row.content_rating,
			viewer: row.viewer,
			cover_url: row.cover_url,
			url: row.url,
			initialized: row.initialized,
			update_strategy: row.update_strategy,
			in_library: row.in_library,
			in_library_at: row.in_library_at,
			viewer_flags: row.viewer_flags,
			incognito: row.incognito,
			sort_key: row.sort_key,
		})
		.collect();

	let excluded_update_categories: HashSet<i64> = crate::update::LibraryUpdateSettings::load(pool, user_id, 0)
		.await?
		.excluded_update_categories
		.into_iter()
		.collect();
	let excluded_download_key = format!(
		"user.{user_id}.{}",
		crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES
	);
	let excluded_download_categories: HashSet<i64> =
		sqlx::query_scalar::<_, String>("SELECT value FROM setting WHERE key = ?")
			.bind(&excluded_download_key)
			.fetch_optional(pool)
			.await?
			.and_then(|raw| serde_json::from_str::<Vec<i64>>(&raw).ok())
			.unwrap_or_default()
			.into_iter()
			.collect();
	let category_rows: Vec<(i64, String, i64, bool)> = sqlx::query_as(
		"SELECT id, name, sort_order, is_default
		 FROM category WHERE user_id = ? ORDER BY sort_order, id",
	)
	.bind(user_id)
	.fetch_all(pool)
	.await?;
	let member_rows: Vec<(i64, String, String)> = sqlx::query_as(
		"SELECT cm.category_id, s.key, m.key FROM category_manga cm
		 JOIN manga m ON m.id = cm.manga_id
		 JOIN source s ON s.id = m.source_id
		 WHERE cm.user_id = ? ORDER BY m.id",
	)
	.bind(user_id)
	.fetch_all(pool)
	.await?;
	let mut members: HashMap<i64, Vec<BackupMangaRef>> = HashMap::new();
	for (category_id, source_key, manga_key) in member_rows {
		members
			.entry(category_id)
			.or_default()
			.push(BackupMangaRef { source_key, manga_key });
	}
	let categories = category_rows
		.into_iter()
		.map(|(id, name, sort_order, is_default)| {
			BackupCategory {
				name,
				sort_order,
				is_default,
				include_in_update: if excluded_update_categories.contains(&id) { 2 } else { 0 },
				include_in_download: if excluded_download_categories.contains(&id) { 2 } else { 0 },
				manga: members.remove(&id).unwrap_or_default(),
			}
		})
		.collect();

	// All-default rows carry no user state and are deduped out of the export (is_downloaded is out of backup scope)
	let progress_rows: Vec<ExportProgressRow> = sqlx::query_as(
		"SELECT s.key AS source_key, m.key AS manga_key, c.key, c.title, c.number, c.volume,
		 c.scanlators, c.language, c.thumbnail, c.url, c.date_uploaded, c.source_order, c.page_count,
		 cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter_user cu
		 JOIN chapter c ON c.id = cu.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 JOIN source s ON s.id = m.source_id
		 WHERE cu.user_id = ?
		 AND (cu.last_page_read != 0 OR cu.is_read != 0 OR cu.is_bookmarked != 0 OR cu.last_read_at IS NOT NULL)
		 ORDER BY c.id",
	)
	.bind(user_id)
	.fetch_all(pool)
	.await?;
	let progress = progress_rows
		.into_iter()
		.map(|row| BackupProgress {
			source_key: row.source_key,
			manga_key: row.manga_key,
			chapter: BackupChapter {
				key: row.key,
				title: row.title,
				number: row.number,
				volume: row.volume,
				scanlators: row.scanlators.0,
				language: row.language,
				thumbnail: row.thumbnail,
				url: row.url,
				date_uploaded: row.date_uploaded,
				source_order: row.source_order,
				page_count: row.page_count,
			},
			last_page_read: row.last_page_read,
			is_read: row.is_read,
			is_bookmarked: row.is_bookmarked,
			last_read_at: row.last_read_at,
		})
		.collect();

	let setting_rows: Vec<(String, String)> =
		sqlx::query_as("SELECT key, value FROM setting ORDER BY key")
			.fetch_all(pool)
			.await?;
	let mut settings = BTreeMap::new();
	let own_prefix = format!("user.{user_id}.");
	for (key, raw) in setting_rows {
		// Legacy owner namespace keys are restored to bare keys, matching the settings API contract
		let key = if key.starts_with("user.") {
			match key.strip_prefix(&own_prefix) {
				Some(scoped) if crate::api::USER_SCOPED_SETTING_KEYS.contains(&scoped) => scoped.to_string(),
				_ => continue,
			}
		} else {
			key
		};
		if omitted_from_backup(&key) {
			continue;
		}
		// A corrupt row must not fail the whole backup; export it as the raw string
		let value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
		settings.insert(key, value);
	}

	let mut source_settings: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
	let source_setting_rows: Vec<(String, String, String)> = sqlx::query_as(
		"SELECT s.key, ss.key, ss.value FROM source_setting ss
		 JOIN source s ON s.id = ss.source_id ORDER BY s.key, ss.key",
	)
	.fetch_all(pool)
	.await?;
	for (source_key, key, raw) in source_setting_rows {
		match serde_json::from_str::<DefaultValue>(&raw) {
			Ok(value) => {
				source_settings
					.entry(source_key)
					.or_default()
					.insert(key, default_value_to_json(&value));
			}
			Err(e) => tracing::warn!(source_key, key, "skip unparsable source_setting: {e}"),
		}
	}

	let doc = BackupDoc {
		version: BACKUP_VERSION,
		created_at: unix_now(),
		sources,
		library,
		categories,
		progress,
		settings,
		source_settings,
	};
	Ok(doc)
}

pub async fn export_backup(
	State(state): State<Arc<AppState>>,
) -> ApiResult<Response> {
	let doc = build_backup_doc(&state.pool).await?;
	let body = serde_json::to_vec(&doc)?;
	let date = chrono::Utc::now().format("%Y%m%d");
	let disposition = format!("attachment; filename=\"sumidoku-backup-{date}.json\"");
	Ok((
		[
			(header::CONTENT_TYPE, "application/json".to_string()),
			(header::CONTENT_DISPOSITION, disposition),
		],
		body,
	)
		.into_response())
}

#[derive(Debug, Deserialize)]
pub struct ImportQuery {
	pub sections: Option<String>,
}

/// Existing backups in the server backup directory, newest first
pub async fn list_backups(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	let dir = crate::backup::backups_dir(&state.pool, &state.config).await?;
	let files: Vec<Value> = crate::backup::list_backup_files(&dir)
		.await?
		.into_iter()
		.map(|f| json!({"name": f.name, "size": f.size, "createdAt": f.created_at}))
		.collect();
	Ok(Json(json!({ "dir": dir.display().to_string(), "files": files })))
}

#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
	pub name: String,
}

/// Restores from a file already in the backup directory: the server reads the file, taking the same validate+transaction path as an upload import
pub async fn restore_backup(
	State(state): State<Arc<AppState>>,
	Json(body): Json<RestoreRequest>,
) -> ApiResult<Json<Value>> {
	if crate::backup::parse_backup_name(&body.name).is_none() {
		return Err(ApiError::bad_request("invalid backup file name"));
	}
	let dir = crate::backup::backups_dir(&state.pool, &state.config).await?;
	let bytes = match tokio::fs::read(dir.join(&body.name)).await {
		Ok(bytes) => bytes,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			return Err(ApiError::not_found("backup not found"));
		}
		Err(e) => return Err(ApiError::internal(format!("read backup: {e}"))),
	};
	let result = import_backup_doc(&state, None, &bytes).await?;
	Ok(Json(result))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
	Library,
	Categories,
	Progress,
	Settings,
	SourceSettings,
}

fn parse_sections(raw: Option<&str>) -> ApiResult<HashSet<Section>> {
	const ALL: [Section; 5] = [
		Section::Library,
		Section::Categories,
		Section::Progress,
		Section::Settings,
		Section::SourceSettings,
	];
	let Some(raw) = raw else {
		return Ok(ALL.into_iter().collect());
	};
	let mut out = HashSet::new();
	for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
		out.insert(match token {
			"library" => Section::Library,
			"categories" => Section::Categories,
			"progress" => Section::Progress,
			"settings" => Section::Settings,
			"sourceSettings" => Section::SourceSettings,
			other => return Err(ApiError::bad_request(format!("invalid section: {other}"))),
		});
	}
	Ok(out)
}

#[derive(Debug, Default)]
struct Restored {
	library: i64,
	categories: i64,
	progress: i64,
	settings: i64,
	source_settings: i64,
}

pub async fn import_backup(
	State(state): State<Arc<AppState>>,
	Query(query): Query<ImportQuery>,
	body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
	let result = import_backup_doc(&state, query.sections.as_deref(), &body).await?;
	Ok(Json(result))
}

/// The validate + single-transaction import executor: shared by upload and restore-from-list endpoints
pub async fn import_backup_doc(state: &AppState, sections_query: Option<&str>, body: &[u8]) -> ApiResult<Value> {
	let user_id = crate::USER_ID;
	let sections = parse_sections(sections_query)?;
	let raw: Value = serde_json::from_slice(body)
		.map_err(|e| ApiError::bad_request(format!("invalid backup document: {e}")))?;
	let version = raw.get("version").and_then(Value::as_i64);
	if version != Some(BACKUP_VERSION) {
		let message = match version {
			Some(v) => format!("unsupported backup version: {v}"),
			None => "missing backup version".to_string(),
		};
		return Err(ApiError::bad_request(message));
	}
	let doc: BackupDoc = serde_json::from_value(raw)
		.map_err(|e| ApiError::bad_request(format!("invalid backup document: {e}")))?;

	let mut restored = Restored::default();
	let mut warnings: Vec<String> = Vec::new();
	let mut touched_sources: HashSet<i64> = HashSet::new();
	{
		let mut tx = crate::db::begin_write(&state.pool).await?;
		if sections.contains(&Section::Library) {
			restore_library(&mut tx, user_id, &doc, &mut restored, &mut warnings).await?;
		}
		if sections.contains(&Section::Categories) {
			restore_categories(&mut tx, user_id, &doc, &mut restored, &mut warnings).await?;
		}
		if sections.contains(&Section::Progress) {
			restore_progress(&mut tx, user_id, &doc, &mut restored, &mut warnings).await?;
		}
		if sections.contains(&Section::Settings) {
			restore_settings(&mut tx, user_id, &doc, &mut restored).await?;
		}
		if sections.contains(&Section::SourceSettings) {
			restore_source_settings(&mut tx, &doc, &mut restored, &mut warnings, &mut touched_sources)
				.await?;
		}
		if sections.contains(&Section::Categories)
			&& doc.categories.iter().any(|category| category.include_in_download == 1)
		{
			// Include-only categories have no exclusion-list equivalent, so automatic downloads must stay conservative.
			sqlx::query(
				"INSERT INTO setting (key, value) VALUES ('auto_download_new_chapters', 'false')
				 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
			)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
	}
	// Source settings may have changed credentials, so hot-restart running instances; a failure does not roll back (the import is committed; the restart waits for the next lazy load)
	for id in touched_sources {
		if let Err(e) = crate::api::restart_if_loaded(state, id).await {
			tracing::warn!(source_id = id, "restart after backup import failed: {}", e.message());
		}
	}
	// Restored local instance settings (roots/oneshots/exclusions) may no longer match the index: converge
	// with a rescan rather than leaving the old rows authoritative
	if let Err(e) = crate::internal::local::scan::enqueue_scan(state, None, crate::task::PRIORITY_MANUAL, true).await {
		tracing::warn!("enqueue rescan after backup import failed: {}", e.message());
	}
	Ok(json!({
		"restored": {
			"library": restored.library,
			"categories": restored.categories,
			"progress": restored.progress,
			"settings": restored.settings,
			"sourceSettings": restored.source_settings,
		},
		"warnings": warnings,
	}))
}

async fn source_id_by_key(tx: &mut Transaction<'_, Sqlite>, key: &str) -> ApiResult<Option<i64>> {
	let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM source WHERE key = ?")
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
	Ok(row.map(|(id,)| id))
}

async fn restore_library(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &BackupDoc,
	restored: &mut Restored,
	warnings: &mut Vec<String>,
) -> ApiResult<()> {
	for entry in &doc.library {
		if entry.source_key.is_empty() || entry.key.is_empty() {
			return Err(ApiError::bad_request(
				"backup library entry must have non-empty sourceKey and key",
			));
		}
		let Some(source_id) = source_id_by_key(tx, &entry.source_key).await? else {
			warnings.push(format!(
				"source not installed: {}; skipped manga {}",
				entry.source_key, entry.key
			));
			continue;
		};
		sqlx::query(
			"INSERT INTO manga (source_id, key, title, authors, artists, description, tags, status,
			 content_rating, viewer, cover_url, url, initialized, update_strategy)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
			 ON CONFLICT (source_id, key) DO UPDATE SET
			 title = excluded.title, authors = excluded.authors, artists = excluded.artists,
			 description = excluded.description, tags = excluded.tags, status = excluded.status,
			 content_rating = excluded.content_rating, viewer = excluded.viewer,
			 cover_url = excluded.cover_url,
			 url = excluded.url, initialized = excluded.initialized,
			 update_strategy = excluded.update_strategy",
		)
		.bind(source_id)
		.bind(&entry.key)
		.bind(&entry.title)
		.bind(serde_json::to_string(&entry.authors)?)
		.bind(serde_json::to_string(&entry.artists)?)
		.bind(&entry.description)
		.bind(serde_json::to_string(&entry.tags)?)
		.bind(entry.status)
		.bind(entry.content_rating)
		.bind(entry.viewer)
		.bind(&entry.cover_url)
		.bind(&entry.url)
		.bind(entry.initialized)
		.bind(entry.update_strategy)
		.execute(&mut **tx)
		.await?;
		let (manga_id,): (i64,) =
			sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
				.bind(source_id)
				.bind(&entry.key)
				.fetch_one(&mut **tx)
				.await?;
		sqlx::query(
			"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at, viewer_flags, incognito, sort_key)
			 VALUES (?, ?, ?, ?, ?, ?, ?)
			 ON CONFLICT (manga_id, user_id) DO UPDATE SET
			 in_library = excluded.in_library, in_library_at = excluded.in_library_at,
			 viewer_flags = excluded.viewer_flags, incognito = excluded.incognito,
			 sort_key = excluded.sort_key",
		)
		.bind(manga_id)
		.bind(user_id)
		.bind(entry.in_library)
		.bind(entry.in_library_at)
		.bind(entry.viewer_flags)
		.bind(entry.incognito)
		.bind(&entry.sort_key)
		.execute(&mut **tx)
		.await?;
		restored.library += 1;
	}
	Ok(())
}

fn check_include_in(category: &str, field: &str, value: i64) -> ApiResult<i64> {
	match value {
		v @ 0..=2 => Ok(v),
		v => Err(ApiError::bad_request(format!(
			"backup category {category} {field} must be 0, 1 or 2, got {v}"
		))),
	}
}

async fn merge_excluded_categories(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	setting: &str,
	migrated: Vec<i64>,
) -> ApiResult<()> {
	if migrated.is_empty() {
		return Ok(());
	}
	let key = format!("user.{user_id}.{setting}");
	let current: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = ?")
		.bind(&key)
		.fetch_optional(&mut **tx)
		.await?;
	let mut existing: Vec<i64> = current
		.and_then(|(raw,)| serde_json::from_str(&raw).ok())
		.unwrap_or_default();
	for id in migrated {
		if !existing.contains(&id) {
			existing.push(id);
		}
	}
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(&key)
	.bind(serde_json::to_string(&existing)?)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

async fn restore_categories(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &BackupDoc,
	restored: &mut Restored,
	warnings: &mut Vec<String>,
) -> ApiResult<()> {
	let mut migrated_update: Vec<i64> = Vec::new();
	let mut migrated_download: Vec<i64> = Vec::new();
	for category in &doc.categories {
		if category.name.trim().is_empty() {
			return Err(ApiError::bad_request("backup category name must not be empty"));
		}
		check_include_in(&category.name, "includeInUpdate", category.include_in_update)?;
		check_include_in(&category.name, "includeInDownload", category.include_in_download)?;
		let existing: Option<(i64,)> =
			sqlx::query_as("SELECT id FROM category WHERE user_id = ? AND name = ?")
				.bind(user_id)
				.bind(&category.name)
				.fetch_optional(&mut **tx)
				.await?;
		let category_id = match existing {
			Some((id,)) => {
				sqlx::query("UPDATE category SET sort_order = ?, is_default = ? WHERE id = ?")
					.bind(category.sort_order)
					.bind(category.is_default)
					.bind(id)
					.execute(&mut **tx)
					.await?;
				id
			}
			None => {
				sqlx::query_scalar(
					"INSERT INTO category (user_id, name, sort_order, is_default)
					 VALUES (?, ?, ?, ?) RETURNING id",
				)
				.bind(user_id)
				.bind(&category.name)
				.bind(category.sort_order)
				.bind(category.is_default)
				.fetch_one(&mut **tx)
				.await?
			}
		};
		if category.include_in_update == 2 {
			migrated_update.push(category_id);
		}
		if category.include_in_download == 2 {
			migrated_download.push(category_id);
		}
		for member in &category.manga {
			let Some(source_id) = source_id_by_key(tx, &member.source_key).await? else {
				warnings.push(format!(
					"source not installed: {}; skipped category {} membership for manga {}",
					member.source_key, category.name, member.manga_key
				));
				continue;
			};
			let manga: Option<(i64,)> =
				sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
					.bind(source_id)
					.bind(&member.manga_key)
					.fetch_optional(&mut **tx)
					.await?;
			let Some((manga_id,)) = manga else {
				warnings.push(format!(
					"manga not found: {}/{}; skipped category {} membership",
					member.source_key, member.manga_key, category.name
				));
				continue;
			};
			sqlx::query(
				"INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id)
				 VALUES (?, ?, ?)",
			)
			.bind(category_id)
			.bind(manga_id)
			.bind(user_id)
			.execute(&mut **tx)
			.await?;
		}
		restored.categories += 1;
	}
	merge_excluded_categories(
		tx,
		user_id,
		crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES,
		migrated_update,
	)
	.await?;
	merge_excluded_categories(
		tx,
		user_id,
		crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES,
		migrated_download,
	)
	.await?;
	Ok(())
}

async fn restore_progress(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &BackupDoc,
	restored: &mut Restored,
	warnings: &mut Vec<String>,
) -> ApiResult<()> {
	for entry in &doc.progress {
		if entry.source_key.is_empty() || entry.manga_key.is_empty() || entry.chapter.key.is_empty() {
			return Err(ApiError::bad_request(
				"backup progress entry must have non-empty sourceKey, mangaKey and chapter.key",
			));
		}
		let Some(source_id) = source_id_by_key(tx, &entry.source_key).await? else {
			warnings.push(format!(
				"source not installed: {}; skipped progress for manga {}",
				entry.source_key, entry.manga_key
			));
			continue;
		};
		let manga: Option<(i64,)> =
			sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
				.bind(source_id)
				.bind(&entry.manga_key)
				.fetch_optional(&mut **tx)
				.await?;
		let Some((manga_id,)) = manga else {
			warnings.push(format!(
				"manga not found: {}/{}; skipped progress for chapter {}",
				entry.source_key, entry.manga_key, entry.chapter.key
			));
			continue;
		};
		// fetched_at = 0 marks a row created by the backup, to be overwritten by the source refresh; existing chapters keep their newer metadata
		sqlx::query(
			"INSERT OR IGNORE INTO chapter (manga_id, key, title, number, volume, scanlators,
			 language, thumbnail, url, date_uploaded, source_order, page_count, fetched_at)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
		)
		.bind(manga_id)
		.bind(&entry.chapter.key)
		.bind(&entry.chapter.title)
		.bind(entry.chapter.number)
		.bind(entry.chapter.volume)
		.bind(serde_json::to_string(&entry.chapter.scanlators)?)
		.bind(&entry.chapter.language)
		.bind(&entry.chapter.thumbnail)
		.bind(&entry.chapter.url)
		.bind(entry.chapter.date_uploaded)
		.bind(entry.chapter.source_order)
		.bind(entry.chapter.page_count)
		.execute(&mut **tx)
		.await?;
		let (chapter_id,): (i64,) =
			sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? AND key = ?")
				.bind(manga_id)
				.bind(&entry.chapter.key)
				.fetch_one(&mut **tx)
				.await?;
		// is_downloaded points at local files; the backup neither carries nor overwrites it
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked,
			 last_read_at) VALUES (?, ?, ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
			 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at",
		)
		.bind(chapter_id)
		.bind(user_id)
		.bind(entry.last_page_read)
		.bind(entry.is_read)
		.bind(entry.is_bookmarked)
		.bind(entry.last_read_at)
		.execute(&mut **tx)
		.await?;
		restored.progress += 1;
	}
	Ok(())
}

/// Raw exclusion ids are instance-local, so categories carry them by name instead
fn omitted_from_backup(key: &str) -> bool {
	key == crate::update::LIBRARY_EXCLUDED_UPDATE_CATEGORIES
		|| key == crate::download::LIBRARY_EXCLUDED_DOWNLOAD_CATEGORIES
		|| crate::api::SERVER_OWNED_SETTING_KEYS.contains(&key)
}

async fn restore_settings(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &BackupDoc,
	restored: &mut Restored,
) -> ApiResult<()> {
	for (key, value) in &doc.settings {
		// User-scoped keys from the backup are written back into the owner's namespace; literal user.* keys are rejected (same rule as /settings PUT)
		// Instance-owned values must not overwrite the target; category restoration handles raw exclusion ids separately.
		if key.starts_with("user.") || omitted_from_backup(key) {
			continue;
		}
		let stored_key = match crate::api::USER_SCOPED_SETTING_KEYS.contains(&key.as_str()) {
			true => crate::api::user_setting_key(user_id, key),
			false => key.clone(),
		};
		sqlx::query(
			"INSERT INTO setting (key, value) VALUES (?, ?)
			 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		)
		.bind(&stored_key)
		.bind(serde_json::to_string(value)?)
		.execute(&mut **tx)
		.await?;
		restored.settings += 1;
	}
	Ok(())
}

async fn restore_source_settings(
	tx: &mut Transaction<'_, Sqlite>,
	doc: &BackupDoc,
	restored: &mut Restored,
	warnings: &mut Vec<String>,
	touched_sources: &mut HashSet<i64>,
) -> ApiResult<()> {
	for (source_key, values) in &doc.source_settings {
		let Some(source_id) = source_id_by_key(tx, source_key).await? else {
			warnings.push(format!(
				"source not installed: {source_key}; skipped {} source settings",
				values.len()
			));
			continue;
		};
		for (key, value) in values {
			let encoded = serde_json::to_string(&json_to_default_value(value))?;
			sqlx::query(
				"INSERT INTO source_setting (source_id, key, value) VALUES (?, ?, ?)
				 ON CONFLICT (source_id, key) DO UPDATE SET value = excluded.value",
			)
			.bind(source_id)
			.bind(key)
			.bind(encoded)
			.execute(&mut **tx)
			.await?;
			restored.source_settings += 1;
		}
		touched_sources.insert(source_id);
	}
	Ok(())
}
