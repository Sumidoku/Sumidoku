//! Scheduled full scan of a local instance (kmrs's scan_root_folder analogue): diffs the indexed manga
//! against the filesystem. A vanished, unprotected manga/chapter is trashed (soft delete) instead of
//! deleted; reappearance clears the flag in place. Indexing is scan-level cheap: title from the leaf
//! directory name and the chapter list from enumeration, initialized = 0 — archive-opening metadata and
//! page analysis belong to the post-scan analysis pass (`analyze.rs`) and to the on-open path.
//!
//! Safety invariants, pinned by tests:
//! - failed-directory protection: anything under an unreadable path is excluded from the trash diff;
//! - empty-scan guard: zero hits with any failed directory trashes nothing (we do not know the state);
//! - empty-root guard: a readable root with no visible entries (an unmounted mount point reads as an
//!   empty directory) and live rows underneath fails the scan as root_unavailable — a zero-key,
//!   zero-failure listing from such a root must never reach the trash diff;
//! - a mid-scan IO error fails the whole scan (the module contract: reconciling against half a listing
//!   would mistake unscanned chapters for deleted ones).

use crate::server_state::{self, LOCAL_LAST_SCAN_AT};
use crate::error::ApiResult;
use crate::model::unix_now;
use crate::source::reconcile_chapters_detailed;
use crate::state::AppState;
use crate::task::{PRIORITY_DERIVED, PRIORITY_MANUAL, PRIORITY_SCHEDULED};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const KIND_SCAN: &str = "local_scan";
pub const KIND_ANALYZE: &str = "local_analyze";
pub const KIND_EMPTY_TRASH: &str = "empty_trash";

static SCAN_REQUEST_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Each trigger needs a distinct identity even when its visible settings are unchanged, so a newer request
/// cannot be mistaken for a duplicate during the terminal write.
fn next_scan_request_id() -> String {
	let nanos = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos();
	format!("{nanos}-{}", SCAN_REQUEST_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOutcome {
	pub scanned: i64,
	pub indexed: i64,
	pub unchanged: i64,
	pub reconciled: i64,
	pub trashed: i64,
	pub restored: i64,
	pub protected: i64,
	pub chapters_trashed: i64,
	pub chapters_restored: i64,
}

#[derive(sqlx::FromRow)]
struct PersistedManga {
	id: i64,
	key: String,
	trashed_at: Option<i64>,
	scan_mtime: Option<i64>,
	local_entry_kind: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RestoreManga {
	source_id: i64,
	key: String,
	trashed_at: Option<i64>,
	local_entry_kind: Option<String>,
}

/// kmrs's is_protected_url: an entry protects itself and everything below it; keyed forms already carry
/// their root prefix, and a sibling like seriesAB must not match seriesA
/// Reads one discovered entry's chapters for the upsert pass. Discovery and this read are separate passes, so
/// an entry can vanish or turn unreadable in between (a library being reorganised): that entry is skipped with
/// a warning instead of failing the whole scan, and its row already stayed out of the trash pass by being listed
async fn scan_listed_entry(
	source_id: i64,
	roots: &[std::path::PathBuf],
	key: &str,
	exclusions: &[String],
) -> Option<super::ChapterScan> {
	let scanned = match super::resolve_key(roots, key) {
		Ok((dir, rel)) => super::scan_chapters(dir, rel, exclusions).await,
		Err(e) => Err(e),
	};
	scanned
		.inspect_err(|e| tracing::warn!(source_id, key, "local scan skipped an entry it could not read: {e:?}"))
		.ok()
}

/// A reconcile means the manga's subtree changed on disk: a chapter analysed to zero pages (an empty
/// directory, a broken archive) may hold pages now, so it becomes eligible for the analysis pass again
async fn clear_zero_page_stamps(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, manga_id: i64) -> ApiResult<()> {
	crate::store::chapter::clear_manga_analyzed(tx, manga_id).await?;
	Ok(())
}

fn is_protected(key: &str, failed: &[String]) -> bool {
	failed
		.iter()
		.any(|entry| key == entry || key.starts_with(&format!("{entry}/")))
}

/// Whether the key's leaf name carries an archive extension: the shape test for a oneshot entry. Only the
/// filesystem shape decides a row's fate during reconcile — a directory named "Series.cbz" is a directory
fn is_archive_name(key: &str) -> bool {
	key.rsplit('/')
		.next()
		.unwrap_or(key)
		.rsplit('.')
		.next()
		.is_some_and(|ext| ["cbz", "zip", "epub"].iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// `Some(true)` when a root holds at least one entry the scan would count (not dot-prefixed, not
/// `@eaDir` — the skip rule `read_items` applies), `Some(false)` when it holds none, `None` when it
/// cannot be listed. A mount point with nothing mounted on it is a readable but empty directory:
/// scanning it finds zero keys and zero failures, which the trash diff would read as "every row
/// deleted". Hidden entries do not count — the scan skips them, and file browsers leave them behind
/// (`.DS_Store`).
async fn root_has_visible_entry(root: &std::path::Path) -> Option<bool> {
	let mut entries = tokio::fs::read_dir(root).await.ok()?;
	loop {
		match entries.next_entry().await {
			Ok(Some(entry)) => {
				let name = entry.file_name();
				let name = name.to_string_lossy();
				if !name.starts_with('.') && name != "@eaDir" {
					return Some(true);
				}
			}
			Ok(None) => return Some(false),
			Err(_) => return None,
		}
	}
}

/// Full scan of one local instance. Runs against the live filesystem; every manga's writes commit in their
/// own transaction (the migrate pattern), so a later failure never rolls back earlier, already-correct work.
/// `refresh_metadata` gates the metadata invalidation (changed rows are marked initialized = 0 so the
/// analysis pass re-reads embedded metadata): enumeration/reconcile always happens, metadata re-reads only
/// with the user's opt-in. `cancelled` (the task's flag) stops the per-manga loops between entries; work
/// already committed stands and the next scan continues from it. A settings write does not converge a
/// running scan on mixed inputs: it cancels the task and requeues a fresh one (see the settings endpoint)
pub async fn scan_instance(
	state: &Arc<AppState>,
	source_id: i64,
	refresh_metadata: bool,
	cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> ApiResult<ScanOutcome> {
	let is_cancelled = || cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
	let roots = state.local_dirs(source_id).await?;
	let oneshots = state.local_oneshots_dir(source_id).await?;
	let exclusions = state.local_exclusions(source_id).await?;
	let force = state.local_force_modified_time(source_id).await?;
	let persisted: Vec<PersistedManga> = sqlx::query_as(
		"SELECT id, key, trashed_at, scan_mtime, local_entry_kind FROM manga WHERE source_id = ?",
	)
	.bind(source_id)
	.fetch_all(&state.pool)
	.await?;
	// kmrs's unavailable library: a missing or unreadable root means we cannot know the state of anything
	// under it, so the scan fails outright instead of trashing on a partial view (browse treats a missing
	// root as an empty listing; the lifecycle scan must not). The same holds for a readable root with no
	// visible entries — the shape of a mount point whose disk is not mounted (kmrs's "no visible
	// entries"): its listing is zero keys and zero failures, which no downstream guard distinguishes from
	// "the user deleted everything". Only a root with live rows at risk refuses; an empty root with
	// nothing to trash is just an empty library and scans normally.
	for (index, root) in roots.iter().enumerate() {
		match tokio::fs::metadata(root).await {
			Ok(metadata) if metadata.is_dir() => {}
			_ => {
				return Err(crate::error::ApiError::conflict(
					"root_unavailable",
					format!("local root is not a readable directory: {}", root.display()),
				));
			}
		}
		if root_has_visible_entry(root).await == Some(false)
			&& persisted
				.iter()
				.any(|row| row.trashed_at.is_none() && super::root_index_of(&row.key) == index)
		{
			return Err(crate::error::ApiError::conflict(
				"root_unavailable",
				format!("local root has no visible entries (unmounted?): {}", root.display()),
			));
		}
	}
	// Index every discovered directory, shadowed copies included (N:rel keys too): the multi-root dedup is
	// a read-model rule (local_search reapplies it), not a scan input rule — a shadowed copy must be
	// scannable, analyzable and restorable on its own
	let mut keys = Vec::new();
	let mut failed = Vec::new();
	for (index, dir) in roots.iter().enumerate() {
		let (root_keys, root_failed) = super::scan_one_dir(dir, oneshots.as_deref(), &exclusions, true).await?;
		keys.extend(root_keys.into_iter().map(|rel| match index {
			0 => rel,
			_ => format!("{index}:{rel}"),
		}));
		failed.extend(root_failed.into_iter().map(|rel| match index {
			0 => rel,
			_ => format!("{index}:{rel}"),
		}));
	}
	let mut outcome = ScanOutcome {
		scanned: keys.len() as i64,
		..ScanOutcome::default()
	};
	let now = unix_now();

	let scanned: HashSet<&str> = keys.iter().map(String::as_str).collect();
	if !(keys.is_empty() && !failed.is_empty()) {
		for row in &persisted {
			if is_cancelled() {
				break;
			}
			if scanned.contains(row.key.as_str()) {
				continue;
			}
			// an excluded subtree is out of the scan's sight, not gone: its rows are protected the same way
			let lowered = row.key.to_lowercase();
			if exclusions.iter().any(|exclusion| lowered.contains(exclusion)) {
				outcome.protected += 1;
				continue;
			}
			if is_protected(&row.key, &failed) {
				outcome.protected += 1;
				continue;
			}
			// only a genuinely absent path trashes. An existing but wrong-typed entry (a regular file in the
			// manga dir's place) or an unreadable one is protected — we cannot know its state. A correctly
			// typed entry the current rules no longer claim (reclassified — e.g. the oneshots rule started
			// matching, or its chapters moved into a nested layout) trashes: the row is not failed, it is
			// no longer a manga
			let protected_entry = match super::resolve_key(&roots, &row.key) {
				Ok((dir, rel)) => match tokio::fs::metadata(dir.join(rel)).await {
					Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
					Err(_) => true,
					Ok(metadata) => {
						// the actual entry shape decides, not the key's filename suffix (a directory named
						// "Series.cbz" is still a directory): a readable directory the rules no longer claim
						// is reclassified and trashes; a regular (non-archive) file in a manga's place is
						// wrong-typed — we cannot know its state, so it is protected. An archive file the
						// oneshots rule no longer claims is reclassified too and trashes
						!metadata.is_dir() && !is_archive_name(&row.key)
					}
				},
				Err(_) => true,
			};
			if protected_entry {
				outcome.protected += 1;
				continue;
			}
			if row.trashed_at.is_some() {
				continue;
			}
			// trashing a manga trashes its chapters with it (kmrs's soft_delete_many cascade)
			let mut tx = crate::db::begin_write(&state.pool).await?;
			crate::store::manga::trash(&mut tx, row.id, now).await?;
			crate::store::chapter::trash_manga(&mut tx, row.id, now).await?;
			tx.commit().await?;
			outcome.trashed += 1;
		}
	}

	let known: HashMap<&str, &PersistedManga> = persisted.iter().map(|row| (row.key.as_str(), row)).collect();
	for key in &keys {
		if is_cancelled() {
			break;
		}
		let Some(scan) = scan_listed_entry(source_id, &roots, key, &exclusions).await else {
			outcome.protected += 1;
			continue;
		};
		let mtime = if force { scan.all_mtime_ms } else { scan.dir_mtime_ms };
		let mut tx = crate::db::begin_write(&state.pool).await?;
		match upsert_entry(&mut tx, source_id, key, known.get(key.as_str()).copied(), &scan, mtime, refresh_metadata, false, None, true).await? {
			EntryUpsert::Unchanged { .. } => {
				tx.commit().await?;
				outcome.unchanged += 1;
			}
			// a trashed row returns only under its recorded shape (manual restore answers 409 gone
			// otherwise): an entry of another kind now at the key is a different thing. An unknown kind
			// is learned here instead
			EntryUpsert::ShapeMismatch | EntryUpsert::KindUnknown => {
				outcome.protected += 1;
			}
			EntryUpsert::Done { is_new, was_trashed, reconciled, .. } => {
				tx.commit().await?;
				if is_new {
					outcome.indexed += 1;
				} else if was_trashed {
					outcome.restored += 1;
				} else {
					outcome.reconciled += 1;
				}
				outcome.chapters_trashed += reconciled.trashed;
				outcome.chapters_restored += reconciled.restored;
			}
		}
	}
	Ok(outcome)
}

/// What the entry upsert decided; the two scan paths map it differently — the full scan counts and
/// protects, the entry scan answers 409 for the blocked shapes and reports the reconcile outcome
enum EntryUpsert {
	/// The unchanged fast path: same mtime, not trashed, kind matches (a missing kind was backfilled)
	Unchanged { manga_id: i64 },
	/// The entry was inserted or reconciled and its chapters are in sync
	Done {
		manga_id: i64,
		is_new: bool,
		was_trashed: bool,
		reconciled: crate::source::ReconcileOutcome,
	},
	/// A trashed row whose recorded kind disagrees with the entry now at the key: the full scan
	/// protects the row; the entry scan refuses to restore over a different entry (409 gone)
	ShapeMismatch,
	/// A trashed row without a recorded kind: full scans learn the kind by reconciling; entry scans
	/// refuse (restore needs the shape a full scan records first)
	KindUnknown,
}

/// The one upsert branch shared by the full scan (`scan_instance`) and the entry scan
/// (`scan_manga_entry`): identity guard, trashed-shape guard, unchanged fast path, then mark/insert +
/// chapter reconcile + the latest-updates stamp. `existing` is the caller's view of the persisted row —
/// the full scan's snapshot from scan start, the entry scan's fresh read inside this transaction.
/// `force_reconcile` bypasses the fast path and forbids a fresh insert (restore keeps the original
/// row), `expected_manga_id` pins the row restore is allowed to claim, and `learn_unknown_kind` lets
/// the full scan adopt a kind for rows recorded before the column existed where the entry scan
/// rejects them
#[allow(clippy::too_many_arguments)]
async fn upsert_entry(
	tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
	source_id: i64,
	key: &str,
	existing: Option<&PersistedManga>,
	scan: &super::ChapterScan,
	mtime: i64,
	refresh_metadata: bool,
	force_reconcile: bool,
	expected_manga_id: Option<i64>,
	learn_unknown_kind: bool,
) -> ApiResult<EntryUpsert> {
	// Restore must keep the original row, not a replacement with the same source and key.
	if let Some(row) = existing
		&& let Some(expected) = expected_manga_id
		&& (row.id != expected || row.trashed_at.is_none())
	{
		return Err(crate::error::ApiError::conflict("not_trashed", "the original manga row was purged or already restored"));
	}
	// A trashed row returns only under its recorded shape: an entry of another kind now at the key is
	// a different thing. An unknown kind is learned by full scans; entry scans reject it.
	if let Some(row) = existing
		&& row.trashed_at.is_some()
	{
		match row.local_entry_kind.as_deref() {
			Some(kind) if kind == scan.entry_kind => {}
			Some(_) => return Ok(EntryUpsert::ShapeMismatch),
			None if learn_unknown_kind => {}
			None => return Ok(EntryUpsert::KindUnknown),
		}
	}
	// the fast path: an untouched, non-trashed subtree skips reconciliation entirely. Trashed rows are
	// exempt — reappearance must always restore — and a NULL stored mtime (pre-index row) always
	// reconciles once so the row picks up its stamp
	if let Some(row) = existing
		&& row.trashed_at.is_none()
		&& row.scan_mtime == Some(mtime)
		&& row.local_entry_kind.as_deref().is_none_or(|kind| kind == scan.entry_kind)
		&& !force_reconcile
	{
		if row.local_entry_kind.is_none() {
			crate::store::manga::backfill_entry_kind(&mut *tx, row.id, scan.entry_kind).await?;
		}
		return Ok(EntryUpsert::Unchanged { manga_id: row.id });
	}
	let chapters: Vec<aidoku::Chapter> = scan.chapters.iter().map(super::to_aidoku_chapter).collect();
	let now = crate::model::unix_now();
	let is_existing = existing.is_some();
	let was_trashed = existing.is_some_and(|row| row.trashed_at.is_some());
	let manga_id = match existing {
		Some(row) => {
			// A changed subtree (or a restored row) reconciles; marking initialized = 0 (kmrs's
			// OUTDATED, the analysis pass re-reads embedded metadata) happens only with the caller's
			// metadata opt-in — enumeration is not metadata refresh
			crate::store::manga::mark_scanned(&mut *tx, row.id, mtime, scan.entry_kind, refresh_metadata).await?;
			row.id
		}
		None => {
			// Restore cannot recreate a purged row with a fresh id and lose the original user state.
			if force_reconcile {
				return Err(crate::error::ApiError::conflict("not_trashed", "manga was purged before restore"));
			}
			// index-level insert: leaf-name title, no metadata (initialized stays 0 for the analysis
			// pass / on-open path). updated_at stays NULL so the manga appears in latest-added but not
			// latest-updates until a content change sets it.
			crate::store::manga::insert_local(&mut *tx, source_id, key, super::title_of(key), mtime, scan.entry_kind, now).await?
		}
	};
	let reconciled = reconcile_chapters_detailed(&mut *tx, manga_id, &chapters, &scan.failed, true).await?;
	clear_zero_page_stamps(&mut *tx, manga_id).await?;
	// Only existing manga count as "updated"; a freshly inserted manga's chapters are part of its
	// initial indexing and should keep updated_at NULL so it appears in latest-added only.
	let content_changed = is_existing
		&& (!reconciled.new_ids.is_empty() || reconciled.trashed > 0 || reconciled.restored > 0);
	if content_changed {
		crate::store::manga::set_updated_at(&mut *tx, manga_id, now).await?;
	}
	Ok(EntryUpsert::Done { manga_id, is_new: !is_existing, was_trashed, reconciled })
}

/// Entry-level scan of one manga (manual refresh / open / library update): directory enumeration only,
/// never opens an archive — that work belongs to the analysis pass. Returns the manga id, the chapter
/// reconcile outcome, and whether anything changed. An unchanged subtree (same mtime, not trashed) skips
/// chapter reconciliation because only a missing original kind might need a write; a changed or reappearing
/// one reconciles and, with `refresh_metadata`, marks the manga
/// initialized = 0 so the analysis pass re-reads its embedded metadata (kmrs's OUTDATED). `force_reconcile`
/// bypasses the unchanged fast path: restore uses it to validate the original shape and untrash in one write
/// transaction, even when the mtime did not move
pub async fn scan_manga_entry(
	state: &AppState,
	source_id: i64,
	manga_key: &str,
	refresh_metadata: bool,
	force_reconcile: bool,
	expected_manga_id: Option<i64>,
) -> ApiResult<(i64, crate::source::ReconcileOutcome, bool)> {
	let roots = state.local_dirs(source_id).await?;
	let exclusions = state.local_exclusions(source_id).await?;
	let force = state.local_force_modified_time(source_id).await?;
	let (dir, rel) = super::resolve_key(&roots, manga_key)?;
	let scan = super::scan_chapters(dir, rel, &exclusions).await?;
	let mtime = if force { scan.all_mtime_ms } else { scan.dir_mtime_ms };
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let existing: Option<PersistedManga> = sqlx::query_as(
		"SELECT id, key, trashed_at, scan_mtime, local_entry_kind FROM manga WHERE source_id = ? AND key = ?",
	)
	.bind(source_id)
	.bind(manga_key)
	.fetch_optional(&mut *tx)
	.await?;
	match upsert_entry(&mut tx, source_id, manga_key, existing.as_ref(), &scan, mtime, refresh_metadata, force_reconcile, expected_manga_id, false).await? {
		EntryUpsert::Unchanged { manga_id } => {
			tx.commit().await?;
			Ok((manga_id, crate::source::ReconcileOutcome {
				new_ids: Vec::new(),
				previous_latest_chapter_number: None,
				trashed: 0,
				restored: 0,
			}, false))
		}
		// Refresh may reclassify a live row, but a trashed row can return only under its recorded shape.
		EntryUpsert::ShapeMismatch | EntryUpsert::KindUnknown => Err(crate::error::ApiError::conflict(
			"gone",
			"the on-disk entry no longer matches the recorded manga shape",
		)),
		EntryUpsert::Done { manga_id, reconciled, .. } => {
			tx.commit().await?;
			// The analysis pass owns archive-opening work; opening or refreshing an unanalyzed manga
			// kicks it at manual priority (the per-instance dedup id makes repeats free); harnesses
			// without the manager skip
			if let Some(tasks) = state.maybe_tasks()
				&& let Err(e) = tasks
					.enqueue(
						&format!("{KIND_ANALYZE}:{source_id}"),
						KIND_ANALYZE,
						Some(source_id),
						json!({}),
						PRIORITY_MANUAL,
					)
					.await
			{
				tracing::warn!(source_id, "enqueue analysis failed: {}", e.message());
			}
			Ok((manga_id, reconciled, true))
		}
	}
}

/// Registers the three local task kinds on the manager (called from build_app)
pub async fn register_handlers(manager: &crate::task::TaskManager) {
	manager
		.register(KIND_SCAN, Arc::new(|run| {
			Box::pin(async move {
				let source_id = run
					.task
					.source_id
					.ok_or_else(|| crate::error::ApiError::bad_request("local_scan needs a source id"))?;
				let refresh_metadata = serde_json::from_str::<Value>(&run.task.payload)?
					.get("refreshMetadata")
					.and_then(Value::as_bool)
					.unwrap_or(false);
				if run.is_cancelled() {
					return Ok(serde_json::to_value(ScanOutcome::default())?);
				}
				// One pass per execution, no mid-scan convergence: a settings write cancels the live task
				// and requeues a fresh one (the settings endpoint cancels before enqueueing), and a newer
				// manual trigger moves the payload, which the terminal-write supersession requeues on
				let outcome = scan_instance(&run.state, source_id, refresh_metadata, Some(&run.cancel_flag())).await?;
				// A cancelled scan leaves no side effects behind it: no derived analysis either
				if !run.is_cancelled() {
					run.state
						.tasks()
						.enqueue(
							&format!("{KIND_ANALYZE}:{source_id}"),
							KIND_ANALYZE,
							Some(source_id),
							json!({}),
							PRIORITY_DERIVED,
						)
						.await?;
				}
				Ok(serde_json::to_value(outcome)?)
			})
		}))
		.await;
	manager
		.register(KIND_ANALYZE, Arc::new(|run| {
			Box::pin(async move {
				let source_id = run
					.task
					.source_id
					.ok_or_else(|| crate::error::ApiError::bad_request("local_analyze needs a source id"))?;
				super::analyze::analyze_instance(&run, source_id).await
			})
		}))
		.await;
	manager
		.register(KIND_EMPTY_TRASH, Arc::new(|run| {
			Box::pin(async move {
				let source_id = run
					.task
					.source_id
					.ok_or_else(|| crate::error::ApiError::bad_request("empty_trash needs a source id"))?;
				empty_trash(&run, source_id).await
			})
		}))
		.await;
}

/// Cancels the instance's live scan task, if any: a settings write supersedes a running scan wholesale —
/// converging it mid-flight would commit rows reconciled under the old inputs. The caller re-enqueues
/// afterwards, so a missing or already-terminal task is the normal no-op, not an error
pub async fn cancel_instance_scan(state: &AppState, source_id: i64) {
	match state.tasks().cancel(&format!("{KIND_SCAN}:{source_id}")).await {
		Ok(_) => tracing::info!(source_id, "cancelled the live local scan; a fresh one is requeued"),
		Err(e) if e.code() == "not_found" || e.code() == "task_finished" => {}
		Err(e) => tracing::warn!(source_id, "cancel local scan failed: {}", e.message()),
	}
}

/// Enqueues a scan per local instance (or just the given one); the per-instance dedup id makes a repeat
/// trigger idempotent — kmrs's "a scheduled scan never stacks up". `refresh_metadata` and a unique request id
/// ride the payload: the terminal write can tell a genuinely newer trigger apart from identical settings.
pub async fn enqueue_scan(
	state: &AppState,
	source_id: Option<i64>,
	priority: i64,
	refresh_metadata: bool,
) -> ApiResult<Vec<crate::task::TaskRow>> {
	let ids: Vec<i64> = match source_id {
		Some(id) => vec![id],
		None => local_instance_ids(&state.pool).await?,
	};
	let mut tasks = Vec::with_capacity(ids.len());
	for id in ids {
		tasks.push(
			state
				.tasks()
				.enqueue(
					&format!("{KIND_SCAN}:{id}"),
					KIND_SCAN,
					Some(id),
					json!({ "refreshMetadata": refresh_metadata, "requestId": next_scan_request_id() }),
					priority,
				)
				.await?,
		);
	}
	Ok(tasks)
}

async fn local_instance_ids(pool: &SqlitePool) -> ApiResult<Vec<i64>> {
	let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM source WHERE kind = 'local' AND enabled = 1 ORDER BY id")
		.fetch_all(pool)
		.await?;
	Ok(ids)
}

const SCHEDULE_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// The scan scheduler (BackupScheduler's shape): a minute tick re-reads the config interval, and due means
/// now - last scan >= interval. The interval is deployment-level config (local_scan_secs), not a setting —
/// 0 keeps the loop inert
pub async fn run_scheduler(state: Arc<AppState>) {
	let mut ticker = tokio::time::interval(SCHEDULE_TICK);
	// interval's first tick completes immediately; the first check waits a full tick
	ticker.tick().await;
	loop {
		ticker.tick().await;
		if let Err(e) = run_scheduled_if_due(&state).await {
			tracing::warn!("scheduled local scan failed: {}", e.message());
		}
	}
}

async fn run_scheduled_if_due(state: &Arc<AppState>) -> ApiResult<bool> {
	let interval = state.config.local_scan_secs;
	if interval == 0 {
		return Ok(false);
	}
	let now = unix_now();
	let last = server_state::time(&state.pool, LOCAL_LAST_SCAN_AT).await?;
	let due = last.is_none_or(|at| now.saturating_sub(at) >= i64::try_from(interval).unwrap_or(i64::MAX));
	if !due {
		return Ok(false);
	}
	// Stamp before enqueueing: a crash between them would otherwise double-run; at worst the stamp stands
	// and the next tick is one interval later
	server_state::set_time(&state.pool, LOCAL_LAST_SCAN_AT, now).await?;
	let settings = crate::update::LibraryUpdateSettings::load(&state.pool, 0).await?;
	enqueue_scan(state, None, PRIORITY_SCHEDULED, settings.refresh_metadata).await?;
	tracing::info!("scheduled local scan enqueued");
	Ok(true)
}

/// Boot hook for local_scan_on_startup
pub async fn enqueue_startup_scan(state: &AppState) -> ApiResult<()> {
	if state.config.local_scan_on_startup {
		let settings = crate::update::LibraryUpdateSettings::load(&state.pool, 0).await?;
		enqueue_scan(state, None, PRIORITY_SCHEDULED, settings.refresh_metadata).await?;
	}
	Ok(())
}

/// Trashed manga of one instance (or every local instance), oldest trashing first
pub async fn list_trash(state: &AppState, source_id: Option<i64>) -> ApiResult<Vec<Value>> {
	let rows: Vec<(i64, i64, String, String, i64)> = match source_id {
		Some(id) => {
			sqlx::query_as(
				"SELECT m.id, m.source_id, m.key, m.title, m.trashed_at FROM manga m
				 JOIN source s ON s.id = m.source_id
				 WHERE s.kind = 'local' AND m.trashed_at IS NOT NULL AND m.source_id = ?
				 ORDER BY m.trashed_at, m.id",
			)
			.bind(id)
			.fetch_all(&state.pool)
			.await?
		}
		None => {
			sqlx::query_as(
				"SELECT m.id, m.source_id, m.key, m.title, m.trashed_at FROM manga m
				 JOIN source s ON s.id = m.source_id
				 WHERE s.kind = 'local' AND m.trashed_at IS NOT NULL
				 ORDER BY m.trashed_at, m.id",
			)
			.fetch_all(&state.pool)
			.await?
		}
	};
	let mut out = Vec::with_capacity(rows.len());
	for (manga_id, source, key, title, trashed_at) in rows {
		let chapters: i64 =
			sqlx::query_scalar("SELECT COUNT(*) FROM chapter WHERE manga_id = ? AND trashed_at IS NOT NULL")
				.bind(manga_id)
				.fetch_one(&state.pool)
				.await?;
		out.push(json!({
			"manga": {"id": manga_id, "sourceId": source, "key": key, "title": title},
			"trashedAt": trashed_at,
			"trashedChapters": chapters,
		}));
	}
	Ok(out)
}

/// Restores a trashed manga by reconciling it against disk (not by blanket-clearing flags): only entries
/// actually back on disk untrash — a manga whose ch1 returned while ch2 is still missing keeps ch2 trashed.
/// The entry must match the original kind recorded by a scan because neither existence nor a filename suffix
/// can distinguish a replacement. Unknown history cannot prove that match and must be learned by a scan first.
/// The untrash itself is a conditional claim on the trashed row, so a purge racing the
/// restore cannot delete the row in between and leave restore reporting success on a row whose progress and
/// library links have already cascaded away — and scan_manga_entry updates the existing row instead of
/// re-inserting a new id
pub async fn restore_manga(state: &Arc<AppState>, manga_id: i64) -> ApiResult<()> {
	let row: Option<RestoreManga> = sqlx::query_as(
		"SELECT source_id, key, trashed_at, local_entry_kind FROM manga WHERE id = ?",
	)
	.bind(manga_id)
	.fetch_optional(&state.pool)
	.await?;
	let row = row.ok_or_else(|| crate::error::ApiError::not_found("manga not found"))?;
	if !crate::source::source_is_local(&state.pool, row.source_id).await? {
		return Err(crate::error::ApiError::bad_request("not a local manga"));
	}
	if row.trashed_at.is_none() {
		return Err(crate::error::ApiError::conflict("not_trashed", "manga is not trashed"));
	}
	let roots = state.local_dirs(row.source_id).await?;
	let (dir, rel) = super::resolve_key(&roots, &row.key)?;
	match tokio::fs::metadata(dir.join(rel)).await {
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			return Err(crate::error::ApiError::conflict(
				"gone",
				"the entry is still missing on disk",
			));
		}
		Err(_) => {
			return Err(crate::error::ApiError::conflict(
				"gone",
				"the entry is not readable on disk",
			));
		}
		Ok(metadata) => {
			let shape_ok = match row.local_entry_kind.as_deref() {
				Some("directory") => metadata.is_dir(),
				Some("archive") => metadata.is_file(),
				_ => false,
			};
			if !shape_ok {
				let message = if row.local_entry_kind.is_none() {
					"the original entry kind is unknown; run a local scan before restoring"
				} else {
					"the on-disk entry no longer matches its original kind"
				};
				return Err(crate::error::ApiError::conflict("gone", message));
			}
		}
	}
	// The row stays trashed until the entry scan validates its shape and commits, so a failed rescan leaves
	// the original row and all user state untouched instead of restoring a different entry kind.
	scan_manga_entry(state, row.source_id, &row.key, true, true, Some(manga_id)).await?;
	Ok(())
}

/// Hard delete of one trashed manga: the row cascade removes chapters, progress, category memberships and
/// page rows (the same cascade as source deletion). The derived cover cache goes with it, or an id reused
/// after a rescan could show the previous manga's cover
pub async fn purge_manga(state: &AppState, manga_id: i64) -> ApiResult<()> {
	let mut write = crate::db::begin_write(&state.pool).await?;
	let deleted = crate::store::manga::delete_trashed(&mut write, manga_id).await?;
	write.commit().await?;
	if !deleted {
		return Err(crate::error::ApiError::conflict(
			"not_trashed",
			"manga is not trashed (or does not exist)",
		));
	}
	crate::source::cover::invalidate_cover(&state.config.covers_dir, manga_id).await;
	Ok(())
}

/// The empty_trash task body: purges every trashed manga of the instance, checking cancellation between items
async fn empty_trash(run: &crate::task::TaskRun, source_id: i64) -> ApiResult<Value> {
	let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM manga WHERE source_id = ? AND trashed_at IS NOT NULL")
		.bind(source_id)
		.fetch_all(&run.state.pool)
		.await?;
	let total = ids.len() as i64;
	let mut purged = 0;
	for id in ids {
		if run.is_cancelled() {
			break;
		}
		// Guarded per row: a manga restored after the ids snapshot must not be deleted again
		let mut write = crate::db::begin_write(&run.state.pool).await?;
		let deleted = crate::store::manga::delete_trashed(&mut write, id).await?;
		write.commit().await?;
		if deleted {
			crate::source::cover::invalidate_cover(&run.state.config.covers_dir, id).await;
			purged += 1;
		}
	}
	Ok(json!({ "purged": purged, "total": total }))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::Config;
	use tokio::sync::mpsc;

	#[tokio::test]
	async fn an_entry_gone_after_discovery_is_skipped_not_fatal() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let root = tmp.path().join("local");
		std::fs::create_dir_all(root.join("Alpha/ch 1")).expect("chapter directory");
		std::fs::write(root.join("Alpha/ch 1/1.png"), b"page").expect("page");
		let roots = vec![root];
		assert!(scan_listed_entry(1, &roots, "Alpha", &[]).await.is_some());
		// listed by discovery, then removed before the upsert pass reads it
		std::fs::remove_dir_all(roots[0].join("Alpha")).expect("remove entry");
		assert!(scan_listed_entry(1, &roots, "Alpha", &[]).await.is_none());
		// a key past the configured roots cannot resolve either
		assert!(scan_listed_entry(1, &roots, "3:Alpha", &[]).await.is_none());
	}

	async fn local_state() -> (Arc<AppState>, i64, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let root = tmp.path().join("local");
		std::fs::create_dir_all(root.join("Alpha.cbz/ch 1")).expect("chapter directory");
		std::fs::write(root.join("Alpha.cbz/ch 1/1.png"), b"page").expect("page");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let source_id: i64 = sqlx::query_scalar("SELECT id FROM source WHERE key = 'local.local'")
			.fetch_one(&pool)
			.await
			.expect("local source");
		let config = Config {
			config_dir: tmp.path().to_path_buf(),
			covers_dir: tmp.path().join("covers"),
			downloads_dir: tmp.path().join("downloads"),
			local_dirs: Some(vec![root]),
			..Config::default()
		};
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		(AppState::new(pool, config, defaults_tx, cookie_tx), source_id, tmp)
	}

	#[tokio::test]
	async fn restore_does_not_claim_a_replacement_row_with_the_same_key() {
		let (state, source_id, _tmp) = local_state().await;
		let original_id: i64 = sqlx::query_scalar(
			"INSERT INTO manga (source_id, key, title, trashed_at, local_entry_kind) VALUES (?, 'Alpha.cbz', 'Alpha', 1, 'directory') RETURNING id",
		)
		.bind(source_id)
		.fetch_one(&state.pool)
		.await
		.expect("original manga");
		sqlx::query("DELETE FROM manga WHERE id = ?")
			.bind(original_id)
			.execute(&state.pool)
			.await
			.expect("purge original");
		let replacement_id: i64 = sqlx::query_scalar(
			"INSERT INTO manga (id, source_id, key, title, trashed_at, local_entry_kind) VALUES (?, ?, 'Alpha.cbz', 'Replacement', 1, 'directory') RETURNING id",
		)
		.bind(original_id + 1)
		.bind(source_id)
		.fetch_one(&state.pool)
		.await
		.expect("replacement manga");
		assert_ne!(replacement_id, original_id, "the fixture must exercise a different row identity");

		let error = scan_manga_entry(&state, source_id, "Alpha.cbz", true, true, Some(original_id))
			.await
			.expect_err("the original id must not restore a replacement row");
		assert_eq!(error.code(), "not_trashed");
		let (id, title, trashed_at): (i64, String, Option<i64>) =
			sqlx::query_as("SELECT id, title, trashed_at FROM manga WHERE source_id = ? AND key = 'Alpha.cbz'")
				.bind(source_id)
				.fetch_one(&state.pool)
				.await
				.expect("replacement row");
		assert_eq!(id, replacement_id);
		assert_eq!(title, "Replacement");
		assert_eq!(trashed_at, Some(1));
	}

	async fn insert_live_manga(state: &AppState, source_id: i64, key: &str) -> i64 {
		sqlx::query_scalar(
			"INSERT INTO manga (source_id, key, title, local_entry_kind) VALUES (?, ?, ?, 'directory') RETURNING id",
		)
		.bind(source_id)
		.bind(key)
		.bind(crate::internal::local::title_of(key))
		.fetch_one(&state.pool)
		.await
		.expect("live manga row")
	}

	async fn trashed_at(state: &AppState, manga_id: i64) -> Option<i64> {
		sqlx::query_scalar("SELECT trashed_at FROM manga WHERE id = ?")
			.bind(manga_id)
			.fetch_one(&state.pool)
			.await
			.expect("manga row")
	}

	/// kmrs #365's case: the root reads fine but shows no entries — a mount point with nothing mounted.
	/// The scan must fail as root_unavailable, not trash every live row (#498)
	#[tokio::test]
	async fn an_empty_root_with_live_rows_fails_the_scan() {
		let (state, source_id, tmp) = local_state().await;
		let root = tmp.path().join("local");
		let manga_id = insert_live_manga(&state, source_id, "Alpha.cbz").await;
		std::fs::remove_dir_all(root.join("Alpha.cbz")).expect("unmount the disk");

		let error = scan_instance(&state, source_id, false, None)
			.await
			.expect_err("an empty root with live rows must fail the scan");
		assert_eq!(error.code(), "root_unavailable");
		assert_eq!(trashed_at(&state, manga_id).await, None, "nothing may be trashed");
	}

	/// A root with nothing to trash is just an empty library: the scan runs and indexes nothing
	/// (kmrs's "a new library on an empty folder stays available")
	#[tokio::test]
	async fn an_empty_root_without_live_rows_scans_to_empty() {
		let (state, source_id, tmp) = local_state().await;
		let root = tmp.path().join("local");
		std::fs::remove_dir_all(root.join("Alpha.cbz")).expect("empty the root");

		let outcome = scan_instance(&state, source_id, false, None)
			.await
			.expect("an empty root with no live rows is a plain empty library");
		assert_eq!(outcome.scanned, 0);
		assert_eq!(outcome.trashed, 0);
	}

	/// Hidden entries do not count as content: the scan skips them, so a root holding only a
	/// .DS_Store is exactly the unmounted-mount-point shape
	#[tokio::test]
	async fn hidden_entries_do_not_keep_a_root_available() {
		let (state, source_id, tmp) = local_state().await;
		let root = tmp.path().join("local");
		let manga_id = insert_live_manga(&state, source_id, "Alpha.cbz").await;
		std::fs::remove_dir_all(root.join("Alpha.cbz")).expect("unmount the disk");
		std::fs::write(root.join(".DS_Store"), b"").expect("hidden file");

		let error = scan_instance(&state, source_id, false, None)
			.await
			.expect_err("hidden entries must not count as content");
		assert_eq!(error.code(), "root_unavailable");
		assert_eq!(trashed_at(&state, manga_id).await, None);
	}

	/// A root with visible content is trusted: the manga really is gone, so it trashes normally
	#[tokio::test]
	async fn a_root_with_visible_entries_scans_and_trashes() {
		let (state, source_id, tmp) = local_state().await;
		let root = tmp.path().join("local");
		let manga_id = insert_live_manga(&state, source_id, "Alpha.cbz").await;
		std::fs::remove_dir_all(root.join("Alpha.cbz")).expect("delete the manga");
		std::fs::write(root.join("notes.txt"), b"").expect("visible non-manga file");

		let outcome = scan_instance(&state, source_id, false, None)
			.await
			.expect("a root with visible entries scans normally");
		assert_eq!(outcome.trashed, 1);
		assert!(trashed_at(&state, manga_id).await.is_some());
	}

	#[tokio::test]
	async fn restore_rejects_a_row_that_was_already_restored() {
		let (state, source_id, _tmp) = local_state().await;
		let manga_id: i64 = sqlx::query_scalar(
			"INSERT INTO manga (source_id, key, title, trashed_at, local_entry_kind) VALUES (?, 'Alpha.cbz', 'Alpha', 1, 'directory') RETURNING id",
		)
		.bind(source_id)
		.fetch_one(&state.pool)
		.await
		.expect("trashed manga");

		let (restored_id, _, _) = scan_manga_entry(&state, source_id, "Alpha.cbz", true, true, Some(manga_id))
			.await
			.expect("first restore");
		assert_eq!(restored_id, manga_id);
		let error = scan_manga_entry(&state, source_id, "Alpha.cbz", true, true, Some(manga_id))
			.await
			.expect_err("a second restore must lose to the first");
		assert_eq!(error.code(), "not_trashed");
		let trashed_at: Option<i64> = sqlx::query_scalar("SELECT trashed_at FROM manga WHERE id = ?")
			.bind(manga_id)
			.fetch_one(&state.pool)
			.await
			.expect("restored row");
		assert_eq!(trashed_at, None);
	}

	/// Root attribution follows the key's N: prefix: an empty root 1 fails the scan even while root 0
	/// is fine, and the error names the offending root
	#[tokio::test]
	async fn an_empty_second_root_fails_while_the_first_is_fine() {
		let (state, source_id, tmp) = local_state().await;
		let second = tmp.path().join("mounted");
		std::fs::create_dir_all(&second).expect("empty second root");
		let paths = aidoku_wasm::DefaultValue::StringArray(vec![
			tmp.path().join("local").to_string_lossy().into_owned(),
			second.to_string_lossy().into_owned(),
		]);
		sqlx::query(
			"INSERT INTO source_setting (source_id, key, value) VALUES (?, 'paths', ?)
			 ON CONFLICT(source_id, key) DO UPDATE SET value = excluded.value",
		)
		.bind(source_id)
		.bind(serde_json::to_string(&paths).expect("paths json"))
		.execute(&state.pool)
		.await
		.expect("point the instance at both roots");
		let manga_id = insert_live_manga(&state, source_id, "1:Beta").await;

		let error = scan_instance(&state, source_id, false, None)
			.await
			.expect_err("the empty second root must fail the scan");
		assert_eq!(error.code(), "root_unavailable");
		assert!(
			error.message().contains("mounted"),
			"the error must name the empty root, got: {}",
			error.message()
		);
		assert_eq!(trashed_at(&state, manga_id).await, None);
	}
}
