//! Suwayomi/Mihon backup import (.tachibk / .proto.gz): protobuf wire compatible with Mihon's
//! backup schema, container sniffed by magic bytes — both actually write gzip(proto) (Suwayomi's
//! ProtoBackupExport and Mihon's BackupCreator both go through okio gzip), and Mihon also accepts a bare
//! proto; the zip container is defensive compatibility for forks. See import_merge for the additive merge primitives.
//!
//! The prost messages are a hand-written wire contract (field-number provenance in the PR): undeclared fields
//! (memo, preferences, suwayomi meta 9000+, etc.) are skipped by prost decoding

use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use crate::tracker::core::TrackMode;
use crate::tracker::{Provider, dao};
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::Json;
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::backup::BODY_MAX;
use super::import_merge::{
	ChapterMeta, ImportCounts, MangaMeta, ensure_category, ensure_chapter_row, ensure_manga_row,
	mark_in_library, merge_chapter_progress,
};
use super::key_realign::{RealignCounts, realign_chapter_key, realign_manga_key};

/// Cap on the decompressed/unpacked proto: same tier as the upload body cap, guarding against zip/gzip bombs
const PROTO_MAX: u64 = BODY_MAX as u64;

#[derive(Clone, PartialEq, Message)]
struct TachiBackup {
	#[prost(message, repeated, tag = "1")]
	manga: Vec<TachiManga>,
	#[prost(message, repeated, tag = "2")]
	categories: Vec<TachiCategory>,
	#[prost(message, repeated, tag = "101")]
	sources: Vec<TachiSource>,
}

#[derive(Clone, PartialEq, Message)]
struct TachiManga {
	#[prost(int64, tag = "1")]
	source: i64,
	/// In-source key (called key in 1.x), not a browsable URL
	#[prost(string, tag = "2")]
	url: String,
	#[prost(string, tag = "3")]
	title: String,
	#[prost(string, optional, tag = "4")]
	artist: Option<String>,
	#[prost(string, optional, tag = "5")]
	author: Option<String>,
	#[prost(string, optional, tag = "6")]
	description: Option<String>,
	#[prost(string, repeated, tag = "7")]
	genre: Vec<String>,
	#[prost(int32, tag = "8")]
	status: i32,
	#[prost(string, optional, tag = "9")]
	thumbnail_url: Option<String>,
	#[prost(int64, tag = "13")]
	date_added: i64,
	#[prost(message, repeated, tag = "16")]
	chapters: Vec<TachiChapter>,
	/// Category order values (Mihon's MangaRestorer looks up by order), not array positions
	#[prost(int64, repeated, tag = "17")]
	categories: Vec<i64>,
	/// Tracker bindings (Mihon BackupManga tag 18); TachiTracking declares only the fields the import consumes
	#[prost(message, repeated, tag = "18")]
	tracking: Vec<TachiTracking>,
	/// kotlinx ProtoBuf omits default-valued fields, so favorite=true never goes on the wire; absent means true
	#[prost(bool, optional, tag = "100")]
	favorite: Option<bool>,
	#[prost(message, repeated, tag = "104")]
	history: Vec<TachiHistory>,
}

#[derive(Clone, PartialEq, Message)]
struct TachiChapter {
	#[prost(string, tag = "1")]
	url: String,
	#[prost(string, tag = "2")]
	name: String,
	#[prost(string, optional, tag = "3")]
	scanlator: Option<String>,
	#[prost(bool, tag = "4")]
	read: bool,
	#[prost(int64, tag = "6")]
	last_page_read: i64,
	#[prost(int64, tag = "8")]
	date_upload: i64,
	#[prost(float, tag = "9")]
	chapter_number: f32,
	#[prost(int64, tag = "10")]
	source_order: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiCategory {
	#[prost(string, tag = "1")]
	name: String,
	#[prost(int64, tag = "2")]
	order: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiSource {
	#[prost(string, tag = "1")]
	name: String,
	#[prost(int64, tag = "2")]
	source_id: i64,
}

#[derive(Clone, PartialEq, Message)]
struct TachiHistory {
	#[prost(string, tag = "1")]
	url: String,
	#[prost(int64, tag = "2")]
	last_read: i64,
}

/// Tracker binding nested in each manga. The remote entry id moved from mediaIdInt (tag 3, what
/// Suwayomi-Server still writes) to mediaId (tag 100, Mihon); Mihon's getTrackImpl keeps the int
/// when it is non-zero, so both are declared with the same precedence. Tags 2/4 and 6-12 (library
/// id, tracking URL and the tracker-side progress/score/status/dates) carry nothing the local
/// binding stores.
#[derive(Clone, PartialEq, Message)]
struct TachiTracking {
	#[prost(int32, tag = "1")]
	sync_id: i32,
	#[prost(int32, tag = "3")]
	media_id_int: i32,
	#[prost(string, tag = "5")]
	title: String,
	#[prost(int64, tag = "100")]
	media_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuwayomiImportQuery {
	#[serde(default)]
	pub dry_run: bool,
	/// urlencoded JSON：{ "<mihonSourceId>": "<aidokuSourceKey>" }
	#[serde(default)]
	pub source_mapping: Option<String>,
}

/// Backup timestamps are epoch milliseconds, 0 means no value (Mihon's toInstantOrNull convention)
fn ms_to_unix(ms: i64) -> Option<i64> {
	(ms > 0).then_some(ms / 1000)
}

/// Maps the backup's status enum (SManga: 1 ongoing 2 completed 3 licensed 4 publishing finished 5 cancelled 6 hiatus) onto our 1/2/3/4
fn status_from_tachi(status: i32) -> i64 {
	match status {
		1 => 1,
		2 => 2,
		5 => 3,
		6 => 4,
		_ => 0,
	}
}

/// Mihon/Suwayomi tracker ids (TrackerManager constants; both agree on 1-9, Suwayomi-Server only
/// implements 1-5 and 7): only ids with a local provider land, the rest (Kitsu 3, Shikimori 4,
/// Komga 6, MangaUpdates 7, Kavita 8, Suwayomi 9, Hikka 10, Comick 12) are reported as unsupported.
/// MangaBaka (11) exists only in Mihon.
fn tachi_tracker_provider(sync_id: i32) -> Option<Provider> {
	match sync_id {
		1 => Some(Provider::Mal),
		2 => Some(Provider::Anilist),
		5 => Some(Provider::Bangumi),
		11 => Some(Provider::Mangabaka),
		_ => None,
	}
}

/// Normalizes to lowercase alphanumerics only (Unicode-aware, so CJK source names do not collapse to an empty string)
fn normalize_source_name(name: &str) -> String {
	name.chars()
		.filter(|c| c.is_alphanumeric())
		.flat_map(char::to_lowercase)
		.collect()
}

fn gunzip(bytes: &[u8]) -> ApiResult<Vec<u8>> {
	use std::io::Read;
	let mut out = Vec::new();
	flate2::read::GzDecoder::new(bytes)
		.take(PROTO_MAX + 1)
		.read_to_end(&mut out)
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	if out.len() as u64 > PROTO_MAX {
		return Err(ApiError::bad_request("suwayomi backup too large"));
	}
	Ok(out)
}

/// zip container: take backup.proto, else the sole .proto entry; the entry itself may still be gzip
fn unzip_proto(bytes: &[u8], depth: u8) -> ApiResult<Vec<u8>> {
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	let names: Vec<String> = archive
		.file_names()
		.map(|n| n.map(|cow| cow.into_owned()))
		.collect::<Result<Vec<_>, _>>()
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	let protos: Vec<&str> = names
		.iter()
		.map(String::as_str)
		.filter(|n| n.ends_with(".proto"))
		.collect();
	let entry = if names.iter().any(|n| n == "backup.proto") {
		"backup.proto"
	} else if let [only] = protos.as_slice() {
		only
	} else {
		return Err(ApiError::bad_request("suwayomi backup zip has no proto entry"));
	};
	let inner = aidoku_wasm::package::read_zip_entry(&mut archive, entry, PROTO_MAX)
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;
	unpack(&inner, depth + 1)
}

fn unpack(bytes: &[u8], depth: u8) -> ApiResult<Vec<u8>> {
	if bytes.starts_with(b"\x1f\x8b") {
		return gunzip(bytes);
	}
	if bytes.starts_with(b"PK\x03\x04") {
		if depth >= 1 {
			return Err(ApiError::bad_request("invalid suwayomi backup: nested zip"));
		}
		return unzip_proto(bytes, depth);
	}
	Ok(bytes.to_vec())
}

/// Groups by Mihon source id (keeping first-seen order within the backup, for a stable report)
fn group_by_source(doc: &TachiBackup) -> Vec<(i64, Vec<&TachiManga>)> {
	let mut groups: Vec<(i64, Vec<&TachiManga>)> = Vec::new();
	let mut index: HashMap<i64, usize> = HashMap::new();
	for manga in &doc.manga {
		let pos = match index.get(&manga.source) {
			Some(&pos) => pos,
			None => {
				let pos = groups.len();
				index.insert(manga.source, pos);
				groups.push((manga.source, Vec::new()));
				pos
			}
		};
		groups[pos].1.push(manga);
	}
	groups
}

#[derive(Debug, sqlx::FromRow)]
struct InstalledSource {
	id: i64,
	key: String,
	name: String,
}

/// Where a Mihon source group lands: explicit mapping > unique-hit auto match > unmatched (multiple hits are never guessed)
enum Resolution {
	Mapped { key: String },
	Auto { key: String },
	Unmatched { candidates: Vec<(String, String)> },
}

impl Resolution {
	fn key(&self) -> Option<&str> {
		match self {
			Resolution::Mapped { key } | Resolution::Auto { key } => Some(key),
			Resolution::Unmatched { .. } => None,
		}
	}
}

fn resolve_group(
	mihon_id: i64,
	name: &str,
	mapping: &HashMap<i64, String>,
	installed: &[InstalledSource],
	by_normalized_name: &HashMap<String, Vec<usize>>,
) -> Resolution {
	if let Some(key) = mapping.get(&mihon_id) {
		return Resolution::Mapped { key: key.clone() };
	}
	let normalized = normalize_source_name(name);
	let candidates: Vec<(String, String)> = by_normalized_name
		.get(&normalized)
		.map(|ids| {
			ids.iter()
				.map(|i| (installed[*i].key.clone(), installed[*i].name.clone()))
				.collect()
		})
		.unwrap_or_default();
	// A name normalizing to empty (e.g. pure punctuation) has no matching eligibility; only a unique hit auto-matches
	if !normalized.is_empty() && candidates.len() == 1 {
		return Resolution::Auto {
			key: candidates[0].0.clone(),
		};
	}
	Resolution::Unmatched { candidates }
}

fn parse_mapping(raw: Option<&str>) -> ApiResult<HashMap<i64, String>> {
	let Some(raw) = raw else {
		return Ok(HashMap::new());
	};
	let parsed: HashMap<String, String> = serde_json::from_str(raw)
		.map_err(|e| ApiError::bad_request(format!("invalid sourceMapping: {e}")))?;
	let mut out = HashMap::new();
	for (k, v) in parsed {
		let id: i64 = k
			.parse()
			.map_err(|_| ApiError::bad_request(format!("invalid sourceMapping key: {k}")))?;
		out.insert(id, v);
	}
	Ok(out)
}

pub async fn import_suwayomi_backup(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<SuwayomiImportQuery>,
	body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
	let mapping = parse_mapping(query.source_mapping.as_deref())?;
	let result = import_suwayomi_doc(&state, user.id, &body, query.dry_run, &mapping).await?;
	Ok(Json(result))
}

/// Validation + single-transaction import; dry-run takes the same execution path and rolls back instead of committing at the end, so the report matches a real run
pub async fn import_suwayomi_doc(
	state: &AppState,
	user_id: i64,
	body: &[u8],
	dry_run: bool,
	mapping: &HashMap<i64, String>,
) -> ApiResult<Value> {
	let proto = unpack(body, 0)?;
	let doc = TachiBackup::decode(proto.as_slice())
		.map_err(|e| ApiError::bad_request(format!("invalid suwayomi backup: {e}")))?;

	// Keys are the anchor for all later reconciliation; an empty key cannot land safely in the DB
	for manga in &doc.manga {
		if manga.url.is_empty() {
			return Err(ApiError::bad_request("suwayomi manga entry must have a non-empty url"));
		}
		if manga.chapters.iter().any(|c| c.url.is_empty())
			|| manga.history.iter().any(|h| h.url.is_empty())
		{
			return Err(ApiError::bad_request(
				"suwayomi chapter/history entry must have a non-empty url",
			));
		}
	}

	let source_names: HashMap<i64, &str> = doc
		.sources
		.iter()
		.map(|s| (s.source_id, s.name.as_str()))
		.collect();
	let installed: Vec<InstalledSource> =
		sqlx::query_as("SELECT id, key, name FROM source ORDER BY id")
			.fetch_all(&state.pool)
			.await?;
	let installed_id_by_key: HashMap<&str, i64> = installed
		.iter()
		.map(|s| (s.key.as_str(), s.id))
		.collect();
	for key in mapping.values() {
		if !installed_id_by_key.contains_key(key.as_str()) {
			return Err(ApiError::bad_request(format!(
				"sourceMapping target not installed: {key}"
			)));
		}
	}
	let mut by_normalized_name: HashMap<String, Vec<usize>> = HashMap::new();
	for (i, s) in installed.iter().enumerate() {
		let normalized = normalize_source_name(&s.name);
		if !normalized.is_empty() {
			by_normalized_name.entry(normalized).or_default().push(i);
		}
	}

	let groups = group_by_source(&doc);
	let resolutions: Vec<Resolution> = groups
		.iter()
		.map(|(id, _)| {
			resolve_group(
				*id,
				source_names.get(id).copied().unwrap_or(""),
				mapping,
				&installed,
				&by_normalized_name,
			)
		})
		.collect();

	let mut import = ImportState::default();
	let mut realigned: Vec<RealignCounts> = Vec::with_capacity(groups.len());
	{
		let mut tx = crate::db::begin_write(&state.pool).await?;
		import_categories(&mut tx, user_id, &doc, &mut import.seen_categories).await?;
		for ((mihon_id, mangas), resolution) in groups.iter().zip(&resolutions) {
			let name = source_names.get(mihon_id).copied().unwrap_or("");
			let Some(source_key) = resolution.key() else {
				realigned.push(RealignCounts::default());
				for manga in mangas {
					import.skipped.push(json!({
						"section": if manga.favorite.unwrap_or(true) { "library" } else { "progress" },
						"mihonSourceId": mihon_id.to_string(),
						"mihonSourceName": name,
						"mangaKey": manga.url,
						"reason": "no source mapping",
					}));
				}
				continue;
			};
			let source_id = installed_id_by_key[source_key];
			let source = installed
				.iter()
				.find(|s| s.id == source_id)
				.expect("resolved key is installed");
			let group = GroupImport {
				mihon_id: *mihon_id,
				mihon_name: name,
				source,
			};
			let counts = import_group(&mut tx, user_id, &doc, &group, mangas, &mut import).await?;
			realigned.push(counts);
		}
		import.counts.categories = import.seen_categories.len() as i64;
		if dry_run {
			tx.rollback().await?;
		} else {
			tx.commit().await?;
		}
	}

	let sources_json: Vec<Value> = groups
		.iter()
		.zip(&resolutions)
		.zip(&realigned)
		.map(|(((mihon_id, mangas), resolution), counts)| {
			let name = source_names.get(mihon_id).copied().unwrap_or("");
			let (state_label, key) = match resolution {
				Resolution::Mapped { key } => ("mapped", Some(key.as_str())),
				Resolution::Auto { key } => ("auto", Some(key.as_str())),
				Resolution::Unmatched { .. } => ("unmatched", None),
			};
			let candidates: Vec<Value> = match resolution {
				Resolution::Unmatched { candidates } => candidates
					.iter()
					.map(|(k, n)| json!({"key": k, "name": n}))
					.collect(),
				_ => Vec::new(),
			};
			let mut value = json!({
				"mihonSourceId": mihon_id.to_string(),
				"mihonSourceName": name,
				"mangaCount": mangas.len(),
				"match": state_label,
				"candidates": candidates,
			});
			if let Some(key) = key {
				value["aidokuSourceKey"] = json!(key);
				if let Some(s) = installed.iter().find(|s| s.key == key) {
					value["aidokuSourceName"] = json!(s.name);
				}
			}
			if !counts.is_zero() {
				value["realignedKeys"] = json!({"manga": counts.manga, "chapter": counts.chapter});
			}
			value
		})
		.collect();

	Ok(json!({
		"dryRun": dry_run,
		"imported": {
			"library": import.counts.library,
			"categories": import.counts.categories,
			"progress": import.counts.progress,
			"trackers": import.counts.trackers,
		},
		"skipped": import.skipped,
		"sources": sources_json,
	}))
}

/// The top-level categories array creates rows first; member references look up this table by order
async fn import_categories(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &TachiBackup,
	seen: &mut HashSet<String>,
) -> ApiResult<()> {
	for category in &doc.categories {
		let name = category.name.trim();
		if name.is_empty() {
			return Err(ApiError::bad_request("suwayomi category must have a non-empty name"));
		}
		ensure_category(tx, user_id, name).await?;
		seen.insert(name.to_string());
	}
	Ok(())
}

/// One resolved backup source group: the Mihon identity used in skip reports plus the resolved install target.
struct GroupImport<'a> {
	mihon_id: i64,
	mihon_name: &'a str,
	source: &'a InstalledSource,
}

/// Mutable accumulators threaded through the import: section counts, created categories and the skip report.
#[derive(Default)]
struct ImportState {
	counts: ImportCounts,
	seen_categories: HashSet<String>,
	skipped: Vec<Value>,
}

async fn import_group(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	doc: &TachiBackup,
	group: &GroupImport<'_>,
	mangas: &[&TachiManga],
	import: &mut ImportState,
) -> ApiResult<RealignCounts> {
	// Category membership is an order-value lookup (Mihon MangaRestorer semantics), not an array position
	let category_by_order: HashMap<i64, &str> = doc
		.categories
		.iter()
		.map(|c| (c.order, c.name.as_str()))
		.collect();
	let mut realigned = RealignCounts::default();
	for manga in mangas {
		let meta = MangaMeta {
			title: manga.title.as_str(),
			authors: serde_json::to_string(manga.author.as_slice())?,
			artists: serde_json::to_string(manga.artist.as_slice())?,
			description: manga.description.as_deref(),
			tags: serde_json::to_string(&manga.genre)?,
			status: status_from_tachi(manga.status),
			content_rating: 0,
			cover_url: manga.thumbnail_url.as_deref(),
			// The url column is a browsable source-site page; manga.url is the in-source key, so it is not carried over
			url: None,
		};
		let (manga_key, manga_changed) = realign_manga_key(group.source.key.as_str(), &manga.url);
		realigned.manga += manga_changed as i64;
		let manga_id = ensure_manga_row(tx, group.source.id, &manga_key, Some(&meta)).await?;
		import_tracking(tx, user_id, group, manga, manga_id, import).await?;
		if manga.favorite.unwrap_or(true) {
			let in_library_at = ms_to_unix(manga.date_added).unwrap_or_else(unix_now);
			mark_in_library(tx, user_id, manga_id, in_library_at).await?;
			for order in &manga.categories {
				let Some(name) = category_by_order.get(order) else {
					continue;
				};
				let name = name.trim();
				if name.is_empty() {
					continue;
				}
				let category_id = ensure_category(tx, user_id, name).await?;
				import.seen_categories.insert(name.to_string());
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
			import.counts.library += 1;
		}

		// Chapter and history keys realign identically, so the two still pair up after the rewrite
		let history: HashMap<String, i64> = manga
			.history
			.iter()
			.map(|h| (realign_chapter_key(group.source.key.as_str(), &h.url).0, h.last_read))
			.collect();
		let mut seen_chapters: HashSet<String> = HashSet::new();
		for chapter in &manga.chapters {
			let (chapter_key, changed) = realign_chapter_key(group.source.key.as_str(), &chapter.url);
			realigned.chapter += changed as i64;
			seen_chapters.insert(chapter_key.clone());
			let meta = ChapterMeta {
				title: (!chapter.name.is_empty()).then_some(chapter.name.as_str()),
				number: (chapter.chapter_number != 0.0).then_some(chapter.chapter_number as f64),
				volume: None,
				scanlators: serde_json::to_string(chapter.scanlator.as_slice())?,
				language: None,
				url: None,
				date_uploaded: ms_to_unix(chapter.date_upload),
				source_order: chapter.source_order,
			};
			let chapter_id = ensure_chapter_row(tx, manga_id, &chapter_key, Some(&meta)).await?;
			let last_read_at = history
				.get(chapter_key.as_str())
				.and_then(|ts| ms_to_unix(*ts));
			merge_chapter_progress(
				tx,
				user_id,
				chapter_id,
				chapter.last_page_read.max(0),
				chapter.read,
				last_read_at,
			)
			.await?;
			import.counts.progress += 1;
		}
		// Chapters appearing only in history (the backup ticked history but not chapters): create bare-key rows carrying just the timestamp
		for entry in &manga.history {
			let (entry_key, changed) = realign_chapter_key(group.source.key.as_str(), &entry.url);
			if seen_chapters.contains(entry_key.as_str()) {
				continue;
			}
			realigned.chapter += changed as i64;
			let chapter_id = ensure_chapter_row(tx, manga_id, &entry_key, None).await?;
			merge_chapter_progress(tx, user_id, chapter_id, 0, false, ms_to_unix(entry.last_read)).await?;
			import.counts.progress += 1;
		}
	}
	Ok(realigned)
}

/// Restores the tracker bindings nested in a manga entry: an existing binding for the same
/// (manga, user, provider) wins, and unsupported trackers or entries never synced to the remote
/// (no media id) are reported rather than dropped silently. The backup format has no track mode
/// or chapter offset, so bindings land as auto/0.
async fn import_tracking(
	tx: &mut Transaction<'_, Sqlite>,
	user_id: i64,
	group: &GroupImport<'_>,
	manga: &TachiManga,
	manga_id: i64,
	import: &mut ImportState,
) -> ApiResult<()> {
	for track in &manga.tracking {
		let mut skip = |reason: &str| {
			import.skipped.push(json!({
				"section": "trackers",
				"mihonSourceId": group.mihon_id.to_string(),
				"mihonSourceName": group.mihon_name,
				"mangaKey": manga.url,
				"reason": reason,
			}));
		};
		let Some(provider) = tachi_tracker_provider(track.sync_id) else {
			skip("tracker not supported");
			continue;
		};
		// Mihon getTrackImpl precedence: the legacy int id wins when set, else the long id
		let remote_id = if track.media_id_int != 0 {
			track.media_id_int as i64
		} else {
			track.media_id
		};
		if remote_id <= 0 {
			skip("tracker entry has no remote id");
			continue;
		}
		let track_id = remote_id.to_string();
		dao::insert_link_if_absent(
			tx,
			&dao::NewTrackerLink {
				manga_id,
				user_id,
				provider: provider.as_str(),
				track_id: &track_id,
				title: Some(track.title.trim()).filter(|t| !t.is_empty()),
				track_mode: TrackMode::Auto,
				chapter_offset: 0,
			},
		)
		.await?;
		import.counts.trackers += 1;
	}
	Ok(())
}
