pub mod cover;
pub mod downsample;
pub mod rekey;
pub mod repo;
pub mod routed;

use crate::error::{ApiError, ApiResult};
use crate::model::{
	ChapterRow, MangaRow, chapter_from_row, json_to_default_value, manga_from_row, unix_now,
};
use crate::state::{AppState, load_cookies, load_defaults};
use aidoku::{Chapter, Manga, PageContext};
use aidoku_wasm::{DefaultValue, SourceConfig, SourceHandle};
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

/// What a registration did to the registry: a brand-new source, a version change on an existing
/// row, or a refresh of an unchanged package (metadata re-stamped; the scan reports nothing for it)
#[derive(Debug, PartialEq, Eq)]
pub enum RegisterOutcome {
	Installed,
	Updated,
	Refreshed,
}

#[derive(Debug, Serialize)]
pub struct SkippedSource {
	pub dir: String,
	pub reason: String,
}

/// source.key allows only this character set (reserved prefixes local/komga/kavita/suwayomi)
pub(crate) fn valid_source_key(key: &str) -> bool {
	!key.is_empty()
		&& key
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Directory-mount scan: first-level subdirectories (source.json + main.wasm) and .aix packages (zip, Payload/ layout) both count as source packages
pub async fn scan_sources_dir(state: &AppState, dir: &Path) -> ApiResult<ScanOutcome> {
	let mut outcome_report = ScanOutcome {
		installed: Vec::new(),
		updated: Vec::new(),
		skipped: Vec::new(),
	};
	let mut read_dir = match tokio::fs::read_dir(dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(outcome_report),
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
			scan_one_dir(state, &path).await
		} else {
			register_aix(state, &path).await
		};
		match result {
			Ok((key, outcome)) => match outcome {
				RegisterOutcome::Installed => outcome_report.installed.push(key),
				RegisterOutcome::Updated => outcome_report.updated.push(key),
				// unchanged package re-registered: nothing worth reporting
				RegisterOutcome::Refreshed => {}
			},
			Err(reason) => outcome_report.skipped.push(SkippedSource {
				dir: dir_name,
				reason,
			}),
		}
	}
	Ok(outcome_report)
}

/// The manifest parse result and the payload location; directory sources and .aix share one registration path
struct SourcePackage {
	doc: serde_json::Value,
	wasm_path: String,
	icon_path: Option<String>,
	settings: Option<serde_json::Value>,
}

async fn scan_one_dir(state: &AppState, path: &Path) -> Result<(String, RegisterOutcome), String> {
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
		state,
		SourcePackage {
			doc,
			wasm_path: wasm_path.display().to_string(),
			icon_path,
			settings,
		},
	)
	.await
}

/// The single entry for .aix registration: shared by directory scan and repo install; on success returns (key, the registration outcome)
pub(crate) async fn register_aix(state: &AppState, path: &Path) -> Result<(String, RegisterOutcome), String> {
	let path_buf = path.to_path_buf();
	// Zip parsing is synchronous IO, offloaded to the blocking pool; package contents stay in memory, never extracted to disk
	let (doc, settings, icon) = tokio::task::spawn_blocking(move || read_aix_package(&path_buf))
		.await
		.map_err(|e| format!("join: {e}"))??;
	// The embedded icon lands next to the package for /sources/{id}/icon to read (Aidoku reads icon.png from the unpacked directory)
	let icon_path = write_aix_icon(path, icon).await;
	register_source(
		state,
		SourcePackage {
			doc,
			wasm_path: path.display().to_string(),
			icon_path,
			settings,
		},
	)
	.await
}

/// Writes the .aix's `Payload/icon.png` to `<name>.png` next to the package; without an icon, removes any leftover file.
/// A write failure only warns and does not block registration
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
	// The payload must be genuinely readable (CRC checked along the way); the scan does not retain the bytes
	read_zip_entry(&mut archive, WASM_ENTRY, WASM_PAYLOAD_MAX)?;
	// The icon is optional: old packages or bare wasm packages may not have one
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

/// On success returns (key, the registration outcome); an existing row only counts as Updated
/// when the package version changed — a same-version rescan merely refreshes metadata in place
async fn register_source(
	state: &AppState,
	package: SourcePackage,
) -> Result<(String, RegisterOutcome), String> {
	let pool = &state.pool;
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
	if key == crate::model::LOCAL_SOURCE_KEY {
		return Err(format!("source key {key:?} is reserved for the built-in local source"));
	}
	let name = info
		.get("name")
		.and_then(|v| v.as_str())
		.unwrap_or(&key)
		.to_string();
	let version = info.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
	// Aidoku's breakingChangeVersion lives in the manifest's config section (SourceInfo.Configuration)
	let breaking_change_version = doc
		.get("config")
		.and_then(|c| c.get("breakingChangeVersion"))
		.and_then(|v| v.as_i64())
		.unwrap_or(0);
	// The raw config section lands in the DB: the DTO exposure and the per-source concurrency cap both read it
	let config = doc
		.get("config")
		.filter(|c| c.is_object())
		.map(|c| c.to_string());
	let base_url = info
		.get("url")
		.and_then(|v| v.as_str())
		.map(str::to_string);
	let content_rating = info
		.get("contentRating")
		.and_then(|v| v.as_i64())
		.unwrap_or(0);
	// languages allows the {"code","isDefault"} object form, but the column stores only language codes: every
	// consumer decodes [String], and storing the raw form would 500 the source list and other queries; isDefault is
	// consumed by the synthesis/seeding paths that read the manifest text
	let languages = serde_json::json!(source_languages(doc)
		.into_iter()
		.map(|(code, _)| code)
		.collect::<Vec<_>>())
	.to_string();
	let icon_path = package.icon_path;
	let wasm = package.wasm_path;
	let now = unix_now();

	let existing: Option<(i64, i64)> = sqlx::query_as("SELECT id, version FROM source WHERE key = ?")
		.bind(&key)
		.fetch_optional(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
	// The dynamic value is the source of truth for base_url: once stored, the synthesized setting's candidates and seed and the login host fallback all come from the row
	let (dynamic, features) = resolve_dynamic_base_url(state, &key, &wasm, existing.map(|(id, _)| id), doc)
		.await
		.unwrap_or_else(|| (String::new(), "{}".to_string()));
	let base_url = if dynamic.is_empty() { base_url } else { Some(dynamic) };
	let (id, outcome) = if let Some((id, old_version)) = existing {
		// An existing source keeps enabled and source_setting; only the metadata is refreshed
		sqlx::query(
			"UPDATE source SET name = ?, version = ?, languages = ?, content_rating = ?,
			 base_url = ?, breaking_change_version = ?, config = ?, wasm_path = ?, icon_path = ?, updated_at = ?, features = ? WHERE id = ?",
		)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(breaking_change_version)
		.bind(config)
		.bind(wasm)
		.bind(icon_path)
		.bind(now)
		.bind(&features)
		.bind(id)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
		(id, if old_version == version { RegisterOutcome::Refreshed } else { RegisterOutcome::Updated })
	} else {
		let result = sqlx::query(
			"INSERT INTO source (key, name, version, languages, content_rating, base_url,
			 breaking_change_version, config, wasm_path, icon_path, enabled, installed_at, updated_at, features)
			 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
		)
		.bind(&key)
		.bind(name)
		.bind(version)
		.bind(languages)
		.bind(content_rating)
		.bind(base_url)
		.bind(breaking_change_version)
		.bind(config)
		.bind(wasm)
		.bind(icon_path)
		.bind(now)
		.bind(now)
		.bind(&features)
		.execute(pool)
		.await
		.map_err(|e| format!("db: {e}"))?;
		(result.last_insert_rowid(), RegisterOutcome::Installed)
	};
	seed_source_defaults(pool, id, package.settings.as_ref(), doc).await?;
	Ok((key, outcome))
}

/// Old rows installed before #147/0010 may miss manifest-derived metadata: config (installed before the config
/// column), features (before the 0010 column), and breaking_change_version (always 0 from old registrations that
/// never parsed the manifest, which breaks the rekey check on cross-breaking updates). Read each package and stamp
/// them, COALESCE/MAX filling only the gaps: config/features only fill NULL columns, breaking takes the monotonic
/// MAX (never regressing an existing value). Runs serially in the background after startup without blocking the
/// service; a single source's failure (corrupt package/compile failure) leaves NULL, and the DTO treats that as
/// unknown (the home entry point is not hidden). Newly installed sources are stamped at registration and are not in this pass
pub async fn backfill_source_metadata(pool: &sqlx::SqlitePool) {
	let rows: Vec<(i64, String)> = match sqlx::query_as(
		"SELECT id, wasm_path FROM source WHERE (config IS NULL OR features IS NULL) AND kind = 'aidoku'",
	)
	.fetch_all(pool)
	.await
	{
		Ok(rows) => rows,
		Err(e) => {
			tracing::warn!("backfill source metadata select failed: {e}");
			return;
		}
	};
	for (id, wasm_path) in rows {
		let stamped = tokio::task::spawn_blocking(
			move || -> Result<(Option<String>, String, i64), String> {
				let path = std::path::Path::new(&wasm_path);
				let (config, breaking) = read_manifest_config(path)?;
				let bytes = aidoku_wasm::package::read_wasm_payload(path).map_err(|e| e.to_string())?;
				let features = aidoku_wasm::SourceFeatures::from_wasm_bytes(&bytes)?;
				let features = serde_json::to_string(&features).map_err(|e| e.to_string())?;
				Ok((config, features, breaking))
			},
		)
		.await;
		match stamped {
			Ok(Ok((config, features, breaking))) => {
				if let Err(e) = sqlx::query(
					"UPDATE source SET config = COALESCE(config, ?), features = COALESCE(features, ?),
					 breaking_change_version = MAX(breaking_change_version, ?) WHERE id = ?",
				)
				.bind(config)
				.bind(features)
				.bind(breaking)
				.bind(id)
				.execute(pool)
				.await
				{
					tracing::warn!(source_id = id, "backfill source metadata update failed: {e}");
				}
			}
			Ok(Err(e)) => tracing::warn!(source_id = id, "backfill source metadata failed: {e}"),
			Err(e) => tracing::warn!(source_id = id, "backfill source metadata join failed: {e}"),
		}
	}
}

/// For backfill: reads the manifest's raw config section from the package (pass-through convention identical to
/// registration, unknown keys kept) and config.breakingChangeVersion (the rekey check depends on it; old
/// registrations may never have parsed it). A bare wasm package has no manifest, so return (None, 0) — that row's config stays NULL
fn read_manifest_config(path: &Path) -> Result<(Option<String>, i64), String> {
	use aidoku_wasm::package::{JSON_ENTRY_MAX, MANIFEST_ENTRY, read_zip_entry};
	let is_zip = std::fs::File::open(path)
		.and_then(|mut f| {
			let mut magic = [0u8; 2];
			std::io::Read::read_exact(&mut f, &mut magic).map(|_| magic == *b"PK")
		})
		.unwrap_or(false);
	if !is_zip {
		return Ok((None, 0));
	}
	let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
	let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("invalid archive: {e}"))?;
	let raw = read_zip_entry(&mut archive, MANIFEST_ENTRY, JSON_ENTRY_MAX).map_err(|e| e.to_string())?;
	let doc: serde_json::Value =
		serde_json::from_slice(&raw).map_err(|e| format!("parse {MANIFEST_ENTRY}: {e}"))?;
	let config = doc.get("config").filter(|c| c.is_object());
	let breaking = config
		.and_then(|c| c.get("breakingChangeVersion"))
		.and_then(|v| v.as_i64())
		.unwrap_or(0);
	Ok((config.map(|c| c.to_string()), breaking))
}

/// Mirrors the register(defaults:) semantics of Aidoku installing a source: seeds the language list and settings.json defaults.
/// The source reads these keys at runtime via defaults.get (e.g. mangadex's contentRating) and fails outright when they are missing
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
	// With allowsBaseUrlSelect the app synthesizes a base-url setting defaulting to the first candidate
	// (the dynamically resolved value prepended to info.url + info.urls); sources like copymanga always read defaults.get("url")
	let allows_select = doc
		.get("config")
		.and_then(|c| c.get("allowsBaseUrlSelect"))
		.and_then(|v| v.as_bool())
		.unwrap_or(false);
	if allows_select {
		let persisted: Option<(Option<String>,)> = sqlx::query_as("SELECT base_url FROM source WHERE id = ?")
			.bind(source_id)
			.fetch_optional(pool)
			.await
			.map_err(|e| format!("db: {e}"))?;
		let urls = candidate_urls(doc, persisted.and_then(|row| row.0).as_deref());
		if urls.len() > 1 {
			seeds.push(("url".to_string(), DefaultValue::String(urls[0].clone())));
		}
	}
	if let Some(settings) = settings {
		collect_setting_defaults(settings, &mut seeds);
	}
	seed_defaults(pool, source_id, seeds).await
}

/// Called after a successful `get_settings`: seeds the defaults the source declares in code into the DB, mirroring
/// AidokuRunner's `register(defaults:)` inside getSettings()
pub(crate) async fn seed_dynamic_defaults(
	pool: &SqlitePool,
	source_id: i64,
	settings: &[serde_json::Value],
) -> Result<(), String> {
	let mut seeds = Vec::new();
	collect_dynamic_defaults(settings, &mut seeds);
	seed_defaults(pool, source_id, seeds).await
}

/// Seeds defaults: a missing row is seeded directly; an existing row gets the new default only when its current
/// value still equals the previously seeded one (a materialized UserDefaults register(defaults:) — a key never set
/// always resolves to the current default); user-changed values are kept. Rows stored before the migration have
/// seeded_value NULL and are conservatively treated as user values
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

/// The source-declared base url list: info.url first, then info.urls in order (mirroring AidokuRunner's urls assembly)
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

/// The candidate list for the synthesized base-url setting: the persisted base_url (dynamically resolved value or
/// the package's static value) is prepended per AidokuRunner semantics, unless already in the package list. At
/// registration the row's base_url is always the current package's resolution, and the settings endpoint and seeding
/// share this assembly, so candidates and the default never diverge
pub(crate) fn candidate_urls(doc: &serde_json::Value, persisted: Option<&str>) -> Vec<String> {
	let mut urls = source_urls(doc);
	if let Some(persisted) = persisted
		&& !urls.iter().any(|u| u == persisted)
	{
		urls.insert(0, persisted.to_string());
	}
	urls
}

/// Resolves the dynamic base URL at install/update (gated on features.base_url, mirroring AidokuRunner init's
/// `try? getBaseUrl()`: a missing export, a failed call, or an invalid return value only warns and falls back to the
/// package's static value). The one-shot instance carries the DB's current defaults/cookies — Runner resolves with
/// UserDefaults already holding the last registration's values; a fresh install has none. features is returned too,
/// so registration can store it for the DTO capability bits (e.g. hasHome)
async fn resolve_dynamic_base_url(
	state: &AppState,
	key: &str,
	wasm_path: &str,
	existing_id: Option<i64>,
	doc: &serde_json::Value,
) -> Option<(String, String)> {
	let mut config = SourceConfig::new(key, wasm_path);
	config.limits = state.config.wasm_limits();
	config.policy = state.policy.clone();
	let langs: Vec<String> = source_languages(doc).into_iter().map(|(code, _)| code).collect();
	config.accept_language = aidoku_wasm::default_accept_language(&langs);
	if let Some(id) = existing_id {
		config.defaults = load_defaults(&state.pool, id).await.ok()?;
		config.cookies = load_cookies(&state.pool, id).await.ok()?;
	}
	let (handle, features) = match SourceHandle::spawn(config, state.call_semaphore.clone()).await {
		Ok(spawned) => spawned,
		Err(e) => {
			tracing::warn!(key, "spawn for base url resolution failed: {e}");
			return None;
		}
	};
	let resolved = if features.base_url {
		match handle.get_base_url().await {
			Ok(url) => Some(url),
			Err(e) => {
				tracing::warn!(key, "get_base_url failed: {e}");
				None
			}
		}
	} else {
		None
	};
	handle.shutdown().await;
	let resolved = resolved.filter(|url| {
		let valid = url::Url::parse(url).is_ok();
		if !valid {
			tracing::warn!(key, url, "ignore invalid dynamic base url");
		}
		valid
	});
	let features = serde_json::to_string(&features).unwrap_or_else(|e| {
		tracing::warn!(key, "serialize source features failed: {e}");
		"{}".into()
	});
	Some((resolved.unwrap_or_default(), features))
}

/// The source-declared language list: (code, isDefault), in declaration order; string entries carry no isDefault mark
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

/// With languageSelectType == "single" the language setting is single-choice (key language), otherwise multi-choice (key languages)
pub(crate) fn language_select_single(doc: &serde_json::Value) -> bool {
	doc.get("config")
		.and_then(|c| c.get("languageSelectType"))
		.and_then(|v| v.as_str())
		== Some("single")
}

/// The server has no device language, so the default degrades to isDefault-first, falling back to the first entry
/// (AidokuRunner intersects with the device language; the difference is recorded in docs/architecture.md)
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

/// The language-setting seed for multi-language sources: the setting exists only with >1 language; key and value
/// type follow languageSelectType (multiple → languages: StringArray, single → language: String).
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
		// group and page are both containers: AidokuRunner's loadSettingsDefaults recurses into both
		if matches!(ty, Some("group") | Some("page")) {
			if let Some(children) = item.get("items") {
				collect_setting_defaults(children, out);
			}
			continue;
		}
		// Mirroring AidokuRunner loadSettingsDefaults: an explicit default wins (null counts as absent);
		// when absent, select/picker fall to the first candidate, switch to false, the rest are not seeded
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
		// Seeding Null would still be an invalid value for defaults.get, so don't waste a row
		let value = json_to_default_value(&default);
		if !matches!(value, DefaultValue::Null) {
			out.push((key.to_string(), value));
		}
	}
}

/// Defaults of dynamic settings (the nested shape exported by `get_settings`), same rules as `collect_setting_defaults`:
/// group/page recurse, select/picker without an explicit default fall to the first candidate, the rest are taken only when a default exists
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

/// The default of one dynamic setting item; without an explicit default, select/picker degrade to the first candidate
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

/// Upserts the detail fields by (source_id, key); timestamps are stamped by the caller as needed.
/// Takes a connection rather than the pool: migration and similar flows wrap the upsert in their own transaction
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

/// Chapter reconciliation: source_order takes the index in the source's response; a vanished chapter with a
/// download row is locked and kept, otherwise deleted. Chapter keys in protected are never treated as vanished
/// (entries the local source's scan failed to read — kmrs's failed-directory protection).
/// Returns the chapter ids inserted by this reconciliation (update detection uses them to spot new chapters)
pub async fn reconcile_chapters(
	conn: &mut sqlx::SqliteConnection,
	manga_id: i64,
	chapters: &[Chapter],
	protected: &[String],
) -> ApiResult<Vec<i64>> {
	let now = unix_now();
	// When the source has exactly one language, fill it into language-less chapters (mirroring AidokuRunner)
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
		// whitespace-only titles fold to NULL, matching the Aidoku host's "" -> nil
		// normalization; the webui treats both as "no title" either way
		let title = chapter
			.title
			.as_ref()
			.filter(|t| !t.trim().is_empty())
			.cloned();
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
		.bind(title)
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
		if seen.contains(&key) || protected.contains(&key) {
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

/// Batch row fetch for list endpoints: one IN query instead of a manga_row per row
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

/// The built-in local source (kind = local) has no wasm instance; chapters/pages/covers all take the filesystem branch
pub(crate) async fn source_is_local(pool: &SqlitePool, source_id: i64) -> ApiResult<bool> {
	Ok(source_kind(pool, source_id).await? == crate::model::SOURCE_KIND_LOCAL)
}

/// The built-in OPDS source (kind = opds) likewise has no wasm instance; catalog fetches take the opds_source branch
pub(crate) async fn source_is_opds(pool: &SqlitePool, source_id: i64) -> ApiResult<bool> {
	Ok(source_kind(pool, source_id).await? == crate::model::SOURCE_KIND_OPDS)
}

async fn source_kind(pool: &SqlitePool, source_id: i64) -> ApiResult<String> {
	let kind: Option<(String,)> = sqlx::query_as("SELECT kind FROM source WHERE id = ?")
		.bind(source_id)
		.fetch_optional(pool)
		.await?;
	Ok(kind.map(|(k,)| k).unwrap_or_default())
}

/// Reads the page cache; on miss or refresh, pulls get_page_list from the source and stores the batch in the cache
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
	if source_is_local(&state.pool, manga.source_id).await? {
		let entries = crate::local::chapter_pages(
			&state.config.local_dir(),
			&state.local_zip,
			&manga.key,
			&chapter.key,
		)
		.await?;
		// A local chapter has no materialized blobs, but may have downsample variants: invalidate them when the page
		// rows are rewritten (on the online path, materialize_image_pages' whole-directory removal carries the same duty)
		let _ = tokio::fs::remove_dir_all(state.config.page_blobs_dir().join(chapter_id.to_string())).await;
		let rows: Vec<serde_json::Value> = entries
			.iter()
			.map(|entry| crate::local::page_content_json(entry))
			.collect();
		store_page_rows(&state.pool, chapter_id, &rows).await?;
	} else if source_is_opds(&state.pool, manga.source_id).await? {
		// The CBZ fallback's on-disk cache is invalidated when the page rows are rewritten, same discipline as the blob dir; delete before pulling so a refresh gets the new archive
		let _ = tokio::fs::remove_file(crate::opds_source::cbz_path(state, chapter_id)).await;
		let _ = tokio::fs::remove_dir_all(state.config.page_blobs_dir().join(chapter_id.to_string())).await;
		let rows = crate::opds_source::chapter_page_rows(state, &manga, &chapter).await?;
		store_page_rows(&state.pool, chapter_id, &rows).await?;
	} else {
		let handle = state.get_handle(manga.source_id).await?;
		let mut pages = handle
			.get_page_list(manga_from_row(&manga), chapter_from_row(&chapter))
			.await?;
		fetch_page_descriptions(&handle, &mut pages).await?;
		let contents = materialize_image_pages(state, &handle, chapter_id, &pages).await?;
		let rows: Vec<serde_json::Value> = contents
			.iter()
			.enumerate()
			.map(|(index, content)| {
				let page = &pages[index];
				serde_json::json!({
					"content": content,
					"thumbnail": page.thumbnail,
					"hasDescription": page.has_description,
					"description": page.description,
				})
			})
			.collect();
		store_page_rows(&state.pool, chapter_id, &rows).await?;
	}
	let stored: Vec<(i64, String)> =
		sqlx::query_as("SELECT idx, content FROM page WHERE chapter_id = ? ORDER BY idx ASC")
			.bind(chapter_id)
			.fetch_all(&state.pool)
			.await?;
	Ok(stored)
}

/// Rewrites a chapter's page rows wholesale: delete and re-insert, writing back page_count; the variant cache expires with the directory lifecycle
async fn store_page_rows(pool: &SqlitePool, chapter_id: i64, rows: &[serde_json::Value]) -> ApiResult<()> {
	let mut tx = crate::db::begin_write(pool).await?;
	sqlx::query("DELETE FROM page WHERE chapter_id = ?")
		.bind(chapter_id)
		.execute(&mut *tx)
		.await?;
	for (index, stored) in rows.iter().enumerate() {
		sqlx::query("INSERT INTO page (chapter_id, idx, content) VALUES (?, ?, ?)")
			.bind(chapter_id)
			.bind(index as i64)
			.bind(serde_json::to_string(stored)?)
			.execute(&mut *tx)
			.await?;
	}
	sqlx::query("UPDATE chapter SET page_count = ? WHERE id = ?")
		.bind(rows.len() as i64)
		.bind(chapter_id)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(())
}

/// Pages with has_description but no inlined description call back to the source's get_page_description for the
/// text, per Aidoku semantics; same discipline as image materialization: any page's failure fails the chapter, and the next wholesale rewrite clears the half-done state
async fn fetch_page_descriptions(handle: &SourceHandle, pages: &mut [aidoku::Page]) -> ApiResult<()> {
	for page in pages.iter_mut() {
		if !page.has_description || page.description.is_some() {
			continue;
		}
		page.description = Some(handle.get_page_description(clone_page(page)?).await?);
	}
	Ok(())
}

/// Page does not implement Clone while host calls consume by value: deep-copy via a JSON round trip (the four
/// content variants' serde representations are symmetric, and an Image rid deserializes back as externally_managed, triggering no recycling)
fn clone_page(page: &aidoku::Page) -> ApiResult<aidoku::Page> {
	Ok(serde_json::from_value(serde_json::to_value(page)?)?)
}

/// Image page materialization: an RID is valid only in the current actor session, so once the bytes land in a blob
/// file the JSON stores just the file name ({"Image": "<idx>.png"}) — the only form that still resolves after a
/// restart or actor rebuild. Any page's byte fetch failing fails the whole chapter, and the blob directory's next wholesale rewrite clears the half-done state
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

/// Reads the bytes of a materialized Image page: blob file names are only ever written by load_pages (<idx>.png),
/// but the name is still validated as a single path component against traversal
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

/// Startup sweep: blob directories whose chapter row is gone are removed whole (after chapter reconciliation deletes rows, this is the only place left that can reclaim them)
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

/// Bounded response-body read: a header-declared length over the cap fails fast; compressed/unannounced lengths are
/// stopped by streaming accumulation (the repo::fetch_capped pattern). max = 0 means unlimited
pub(crate) async fn read_body(response: reqwest::Response, max: u64) -> ApiResult<(Vec<u8>, Option<String>)> {
	let mime = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.map(str::to_string);
	let too_large = || ApiError::bad_gateway(format!("upstream body exceeds the {max} byte limit"));
	if max > 0
		&& let Some(len) = response.content_length()
		&& len > max
	{
		return Err(too_large());
	}
	let mut response = response;
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|e| ApiError::bad_gateway(format!("read upstream body failed: {e}")))?
	{
		if max > 0 && bytes.len() as u64 + chunk.len() as u64 > max {
			return Err(too_large());
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok((bytes, mime))
}

fn fetch_err(e: crate::source::routed::FetchError) -> ApiError {
	ApiError::bad_gateway(format!("upstream request failed: {e}"))
}

/// A bare GET without the source's image_request transformation. Every caller is a zip page: a whole-chapter
/// archive is a single large body, so it takes the dedicated higher net_zip_body_max_mb cap instead of the page-image cap
pub async fn http_get(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	let max = state.config.net_zip_body_max_mb * 1024 * 1024;
	if source_is_opds(&state.pool, source_id).await? {
		let catalog = crate::opds_source::Catalog::load(state, source_id).await?;
		return catalog.get_bytes(state, url, max).await;
	}
	let url = reqwest::Url::parse(url).map_err(|_| ApiError::bad_request("invalid upstream url"))?;
	let handle = state.get_handle(source_id).await?;
	let accept_language = handle.accept_language();
	let response = state
		.http
		.get_with(&url, Some(accept_language.as_str()))
		.await
		.map_err(fetch_err)?;
	read_body(response, max).await
}

/// The complete result of an image fetch: bytes plus status and headers, which process_*_image needs to build the ImageResponse
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

/// Fetches an image per the source's capabilities: with image_request, send the request from the source-given spec; otherwise a bare GET
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
		let (bytes, mime) = read_body(response, state.config.net_body_max_mb * 1024 * 1024).await?;
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
	let (bytes, mime) = read_body(response, state.config.net_body_max_mb * 1024 * 1024).await?;
	Ok(RemoteImage {
		bytes,
		mime,
		status,
		response_headers,
		request_headers: spec.headers,
	})
}

/// Processor output is uniformly encoded as PNG (lossless, and the source-side decode chain already exists)
const PROCESSED_MIME: &str = "image/png";

/// Page-image fetch: after fetch_remote, run it through the source's page_image_processor when present;
/// an opds source has neither processor nor instance, so page images take a bare GET with basic credentials
pub async fn fetch_page_image(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
	context: Option<PageContext>,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	if source_is_opds(&state.pool, source_id).await? {
		let catalog = crate::opds_source::Catalog::load(state, source_id).await?;
		return catalog.get_bytes(state, url, state.config.net_body_max_mb * 1024 * 1024).await;
	}
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

/// Cover fetch: same as above, through cover_image_processor; opds sources take the page-image arm
pub async fn fetch_cover_image(
	state: &Arc<AppState>,
	source_id: i64,
	url: &str,
) -> ApiResult<(Vec<u8>, Option<String>)> {
	if source_is_opds(&state.pool, source_id).await? {
		let catalog = crate::opds_source::Catalog::load(state, source_id).await?;
		return catalog.get_bytes(state, url, state.config.net_body_max_mb * 1024 * 1024).await;
	}
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

/// OPDS cover byte fetch: same flow as cover.rs (fresh disk cache first, on miss pull from upstream and write the cache)
pub async fn cover_bytes(state: &Arc<AppState>, manga_id: i64) -> ApiResult<(Vec<u8>, Option<String>)> {
	let manga = manga_row(&state.pool, manga_id).await?;
	let covers_dir = state.config.covers_dir.clone();
	let cache_path = covers_dir.join(manga_id.to_string());
	if let Some((bytes, mime)) = cover_cache_read(&cache_path, manga.last_fetched_at).await {
		return Ok((bytes, Some(mime)));
	}
	let (bytes, mime) = if source_is_local(&state.pool, manga.source_id).await? {
		crate::local::resolve_cover(&state.config.local_dir(), &state.local_zip, &manga.key)
			.await?
			.ok_or_else(|| ApiError::not_found("cover not available"))?
	} else {
		let Some(url) = manga.cover_url.clone() else {
			return Err(ApiError::not_found("cover not available"));
		};
		let (bytes, mime) = fetch_cover_image(state, manga.source_id, &url).await?;
		(bytes, mime.unwrap_or_else(|| COVER_DEFAULT_MIME.into()))
	};
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

/// Cache-hit condition: the file exists and the manga details were not re-fetched after the cache write (last_fetched_at <= mtime)
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

/// Writes tmp first then renames, so a concurrent read never sees a half-written cache
async fn cover_cache_write(covers_dir: &Path, id: i64, bytes: &[u8], mime: &str) -> std::io::Result<()> {
	let tmp = covers_dir.join(format!(".{id}.tmp"));
	tokio::fs::write(&tmp, bytes).await?;
	tokio::fs::rename(&tmp, covers_dir.join(id.to_string())).await?;
	let tmp = covers_dir.join(format!(".{id}.content-type.tmp"));
	tokio::fs::write(&tmp, mime).await?;
	tokio::fs::rename(&tmp, covers_dir.join(format!("{id}.content-type"))).await?;
	Ok(())
}

/// LRU: when the cache file count exceeds the cap, delete oldest by mtime first (sidecar included)
fn cover_cache_enforce_limit(covers_dir: &Path, max: usize) {
	let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
	let Ok(read_dir) = std::fs::read_dir(covers_dir) else {
		return;
	};
	for entry in read_dir.flatten() {
		let name = entry.file_name();
		// Only count the cover bodies (file name is a bare manga id); sidecars and tmp files are handled along with their body
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

/// OPDS-PSE page-image byte fetch: same flow as page.rs (Url via fetch_page_image, Zip unpacked, Image reads the materialized blob)
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
	if let Some(entry) = content.get("Local").and_then(serde_json::Value::as_str) {
		let bytes = crate::local::read_page(
			&state.config.local_dir(),
			&state.local_zip,
			&manga.key,
			&chapter.key,
			entry,
		)
		.await?;
		return Ok((bytes, Some(crate::local::image_mime(entry).to_string())));
	}
	if let Some(entry) = content.get("OpdsZip").and_then(serde_json::Value::as_str) {
		let (bytes, mime) = crate::opds_source::read_cbz_page(state, &manga, &chapter, entry).await?;
		return Ok((bytes, Some(mime)));
	}
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

	#[test]
	fn candidate_urls_prepend_and_dedup() {
		let doc = json!({"info": {"url": "https://a.example", "urls": ["https://b.example"]}});
		assert_eq!(
			candidate_urls(&doc, Some("https://dyn.example")),
			vec!["https://dyn.example", "https://a.example", "https://b.example"]
		);
		// A persisted value already in the package list is not duplicated (when unresolved, the persisted value is info.url)
		assert_eq!(
			candidate_urls(&doc, Some("https://a.example")),
			vec!["https://a.example", "https://b.example"]
		);
		assert_eq!(
			candidate_urls(&doc, None),
			vec!["https://a.example", "https://b.example"]
		);
		// With no url in the package, the persisted value alone becomes a candidate
		let bare = json!({"info": {}});
		assert_eq!(candidate_urls(&bare, Some("https://dyn.example")), vec!["https://dyn.example"]);
	}

	#[test]
	fn clone_page_round_trips_all_content_variants() {
		// The Image variant can only be constructed by serde deserialization (ImageRef::from is private to the vendor crate)
		let raws = [
			json!({"content": {"Text": "body"}, "thumbnail": null, "has_description": true, "description": null}),
			json!({"content": {"Url": ["https://x.test/p.png", null]}, "thumbnail": "https://x.test/t.png",
				"has_description": true, "description": null}),
			json!({"content": {"Zip": ["https://x.test/a.zip", "p.png"]}, "thumbnail": null,
				"has_description": false, "description": "inline"}),
			json!({"content": {"Image": 7}, "thumbnail": null, "has_description": true, "description": null}),
		];
		for raw in raws {
			let page: aidoku::Page = serde_json::from_value(raw.clone()).expect("page");
			let cloned = clone_page(&page).expect("clone");
			assert_eq!(serde_json::to_value(&cloned).expect("json"), raw);
		}
	}

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

	/// Bare HTTP/1.1 mock: with announce_length=false it writes no content-length,
	/// closing the connection instead, to exercise the streaming accumulation path
	fn body_server(body: Vec<u8>, announce_length: bool) -> String {
		use std::io::{Read, Write};
		let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
		let port = listener.local_addr().expect("local addr").port();
		std::thread::spawn(move || {
			for stream in listener.incoming() {
				let Ok(mut stream) = stream else { break };
				let mut head = Vec::new();
				let mut chunk = [0u8; 4096];
				loop {
					match stream.read(&mut chunk) {
						Ok(0) | Err(_) => break,
						Ok(n) => {
							head.extend_from_slice(&chunk[..n]);
							if head.windows(4).any(|w| w == b"\r\n\r\n") {
								break;
							}
						}
					}
				}
				let mut out = b"HTTP/1.1 200 OK\r\nconnection: close\r\n".to_vec();
				if announce_length {
					out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
				}
				out.extend_from_slice(b"\r\n");
				out.extend_from_slice(&body);
				if stream.write_all(&out).is_err() {
					break;
				}
			}
		});
		format!("127.0.0.1:{port}")
	}

	async fn get_response(addr: &str) -> reqwest::Response {
		reqwest::Client::new()
			.get(format!("http://{addr}/x"))
			.send()
			.await
			.expect("send")
	}

	#[tokio::test]
	async fn read_body_at_cap_passes_over_cap_fails() {
		// Boundary pinned: exactly the cap passes, one byte over fails
		let addr = body_server(vec![b'x'; 1024], true);
		let (bytes, _) = read_body(get_response(&addr).await, 1024).await.expect("恰好在帽内");
		assert_eq!(bytes.len(), 1024);

		let addr = body_server(vec![b'x'; 1025], true);
		let err = read_body(get_response(&addr).await, 1024)
			.await
			.expect_err("超一字节必须失败而不是无界缓冲");
		assert!(err.message().contains("exceeds the 1024 byte limit"), "got: {}", err.message());
	}

	#[tokio::test]
	async fn read_body_unannounced_over_cap_fails() {
		// No content-length: stopped by streaming accumulation
		let addr = body_server(vec![b'x'; 2048], false);
		let err = read_body(get_response(&addr).await, 1024)
			.await
			.expect_err("无头长度也必须被帽截停");
		assert!(err.message().contains("exceeds the 1024 byte limit"), "got: {}", err.message());
	}

	#[tokio::test]
	async fn read_body_zero_cap_is_unlimited() {
		// 0 is the explicit escape hatch: a contract documented on the config surface, pinned here
		let addr = body_server(vec![b'x'; 2048], true);
		let (bytes, _) = read_body(get_response(&addr).await, 0).await.expect("0 不设限");
		assert_eq!(bytes.len(), 2048);
	}
}
