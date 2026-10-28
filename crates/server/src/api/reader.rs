use crate::api::{PageQuery, chapter_json, manga_json_by_ids, progress_of};
use crate::error::{ApiError, ApiResult};
use crate::model::{ChapterProgress, ChapterUserRow, unix_now};
use crate::source::chapter_row;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPatch {
	pub last_page_read: Option<i64>,
	pub is_read: Option<bool>,
	pub is_bookmarked: Option<bool>,
}

impl ProgressPatch {
	fn is_empty(&self) -> bool {
		self.last_page_read.is_none() && self.is_read.is_none() && self.is_bookmarked.is_none()
	}

	/// Writing lastPageRead counts as having read: only its presence refreshes last_read_at;
	/// under incognito (global or per-manga, either on) no new stamp is written and the old stamp stays untouched.
	/// Marking unread (a bare isRead: false, with no lastPageRead in the same patch) instead wipes the
	/// whole reading trace - Aidoku's mark-unread removes the history entry - so progress, the resume
	/// target and the history stamp all go; the bookmark is untouched. The wipe is explicit user intent
	/// and applies even under incognito. A patch carrying both lastPageRead and isRead:false is a plain
	/// progress write and keeps the merge semantics.
	pub(crate) async fn apply(
		&self,
		tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
		user_id: i64,
		chapter_id: i64,
	) -> ApiResult<ChapterProgress> {
		let current = read_progress(&mut **tx, user_id, chapter_id).await?;
		let stamp = self.last_page_read.is_some() && !incognito_on(tx, user_id, chapter_id).await?;
		// A patch that also carries lastPageRead is a progress write (the reader reports pages with
		// isRead:false mid-chapter), not a mark-unread; only a bare isRead:false means "mark unread".
		let marking_unread = self.is_read == Some(false) && self.last_page_read.is_none();
		let merged = ChapterProgress {
			last_page_read: if marking_unread {
				0
			} else {
				self.last_page_read.unwrap_or(current.last_page_read)
			},
			is_read: self.is_read.unwrap_or(current.is_read),
			is_bookmarked: self.is_bookmarked.unwrap_or(current.is_bookmarked),
			last_read_at: if marking_unread {
				None
			} else if stamp {
				Some(unix_now())
			} else {
				current.last_read_at
			},
		};
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, last_page_read, is_read, is_bookmarked,
			 last_read_at)
			 VALUES (?, ?, ?, ?, ?, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET
			 last_page_read = excluded.last_page_read, is_read = excluded.is_read,
			 is_bookmarked = excluded.is_bookmarked, last_read_at = excluded.last_read_at",
		)
		.bind(chapter_id)
		.bind(user_id)
		.bind(merged.last_page_read)
		.bind(merged.is_read)
		.bind(merged.is_bookmarked)
		.bind(merged.last_read_at)
		.execute(&mut **tx)
		.await?;
		Ok(merged)
	}
}

/// Incognito check: the user's namespace key user.<id>.incognito.enabled or the manga_user.incognito of the
/// chapter's manga — either on pauses stamping (aligned with Aidoku's pause history: no new history, old history untouched)
async fn incognito_on(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, user_id: i64, chapter_id: i64) -> ApiResult<bool> {
	let global: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = ?")
		.bind(crate::api::user_setting_key(user_id, "incognito.enabled"))
		.fetch_optional(&mut **tx)
		.await?;
	if global
		.and_then(|(raw,)| serde_json::from_str::<bool>(&raw).ok())
		.unwrap_or(false)
	{
		return Ok(true);
	}
	let per_manga: Option<(bool,)> = sqlx::query_as(
		"SELECT mu.incognito FROM chapter c
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = ?
		 WHERE c.id = ?",
	)
	.bind(user_id)
	.bind(chapter_id)
	.fetch_optional(&mut **tx)
	.await?;
	Ok(per_manga.map(|(on,)| on).unwrap_or(false))
}

async fn read_progress<'e, E>(executor: E, user_id: i64, chapter_id: i64) -> ApiResult<ChapterProgress>
where
	E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
	let row: Option<(i64, bool, bool, Option<i64>)> = sqlx::query_as(
		"SELECT last_page_read, is_read, is_bookmarked, last_read_at FROM chapter_user
		 WHERE chapter_id = ? AND user_id = ?",
	)
	.bind(chapter_id)
	.bind(user_id)
	.fetch_optional(executor)
	.await?;
	Ok(match row {
		Some((last_page_read, is_read, is_bookmarked, last_read_at)) => ChapterProgress {
			last_page_read,
			is_read,
			is_bookmarked,
			last_read_at,
		},
		None => ChapterProgress::default(),
	})
}

fn progress_json(chapter_id: i64, progress: &ChapterProgress) -> Value {
	json!({
		"chapterId": chapter_id,
		"lastPageRead": progress.last_page_read,
		"isRead": progress.is_read,
		"isBookmarked": progress.is_bookmarked,
		"lastReadAt": progress.last_read_at,
	})
}

#[derive(Clone, Copy)]
enum ReadDeletion {
	Manual,
	Finished,
}

fn read_deletion(patch: &ProgressPatch, page_count: i64) -> Option<ReadDeletion> {
	if patch.is_read == Some(true) && patch.last_page_read.is_none() {
		Some(ReadDeletion::Manual)
	} else if page_count > 0 && patch.last_page_read == Some(page_count - 1) {
		Some(ReadDeletion::Finished)
	} else {
		None
	}
}

async fn delete_after_progress(
	state: &Arc<AppState>,
	user_id: i64,
	chapter_id: i64,
	deletion: ReadDeletion,
	progress: &ChapterProgress,
	settings: &crate::download::DownloadSettings,
) -> ApiResult<()> {
	let enabled = match deletion {
		ReadDeletion::Manual => settings.delete_after_manual_read,
		ReadDeletion::Finished => settings.delete_after_read,
	};
	if !enabled || (progress.is_bookmarked && !settings.allow_delete_bookmarked) {
		return Ok(());
	}
	state.downloads().delete(chapter_id, user_id).await?;
	Ok(())
}

pub async fn update_chapter(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	Json(patch): Json<ProgressPatch>,
) -> ApiResult<Json<Value>> {
	let chapter = chapter_row(&state.pool, id).await?;
	// progress on a trashed chapter is meaningless (its files are gone): refuse the write
	if chapter.trashed_at.is_some() {
		return Err(ApiError::not_found("chapter is in the trash"));
	}
	if patch.is_empty() {
		let progress = read_progress(&state.pool, user.id, id).await?;
		return Ok(Json(progress_json(id, &progress)));
	}
	let deletion = read_deletion(&patch, chapter.page_count);
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let progress = patch.apply(&mut tx, user.id, id).await?;
	tx.commit().await?;
	if progress.is_read {
		notify_tracker_read(&state, chapter.manga_id, user.id, None);
	}
	if let Some(deletion) = deletion {
		let settings = crate::download::DownloadSettings::load(&state.pool, user.id, &state.config).await?;
		delete_after_progress(&state, user.id, id, deletion, &progress, &settings).await?;
	}
	Ok(Json(progress_json(id, &progress)))
}

/// Lossless tap for the tracker sync: it re-reads the current progress anyway,
/// so the payload is just the pair and a miss only costs one idle round trip.
/// `skip` excludes one provider from the push (a pull sync must not echo back
/// to the tracker it just read from; Aidoku's skipTracker).
pub(crate) fn notify_tracker_read(state: &AppState, manga_id: i64, user_id: i64, skip: Option<crate::tracker::Provider>) {
	if let Some(tx) = state.tracker_tx.get() {
		let _ = tx.send(crate::tracker::sync::TrackerSyncEvent::Progress { manga_id, user_id, skip });
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchUpdateRequest {
	#[serde(default)]
	pub chapter_ids: Vec<i64>,
	#[serde(default)]
	pub manga_ids: Vec<i64>,
	#[serde(flatten)]
	pub patch: ProgressPatch,
}

/// Whole-manga selection expands to the manga's untrashed chapters. Marking read additionally
/// requires a readable chapter: a locked one vanished from the source and can only still be read
/// from its downloaded artifact, so it is skipped until the user has it downloaded.
async fn expand_manga_chapters(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	user_id: i64,
	manga_ids: &[i64],
	mark_read: bool,
) -> ApiResult<Vec<i64>> {
	let readable = if mark_read {
		" AND (c.locked = 0 OR EXISTS (SELECT 1 FROM chapter_user cu WHERE cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_downloaded = 1))"
	} else {
		""
	};
	let mut chapter_ids = Vec::new();
	for chunk in manga_ids.chunks(crate::api::MANGA_ID_QUERY_CHUNK) {
		let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let sql = format!(
			"SELECT c.id FROM chapter c WHERE c.manga_id IN ({placeholders}) AND c.trashed_at IS NULL{readable}"
		);
		let mut query = sqlx::query_scalar::<_, i64>(&sql);
		for manga_id in chunk {
			query = query.bind(manga_id);
		}
		if mark_read {
			query = query.bind(user_id);
		}
		chapter_ids.extend(query.fetch_all(&mut **tx).await?);
	}
	Ok(chapter_ids)
}

pub async fn batch_update_chapters(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<BatchUpdateRequest>,
) -> ApiResult<Json<Value>> {
	if body.patch.is_empty() || (body.chapter_ids.is_empty() && body.manga_ids.is_empty()) {
		return Ok(Json(json!({ "updated": 0 })));
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let expanded = expand_manga_chapters(&mut tx, user.id, &body.manga_ids, body.patch.is_read == Some(true)).await?;
	// chapter ids keep the single-chapter guard semantics; the manga expansion merges deduplicated
	let mut seen = std::collections::HashSet::new();
	let chapter_ids: Vec<i64> = body
		.chapter_ids
		.iter()
		.chain(expanded.iter())
		.filter(|chapter_id| seen.insert(**chapter_id))
		.copied()
		.collect();
	let mut updated = 0;
	let mut deletions = Vec::new();
	let mut read_chapter_ids = Vec::new();
	for chapter_id in &chapter_ids {
		let page_count: Option<(i64, Option<i64>)> = sqlx::query_as(
			"SELECT page_count, trashed_at FROM chapter WHERE id = ?",
		)
		.bind(chapter_id)
		.fetch_optional(&mut *tx)
		.await?;
		let Some((page_count, trashed_at)) = page_count else {
			continue;
		};
		// progress on a trashed chapter is meaningless (its files are gone): refuse the write the same way
		// the single-chapter and read paths do, or a batch update would bypass the guard
		if trashed_at.is_some() {
			continue;
		}
		let progress = body.patch.apply(&mut tx, user.id, *chapter_id).await?;
		if let Some(deletion) = read_deletion(&body.patch, page_count) {
			deletions.push((*chapter_id, progress, deletion));
		}
		if progress.is_read {
			read_chapter_ids.push(*chapter_id);
		}
		updated += 1;
	}
	tx.commit().await?;
	if !read_chapter_ids.is_empty() && state.tracker_tx.get().is_some() {
		for chunk in read_chapter_ids.chunks(crate::api::MANGA_ID_QUERY_CHUNK) {
			let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
			let sql = format!("SELECT DISTINCT manga_id FROM chapter WHERE id IN ({placeholders})");
			let mut query = sqlx::query_scalar::<_, i64>(&sql);
			for chapter_id in chunk {
				query = query.bind(chapter_id);
			}
			for manga_id in query.fetch_all(&state.pool).await? {
				notify_tracker_read(&state, manga_id, user.id, None);
			}
		}
	}
	if !deletions.is_empty() {
		let settings = crate::download::DownloadSettings::load(&state.pool, user.id, &state.config).await?;
		for (chapter_id, progress, deletion) in deletions {
			delete_after_progress(&state, user.id, chapter_id, deletion, &progress, &settings).await?;
		}
	}
	Ok(Json(json!({ "updated": updated })))
}

pub async fn get_history(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let rows: Vec<ChapterUserRow> = sqlx::query_as(
		"SELECT c.*, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter_user cu JOIN chapter c ON c.id = cu.chapter_id
		 WHERE cu.user_id = ? AND cu.last_read_at IS NOT NULL
		 ORDER BY cu.last_read_at DESC, cu.chapter_id DESC
		 LIMIT ? OFFSET ?",
	)
	.bind(user.id)
	.bind(PAGE_SIZE + 1)
	.bind((page as i64 - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next_page = rows.len() as i64 > PAGE_SIZE;
	let page_rows: Vec<&ChapterUserRow> = rows.iter().take(PAGE_SIZE as usize).collect();
	let manga_ids: Vec<i64> = page_rows.iter().map(|row| row.chapter.manga_id).collect();
	let manga_values = manga_json_by_ids(&state, user.id, &manga_ids).await?;
	let mut entries = Vec::with_capacity(page_rows.len());
	for (row, manga_value) in page_rows.into_iter().zip(manga_values) {
		let progress = progress_of(row).unwrap_or_default();
		entries.push(json!({
			"chapter": chapter_json(&row.chapter, Some(&progress))?,
			"manga": manga_value,
			"lastReadAt": row.last_read_at,
			"lastPageRead": progress.last_page_read,
			"isRead": progress.is_read,
		}));
	}
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": has_next_page,
	})))
}

#[cfg(test)]
mod tests {
	use super::*;

	async fn reader_db() -> (Arc<AppState>, i64, i64, tempfile::TempDir) {
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
			"INSERT INTO source (key, name, wasm_path) VALUES ('reader-source', 'Source', '') RETURNING id",
		)
		.fetch_one(&pool)
		.await
		.expect("source row");
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key) VALUES (?, 'manga') RETURNING id")
			.bind(source_id)
			.fetch_one(&pool)
			.await
			.expect("manga row");
		let chapter_id: i64 = sqlx::query_scalar(
			"INSERT INTO chapter (manga_id, key, page_count) VALUES (?, 'chapter', 3) RETURNING id",
		)
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

	async fn set_setting(state: &AppState, key: &str, value: bool) {
		sqlx::query(
			"INSERT INTO setting (key, value) VALUES (?, ?)
			 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		)
		.bind(key)
		.bind(if value { "true" } else { "false" })
		.execute(&state.pool)
		.await
		.expect("set setting");
	}

	async fn add_chapter(state: &AppState, manga_id: i64, key: &str) -> i64 {
		sqlx::query_scalar("INSERT INTO chapter (manga_id, key, page_count) VALUES (?, ?, 3) RETURNING id")
			.bind(manga_id)
			.bind(key)
			.fetch_one(&state.pool)
			.await
			.expect("chapter row")
	}

	async fn insert_download(state: &AppState, chapter_id: i64, path: &str) {
		std::fs::create_dir_all(&state.config.downloads_dir).expect("downloads dir");
		std::fs::write(crate::download::product_path(&state.config.downloads_dir, path), b"download").expect("artifact");
		sqlx::query("INSERT INTO download (chapter_id, user_id, status, format, path) VALUES (?, 1, 'finished', 'cbz', ?)")
			.bind(chapter_id)
			.bind(path)
			.execute(&state.pool)
			.await
			.expect("download row");
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, 1, 1)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 1",
		)
		.bind(chapter_id)
		.execute(&state.pool)
		.await
		.expect("download flag");
	}

	async fn set_bookmarked(state: &AppState, chapter_id: i64, bookmarked: bool) {
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_bookmarked) VALUES (?, 1, ?)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_bookmarked = excluded.is_bookmarked",
		)
		.bind(chapter_id)
		.bind(bookmarked)
		.execute(&state.pool)
		.await
		.expect("bookmark");
	}

	async fn apply_patch(state: &Arc<AppState>, chapter_id: i64, patch: ProgressPatch) {
		let _ = update_chapter(
			State(state.clone()),
			Extension(state.auth.admin_user()),
			Path(chapter_id),
			Json(patch),
		)
		.await
		.expect("progress patch");
	}

	async fn download_count(state: &AppState, chapter_id: i64) -> i64 {
		sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE chapter_id = ? AND user_id = 1")
			.bind(chapter_id)
			.fetch_one(&state.pool)
			.await
			.expect("download count")
	}

	#[tokio::test]
	async fn manual_and_reader_finished_deletion_have_independent_switches() {
		let (state, _manga_id, chapter_id, _tmp) = reader_db().await;
		set_setting(&state, crate::download::DELETE_AFTER_MANUAL_READ, true).await;
		set_setting(&state, crate::download::DELETE_AFTER_READ, false).await;
		insert_download(&state, chapter_id, "chapter.cbz").await;
		apply_patch(
			&state,
			chapter_id,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, chapter_id).await, 0);
		assert!(!state.config.downloads_dir.join("chapter.cbz").exists());
		apply_patch(
			&state,
			chapter_id,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;

		insert_download(&state, chapter_id, "chapter.cbz").await;
		set_setting(&state, crate::download::DELETE_AFTER_MANUAL_READ, false).await;
		set_setting(&state, crate::download::DELETE_AFTER_READ, true).await;
		apply_patch(
			&state,
			chapter_id,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, chapter_id).await, 1);
		apply_patch(
			&state,
			chapter_id,
			ProgressPatch {
				last_page_read: Some(2),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, chapter_id).await, 0);

		insert_download(&state, chapter_id, "chapter.cbz").await;
		set_setting(&state, crate::download::DELETE_AFTER_READ, false).await;
		apply_patch(
			&state,
			chapter_id,
			ProgressPatch {
				last_page_read: Some(2),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, chapter_id).await, 1);
	}

	#[tokio::test]
	async fn read_deletion_uses_merged_bookmark_and_shared_artifact_rules() {
		let (state, manga_id, first, _tmp) = reader_db().await;
		let second = add_chapter(&state, manga_id, "second").await;
		set_bookmarked(&state, first, true).await;
		insert_download(&state, first, "shared.cbz").await;
		insert_download(&state, second, "shared.cbz").await;
		set_setting(&state, crate::download::DELETE_AFTER_MANUAL_READ, true).await;
		set_setting(&state, crate::download::ALLOW_DELETE_BOOKMARKED, false).await;
		apply_patch(
			&state,
			first,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, first).await, 1);

		apply_patch(
			&state,
			first,
			ProgressPatch {
				is_read: Some(true),
				is_bookmarked: Some(false),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, first).await, 0);
		assert!(state.config.downloads_dir.join("shared.cbz").exists());

		set_bookmarked(&state, second, true).await;
		apply_patch(
			&state,
			second,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, second).await, 1);
		assert!(state.downloads().delete(second, 1).await.expect("explicit delete"));
		assert!(!state.config.downloads_dir.join("shared.cbz").exists());

		insert_download(&state, second, "shared.cbz").await;
		set_setting(&state, crate::download::ALLOW_DELETE_BOOKMARKED, true).await;
		apply_patch(
			&state,
			second,
			ProgressPatch {
				is_read: Some(true),
				..ProgressPatch::default()
			},
		)
		.await;
		assert_eq!(download_count(&state, second).await, 0);
		assert!(!state.config.downloads_dir.join("shared.cbz").exists());
	}
}
