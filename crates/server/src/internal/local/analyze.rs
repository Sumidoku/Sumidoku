//! Post-scan background analysis (kmrs's analyze task chain): fills the index rows a scan leaves cheap.
//! Two independent passes, each gated by its per-instance setting (analyzeMetadata / analyzePages, config
//! layer as the default):
//! - metadata: manga with initialized = 0 run the full manga_update chain (ComicInfo → in-archive
//!   ComicInfo → series.json → EPUB OPF) plus chapter reconcile in trash mode and EPUB divina stamping;
//! - pages: chapters with page_count = 0 build their page rows through the reader's own load_pages, so the
//!   analysis write path is byte-identical to the on-open path (page rows + count + compatibility in one
//!   transaction).
//!
//! A failure on one item is logged and counted, never fails the pass; cancellation is checked between
//! items.
//!
//! "Needs analysis" is derivable from the rows themselves (initialized = 0 / page_count = 0 with no
//! pages_analyzed_at stamp), so a restart simply re-enqueues the kind and the pass resumes where the rows say
//! it stopped.

use crate::error::ApiResult;
use crate::state::AppState;
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn analyze_instance(run: &crate::task::TaskRun, source_id: i64) -> ApiResult<Value> {
	// a mangaId payload narrows the pass to one manga (the manual per-manga analysis endpoint)
	let manga_id: Option<i64> = serde_json::from_str::<Value>(&run.task.payload)?
		.get("mangaId")
		.and_then(Value::as_i64);
	let state = &run.state;
	let mut metadata_analyzed = 0;
	let mut pages_analyzed = 0;
	let mut failed = 0;

	if state.local_analyze_metadata(source_id).await? {
		let pending: Vec<(i64, String)> = match manga_id {
			Some(id) => sqlx::query_as(
				"SELECT id, key FROM manga WHERE id = ? AND initialized = 0 AND trashed_at IS NULL",
			)
			.bind(id)
			.fetch_all(&state.pool)
			.await?,
			None => sqlx::query_as(
				"SELECT id, key FROM manga WHERE source_id = ? AND initialized = 0 AND trashed_at IS NULL ORDER BY id",
			)
			.bind(source_id)
			.fetch_all(&state.pool)
			.await?,
		};
		for (manga_id, key) in pending {
			if run.is_cancelled() {
				break;
			}
			match analyze_metadata(state, source_id, manga_id, &key).await {
				Ok(()) => metadata_analyzed += 1,
				Err(e) => {
					tracing::warn!(manga_id, key, "local metadata analysis failed: {}", e.message());
					failed += 1;
				}
			}
		}
	}

	if state.local_analyze_pages(source_id).await? {
		let pending: Vec<i64> = match manga_id {
			Some(id) => sqlx::query_scalar(
				"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id
				 WHERE c.manga_id = ? AND c.page_count = 0 AND c.pages_analyzed_at IS NULL
				   AND c.trashed_at IS NULL AND m.trashed_at IS NULL
				 ORDER BY c.id",
			)
			.bind(id)
			.fetch_all(&state.pool)
			.await?,
			None => sqlx::query_scalar(
				"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id
				 WHERE m.source_id = ? AND c.page_count = 0 AND c.pages_analyzed_at IS NULL
				   AND c.trashed_at IS NULL AND m.trashed_at IS NULL
				 ORDER BY c.id",
			)
			.bind(source_id)
			.fetch_all(&state.pool)
			.await?,
		};
		for chapter_id in pending {
			if run.is_cancelled() {
				break;
			}
			// load_pages with refresh = false builds exactly when no cached rows exist — which page_count = 0
			// guarantees — so an already-analyzed chapter is never rebuilt here
			match crate::source::load_pages(state, chapter_id, false).await {
				Ok(_) => pages_analyzed += 1,
				Err(e) => {
					tracing::warn!(chapter_id, "local page analysis failed: {}", e.message());
					failed += 1;
				}
			}
			// a chapter still at zero pages (no images, an unreadable archive) is settled until a scan sees its
			// manga change; without the stamp every pass would reopen it
			crate::store::chapter::stamp_empty_analyzed(&state.pool, chapter_id, crate::model::unix_now()).await?;
		}
	}

	Ok(json!({
		"metadataAnalyzed": metadata_analyzed,
		"pagesAnalyzed": pages_analyzed,
		"failed": failed,
	}))
}

/// Enqueues a manga-scoped analysis at manual priority: the task id is per manga while the source group
/// keeps it serialized with the instance's scan/analysis work (the kind is shared; the payload narrows it)
pub async fn enqueue_manga_analysis(state: &AppState, manga_id: i64) -> ApiResult<crate::task::TaskRow> {
	let row = crate::source::manga_row(&state.pool, manga_id).await?;
	if !crate::source::source_is_local(&state.pool, row.source_id).await? {
		return Err(crate::error::ApiError::bad_request("not a local manga"));
	}
	state
		.tasks()
		.enqueue(
			&format!("{}:{}:{manga_id}", super::scan::KIND_ANALYZE, row.source_id),
			super::scan::KIND_ANALYZE,
			Some(row.source_id),
			json!({ "mangaId": manga_id }),
			crate::task::PRIORITY_MANUAL,
		)
		.await
}

/// One manga's metadata pass: the same manga_update + upsert + reconcile + divina stamp as the on-open
/// register path, committed in one transaction
async fn analyze_metadata(state: &Arc<AppState>, source_id: i64, manga_id: i64, key: &str) -> ApiResult<()> {
	let mut update = super::manga_update(
		&state.local_dirs(source_id).await?,
		&state.local_zip,
		key,
		&state.local_exclusions(source_id).await?,
	)
	.await?;
	let mut tx = crate::db::begin_write(&state.pool).await?;
	// the analysis upsert must not clobber scheduler-owned fields: preserve the row's update_strategy and
	// next_update_time, or an analyzed title previously marked Never would re-enter scheduled updates with
	// the default Manga's Always/NULL (and the upsert's next_update_is_fallback CASE keeps its fallback flag)
	let (strategy, next_at): (i64, Option<i64>) = sqlx::query_as(
		"SELECT update_strategy, next_update_time FROM manga WHERE id = ?",
	)
	.bind(manga_id)
	.fetch_one(&mut *tx)
	.await?;
	update.manga.update_strategy = crate::model::update_strategy_from_int(strategy);
	update.manga.next_update_time = next_at;
	crate::source::upsert_manga(&mut tx, source_id, &update.manga, true, true, true).await?;
	let chapters = update.manga.chapters.clone().unwrap_or_default();
	crate::source::reconcile_chapters_detailed(&mut tx, manga_id, &chapters, &update.protected, true).await?;
	super::store_epub_divina(&mut tx, manga_id, &chapters, &update.epub_divina).await?;
	super::store_chapter_metadata(&mut tx, manga_id, &update.chapter_meta).await?;
	tx.commit().await?;
	Ok(())
}
