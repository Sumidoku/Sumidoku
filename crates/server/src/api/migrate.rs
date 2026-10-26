use super::repo::source_json;
use super::{attach_internal_ids, pins, source_row};
use crate::error::{ApiError, ApiResult};
use crate::model::{SourceRow, unix_now};
use crate::source::{manga_row, reconcile_chapters, repo, upsert_manga};
use crate::state::AppState;
use aidoku::Manga;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Source list for migration: all installed sources (the target-side selector needs them too); mangaCount is the requesting user's in-library count
pub async fn migrate_sources(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let rows: Vec<SourceRow> = sqlx::query_as(
		"SELECT id, key, name, kind, version, languages, content_rating, base_url, breaking_change_version, config, icon_path,
		 wasm_path, enabled, installed_at, updated_at, repo_id, features FROM source ORDER BY id",
	)
	.fetch_all(&state.pool)
	.await?;
	let counts: HashMap<i64, i64> = sqlx::query_as(
		"SELECT m.source_id, COUNT(*) FROM manga m
		 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ? AND mu.in_library = 1
		 GROUP BY m.source_id",
	)
	.bind(user.id)
	.fetch_all(&state.pool)
	.await?
	.into_iter()
	.collect();
	let latest = repo::latest_versions(&state.pool).await?;
	let pinned = pins::pinned_map(&state.pool).await?;
	let out: Vec<Value> = rows
		.iter()
		.map(|row| {
			let mut value = source_json(row, &latest, &pinned);
			if let Some(object) = value.as_object_mut() {
				object.insert(
					"mangaCount".into(),
					json!(counts.get(&row.id).copied().unwrap_or(0)),
				);
			}
			value
		})
		.collect();
	Ok(Json(json!(out)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupQuery {
	pub manga_id: i64,
	pub target_source_id: i64,
	pub query: Option<String>,
}

/// Cross-source title search: goes through the source's normal search path (including its rate limits); candidates
/// get internal ids when already in library; an absent query uses the source entry's title, and a batch row's "manual re-search" passes an explicit query
pub async fn migrate_lookup(
	State(state): State<Arc<AppState>>,
	Query(query): Query<LookupQuery>,
) -> ApiResult<Json<Value>> {
	let origin = manga_row(&state.pool, query.manga_id).await?;
	source_row(&state, query.target_source_id).await?;
	let title = query
		.query
		.filter(|q| !q.trim().is_empty())
		.unwrap_or(origin.title);
	if title.trim().is_empty() {
		return Err(ApiError::bad_request("lookup query is empty"));
	}
	// An opds target has no wasm instance: take the catalog search arm (OpenSearch/traversal fallback), the degraded flag passed through
	if crate::source::source_is_opds(&state.pool, query.target_source_id).await? {
		let outcome = crate::internal::opds_source::search(&state, query.target_source_id, Some(&title)).await?;
		let candidates = attach_internal_ids(&state, query.target_source_id, outcome.entries).await?;
		return Ok(Json(json!({
			"query": title,
			"candidates": candidates,
			"degraded": outcome.degraded,
		})));
	}
	let handle = state.get_handle(query.target_source_id).await?;
	let result = handle
		.get_search_manga_list(Some(title.clone()), 1, Vec::new())
		.await?;
	let candidates = attach_internal_ids(&state, query.target_source_id, result.entries).await?;
	Ok(Json(json!({
		"query": title,
		"candidates": candidates,
	})))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MigrateMode {
	Copy,
	Migrate,
	MigrateWithHistory,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateEntry {
	pub from_manga_id: i64,
	pub to_source_id: i64,
	pub to_manga_key: String,
	/// The candidate entry in the client's hands; when present it becomes the get_manga_update input (same semantics as register_manga)
	pub to_manga: Option<Manga>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateRequest {
	pub mode: MigrateMode,
	pub entries: Vec<MigrateEntry>,
}

struct EntryOutcome {
	to_manga_id: i64,
	warnings: Vec<String>,
}

struct EntryFailure {
	reason: &'static str,
	message: Option<String>,
}

/// Entry-level failures are kept apart from 500s: the reason goes into the per-entry result, while database-class errors still blow up the whole request
enum EntryRun {
	Ok(EntryOutcome),
	Failed(EntryFailure),
}

fn failed(reason: &'static str) -> EntryRun {
	EntryRun::Failed(EntryFailure {
		reason,
		message: None,
	})
}

fn failed_with(reason: &'static str, message: String) -> EntryRun {
	EntryRun::Failed(EntryFailure {
		reason,
		message: Some(message),
	})
}

/// Parses the body by hand: axum's Json rejection is 422, while the endpoint convention for invalid shapes/invalid mode is 400
pub async fn migrate_execute(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let body = body.ok_or_else(|| ApiError::bad_request("missing body"))?;
	let request: MigrateRequest = serde_json::from_value(body.0)
		.map_err(|e| ApiError::bad_request(format!("invalid migrate request: {e}")))?;
	if request.entries.is_empty() {
		return Err(ApiError::bad_request("entries must not be empty"));
	}
	for entry in &request.entries {
		if entry.to_manga_key.is_empty() {
			return Err(ApiError::bad_request("toMangaKey must not be empty"));
		}
	}
	// The batch runs serially: one transaction per entry, a partial failure does not roll back completed entries, each gets ok/reason
	let mut results = Vec::with_capacity(request.entries.len());
	for entry in &request.entries {
		let run = run_entry(&state, user.id, request.mode, entry).await?;
		let value = match run {
			EntryRun::Ok(outcome) => json!({
				"fromMangaId": entry.from_manga_id,
				"ok": true,
				"toMangaId": outcome.to_manga_id,
				"warnings": outcome.warnings,
			}),
			EntryRun::Failed(failure) => json!({
				"fromMangaId": entry.from_manga_id,
				"ok": false,
				"reason": failure.reason,
				"message": failure.message,
			}),
		};
		results.push(value);
	}
	Ok(Json(json!({ "results": results })))
}

async fn run_entry(
	state: &Arc<AppState>,
	user_id: i64,
	mode: MigrateMode,
	entry: &MigrateEntry,
) -> ApiResult<EntryRun> {
	let Ok(origin) = manga_row(&state.pool, entry.from_manga_id).await else {
		return Ok(failed("not_found"));
	};
	if source_row(state, entry.to_source_id).await.is_err() {
		return Ok(failed("source_not_found"));
	}
	// Same source with a different key is the breaking-change forceMigrate fallback path (the new key starts a fresh row,
	// progress carried by chapter number); only migrating onto yourself in place (same key) would wipe your own progress, so it must be refused
	if origin.source_id == entry.to_source_id && origin.key == entry.to_manga_key {
		return Ok(failed("same_source"));
	}
	let origin_user: Option<(bool, Option<i64>)> =
		sqlx::query_as("SELECT in_library, in_library_at FROM manga_user WHERE manga_id = ? AND user_id = ?")
			.bind(origin.id)
			.bind(user_id)
			.fetch_optional(&state.pool)
			.await?;
	if !origin_user.map(|(flag, _)| flag).unwrap_or(false) {
		return Ok(failed("not_in_library"));
	}

	// If the target already exists with chapters, reuse it directly; otherwise fetch details from the source, initializing chapters along the way (outside the transaction; a failed fetch counts as an entry failure)
	let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM manga WHERE source_id = ? AND key = ?")
		.bind(entry.to_source_id)
		.bind(&entry.to_manga_key)
		.fetch_optional(&state.pool)
		.await?;
	let mut to_manga_id = None;
	let mut fetched = None;
	if let Some((id,)) = existing {
		let (chapters,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ?")
			.bind(id)
			.fetch_one(&state.pool)
			.await?;
		if chapters > 0 {
			to_manga_id = Some(id);
		}
	}
	if to_manga_id.is_none() {
		let mut initial = entry.to_manga.clone().unwrap_or_default();
		initial.key = entry.to_manga_key.clone();
		// An opds target has no wasm instance: fetching upstream is a catalog feed pull (the url comes with the candidate entry)
		if crate::source::source_is_opds(&state.pool, entry.to_source_id).await? {
			match crate::internal::opds_source::manga_update(state, entry.to_source_id, initial).await {
				Ok(updated) => fetched = Some(updated),
				Err(e) => return Ok(failed_with("fetch_failed", e.message().to_string())),
			}
		} else {
			let handle = state.get_handle(entry.to_source_id).await?;
			match handle.get_manga_update(initial, true, true).await {
				Ok(updated) => fetched = Some(updated),
				Err(e) => return Ok(failed_with("fetch_failed", e.to_string())),
			}
		}
	}

	let mut warnings = Vec::new();
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let to_manga_id = match (to_manga_id, &fetched) {
		(Some(id), _) => id,
		(None, Some(updated)) => {
			let id = upsert_manga(&mut tx, entry.to_source_id, updated, true, true, true).await?;
			reconcile_chapters(&mut tx, id, &updated.chapters.clone().unwrap_or_default(), &[]).await?;
			id
		}
		(None, None) => unreachable!("fetched is Some when to_manga_id is None"),
	};

	// In-library state carry-over: a target already shelved keeps its own first-shelved time, otherwise it inherits the source entry's
	let origin_at: Option<i64> =
		sqlx::query_scalar("SELECT in_library_at FROM manga_user WHERE manga_id = ? AND user_id = ?")
			.bind(origin.id)
			.bind(user_id)
			.fetch_optional(&mut *tx)
			.await?;
	sqlx::query(
		"INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, ?, 1, ?)
		 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 1",
	)
	.bind(to_manga_id)
	.bind(user_id)
	.bind(origin_at.unwrap_or_else(unix_now))
	.execute(&mut *tx)
	.await?;

	// Categories are a union: the target's existing categories are kept, the source entry's are added in
	let category_ids: Vec<i64> =
		sqlx::query_scalar("SELECT category_id FROM category_manga WHERE manga_id = ? AND user_id = ?")
			.bind(origin.id)
			.bind(user_id)
			.fetch_all(&mut *tx)
			.await?;
	for category_id in &category_ids {
		sqlx::query("INSERT OR IGNORE INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, ?)")
			.bind(category_id)
			.bind(to_manga_id)
			.bind(user_id)
			.execute(&mut *tx)
			.await?;
	}

	let origin_progress: Vec<OriginProgress> = sqlx::query_as(
		"SELECT c.key, c.number, c.volume, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		 WHERE cu.user_id = ? AND c.manga_id = ?
		 AND (cu.last_page_read > 0 OR cu.is_read = 1 OR cu.is_bookmarked = 1 OR cu.last_read_at IS NOT NULL)",
	)
	.bind(user_id)
	.bind(origin.id)
	.fetch_all(&mut *tx)
	.await?;
	let targets: Vec<TargetChapter> = sqlx::query_as(
		"SELECT id, number, volume, source_order FROM chapter WHERE manga_id = ? ORDER BY source_order ASC",
	)
	.bind(to_manga_id)
	.fetch_all(&mut *tx)
	.await?;
	for row in &origin_progress {
		let Some(target) = match_target(&targets, row) else {
			warnings.push(format!(
				"progress not carried: no target chapter matches source chapter {}",
				chapter_label(row)
			));
			continue;
		};
		// Existing target progress keeps the newer values: page number takes the max, read/bookmarked take OR, read time takes the newer
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked, last_read_at)
			 VALUES (?, ?, ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 last_page_read = MAX(last_page_read, excluded.last_page_read),
			 is_read = MAX(is_read, excluded.is_read),
			 is_bookmarked = MAX(is_bookmarked, excluded.is_bookmarked),
			 last_read_at = CASE
			 WHEN excluded.last_read_at IS NULL THEN last_read_at
			 WHEN last_read_at IS NULL THEN excluded.last_read_at
			 ELSE MAX(last_read_at, excluded.last_read_at) END",
		)
		.bind(target.id)
		.bind(user_id)
		.bind(row.last_page_read)
		.bind(row.is_read)
		.bind(row.is_bookmarked)
		.bind(row.last_read_at)
		.execute(&mut *tx)
		.await?;
	}

	match mode {
		MigrateMode::Copy => {}
		MigrateMode::Migrate | MigrateMode::MigrateWithHistory => {
			sqlx::query(
				"INSERT INTO manga_user (manga_id, user_id, in_library) VALUES (?, ?, 0)
				 ON CONFLICT (manga_id, user_id) DO UPDATE SET in_library = 0",
			)
			.bind(origin.id)
			.bind(user_id)
			.execute(&mut *tx)
			.await?;
			sqlx::query("DELETE FROM category_manga WHERE manga_id = ? AND user_id = ?")
				.bind(origin.id)
				.bind(user_id)
				.execute(&mut *tx)
				.await?;
			// chapter_user also carries the download flag, so clearing progress only resets the progress fields without deleting the row
			if mode == MigrateMode::Migrate {
				sqlx::query(
					"UPDATE chapter_user SET last_page_read = 0, is_read = 0, is_bookmarked = 0,
					 last_read_at = NULL
					 WHERE user_id = ? AND chapter_id IN (SELECT id FROM chapter WHERE manga_id = ?)",
				)
				.bind(user_id)
				.bind(origin.id)
				.execute(&mut *tx)
				.await?;
			}
		}
	}
	tx.commit().await?;
	Ok(EntryRun::Ok(EntryOutcome {
		to_manga_id,
		warnings,
	}))
}

#[derive(Debug, sqlx::FromRow)]
struct OriginProgress {
	key: String,
	number: Option<f64>,
	volume: Option<f64>,
	last_page_read: i64,
	is_read: bool,
	is_bookmarked: bool,
	last_read_at: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct TargetChapter {
	id: i64,
	number: Option<f64>,
	volume: Option<f64>,
}

/// Exact chapter-number match (duplicates take the smallest source_order); only when the source chapter has no
/// chapter number does the volume number serve as fallback, and then only onto a target chapter that likewise has no chapter number, so volumes never swallow normal chapters
fn match_target<'t>(targets: &'t [TargetChapter], row: &OriginProgress) -> Option<&'t TargetChapter> {
	if let Some(number) = row.number {
		return targets.iter().find(|t| t.number == Some(number));
	}
	if let Some(volume) = row.volume {
		return targets.iter().find(|t| t.number.is_none() && t.volume == Some(volume));
	}
	None
}

fn chapter_label(row: &OriginProgress) -> String {
	if let Some(number) = row.number {
		return format!("number {number}");
	}
	if let Some(volume) = row.volume {
		return format!("volume {volume}");
	}
	format!("key {}", row.key)
}
