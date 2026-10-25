//! Scheduled full scan of a local instance (kmrs's scan_root_folder analogue): diffs the indexed manga
//! against the filesystem. A vanished, unprotected manga/chapter is trashed (soft delete) instead of
//! deleted; reappearance clears the flag in place. Indexing is scan-level cheap: title from the leaf
//! directory name and the chapter list from enumeration, initialized = 0 — archive-opening metadata and
//! page analysis belong to the post-scan analysis pass (`analyze.rs`) and to the on-open path.
//!
//! Safety invariants, pinned by tests:
//! - failed-directory protection: anything under an unreadable path is excluded from the trash diff;
//! - empty-scan guard: zero hits with any failed directory trashes nothing (we do not know the state);
//! - a mid-scan IO error fails the whole scan (the module contract: reconciling against half a listing
//!   would mistake unscanned chapters for deleted ones).

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

/// Scheduler state in the setting table, mirroring library.last_scheduled_run_at
pub const LOCAL_LAST_SCAN_AT: &str = "local.last_scan_at";

/// How many persisted rows between inputs checkpoints: the scan reloads roots/oneshots/exclusions/force
/// this often so a settings write mid-run is observed and the handler reruns on the fresh configuration
const INPUTS_CHECKPOINT_EVERY: usize = 32;

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
	/// The inputs checkpoint observed a settings change mid-run (internal: the task handler reruns to
	/// converge on the current configuration; never part of the public outcome JSON)
	#[serde(skip)]
	pub inputs_changed: bool,
}

#[derive(sqlx::FromRow)]
struct PersistedManga {
	id: i64,
	key: String,
	trashed_at: Option<i64>,
	scan_mtime: Option<i64>,
}

/// kmrs's is_protected_url: an entry protects itself and everything below it; keyed forms already carry
/// their root prefix, and a sibling like seriesAB must not match seriesA
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

/// Whether the live instance configuration diverged from the snapshot a scan started with. Called at the
/// inputs checkpoint: a settings write during a long scan must be observed, or the acknowledged rescan
/// would never take effect (one-outstanding-execution keeps a single live task)
async fn inputs_stale(
	state: &AppState,
	source_id: i64,
	roots: &[std::path::PathBuf],
	oneshots: &Option<String>,
	exclusions: &[String],
	force: bool,
) -> ApiResult<bool> {
	Ok(state.local_dirs(source_id).await? != roots
		|| state.local_oneshots_dir(source_id).await? != *oneshots
		|| state.local_exclusions(source_id).await? != exclusions
		|| state.local_force_modified_time(source_id).await? != force)
}

/// Full scan of one local instance. Runs against the live filesystem; every manga's writes commit in their
/// own transaction (the migrate pattern), so a later failure never rolls back earlier, already-correct work.
/// `refresh_metadata` gates the metadata invalidation (changed rows are marked initialized = 0 so the
/// analysis pass re-reads embedded metadata): enumeration/reconcile always happens, metadata re-reads only
/// with the user's opt-in. `cancelled` (the task's flag) stops the per-manga loops between entries; work
/// already committed stands and the next scan continues from it. The inputs checkpoint runs on every
/// `INPUTS_CHECKPOINT_EVERY` rows of both passes plus once after the persisted pass, so even an empty
/// scan observes a settings write and the handler reruns on the fresh configuration
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
	// kmrs's unavailable library: a missing or unreadable root means we cannot know the state of anything
	// under it, so the scan fails outright instead of trashing on a partial view (browse treats a missing
	// root as an empty listing; the lifecycle scan must not)
	for root in &roots {
		match tokio::fs::metadata(root).await {
			Ok(metadata) if metadata.is_dir() => {}
			_ => {
				return Err(crate::error::ApiError::conflict(
					"root_unavailable",
					format!("local root is not a readable directory: {}", root.display()),
				));
			}
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
	let persisted: Vec<PersistedManga> = sqlx::query_as(
		"SELECT id, key, trashed_at, scan_mtime FROM manga WHERE source_id = ?",
	)
	.bind(source_id)
	.fetch_all(&state.pool)
	.await?;
	let mut outcome = ScanOutcome {
		scanned: keys.len() as i64,
		..ScanOutcome::default()
	};
	let now = unix_now();

	let scanned: HashSet<&str> = keys.iter().map(String::as_str).collect();
	if !(keys.is_empty() && !failed.is_empty()) {
		for (row_index, row) in persisted.iter().enumerate() {
			if is_cancelled() {
				break;
			}
			// inputs checkpoint: a settings write during a long scan must be observed, or the acknowledged
			// rescan would never take effect (one-outstanding-execution keeps a single live task). The
			// handler reruns on the fresh configuration when this flags a change
			if row_index % INPUTS_CHECKPOINT_EVERY == 0
				&& inputs_stale(state, source_id, &roots, &oneshots, &exclusions, force).await?
			{
				outcome.inputs_changed = true;
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
			sqlx::query("UPDATE manga SET trashed_at = ? WHERE id = ? AND trashed_at IS NULL")
				.bind(now)
				.bind(row.id)
				.execute(&mut *tx)
				.await?;
			sqlx::query("UPDATE chapter SET trashed_at = ? WHERE manga_id = ? AND trashed_at IS NULL")
				.bind(now)
				.bind(row.id)
				.execute(&mut *tx)
				.await?;
			tx.commit().await?;
			outcome.trashed += 1;
		}
	}
	// the checkpoint covers the whole lifecycle, not just row-heavy scans: an empty scan (no persisted rows
	// at all) must still observe a settings write, or a rescan of an empty instance would never converge
	if !outcome.inputs_changed && inputs_stale(state, source_id, &roots, &oneshots, &exclusions, force).await? {
		outcome.inputs_changed = true;
	}

	let known: HashMap<&str, &PersistedManga> = persisted.iter().map(|row| (row.key.as_str(), row)).collect();
	// the upsert pass runs only when the inputs checkpoint stayed stable: a changed configuration must
	// rerun from the top, not index the rest with stale roots/oneshots/exclusions/force
	if !outcome.inputs_changed {
		for (key_index, key) in keys.iter().enumerate() {
			if is_cancelled() {
				break;
			}
			// the upsert stage stays on the same checkpoint cadence: settings written while earlier rows were
			// being indexed must not let the remaining upserts commit under stale rules
			if key_index % INPUTS_CHECKPOINT_EVERY == 0
				&& inputs_stale(state, source_id, &roots, &oneshots, &exclusions, force).await?
			{
				outcome.inputs_changed = true;
				break;
			}
		let (dir, rel) = super::resolve_key(&roots, key)?;
		let scan = super::scan_chapters(dir, rel, &exclusions).await?;
		let mtime = if force { scan.all_mtime_ms } else { scan.dir_mtime_ms };
		match known.get(key.as_str()) {
			// the fast path: an untouched, non-trashed subtree skips reconciliation entirely. Trashed rows
			// are exempt — reappearance must always restore — and a NULL stored mtime (pre-index row) always
			// reconciles once so the row picks up its stamp
			Some(row) if row.trashed_at.is_none() && row.scan_mtime == Some(mtime) => {
				outcome.unchanged += 1;
			}
			existing => {
				let chapters: Vec<aidoku::Chapter> = scan.chapters.iter().map(super::to_aidoku_chapter).collect();
				let mut tx = crate::db::begin_write(&state.pool).await?;
				let manga_id = match existing {
					Some(row) => {
						// A changed subtree (or a restored row) reconciles; marking initialized = 0 (kmrs's
						// OUTDATED, the analysis pass re-reads embedded metadata) happens only with the
						// caller's metadata opt-in — enumeration is not metadata refresh
						if refresh_metadata {
							sqlx::query("UPDATE manga SET scan_mtime = ?, trashed_at = NULL, initialized = 0 WHERE id = ?")
								.bind(mtime)
								.bind(row.id)
								.execute(&mut *tx)
								.await?;
						} else {
							sqlx::query("UPDATE manga SET scan_mtime = ?, trashed_at = NULL WHERE id = ?")
								.bind(mtime)
								.bind(row.id)
								.execute(&mut *tx)
								.await?;
						}
						if row.trashed_at.is_some() {
							outcome.restored += 1;
						} else {
							outcome.reconciled += 1;
						}
						row.id
					}
					None => {
						// index-level insert: leaf-name title, no metadata (initialized stays 0 for the
						// analysis pass / on-open path)
						let id = sqlx::query(
							"INSERT INTO manga (source_id, key, title, scan_mtime) VALUES (?, ?, ?, ?)",
						)
						.bind(source_id)
						.bind(key)
						.bind(super::title_of(key))
						.bind(mtime)
						.execute(&mut *tx)
						.await?
						.last_insert_rowid();
						outcome.indexed += 1;
						id
					}
				};
				let reconciled =
					reconcile_chapters_detailed(&mut tx, manga_id, &chapters, &scan.failed, true).await?;
				outcome.chapters_trashed += reconciled.trashed;
				outcome.chapters_restored += reconciled.restored;
				tx.commit().await?;
			}
		}
	}
	}
	Ok(outcome)
}

/// Entry-level scan of one manga (manual refresh / open / library update): directory enumeration only,
/// never opens an archive — that work belongs to the analysis pass. Returns the manga id, the chapter
/// reconcile outcome, and whether anything changed. An unchanged subtree (same mtime, not trashed) is a
/// no-op; a changed or reappearing one reconciles and, with `refresh_metadata`, marks the manga
/// initialized = 0 so the analysis pass re-reads its embedded metadata (kmrs's OUTDATED). `force_reconcile`
/// bypasses the unchanged fast path: restore uses it because the manga's trash flag was just cleared and
/// its chapters must be untrashed with it even when the mtime did not move
pub async fn scan_manga_entry(
	state: &AppState,
	source_id: i64,
	manga_key: &str,
	refresh_metadata: bool,
	force_reconcile: bool,
) -> ApiResult<(i64, crate::source::ReconcileOutcome, bool)> {
	let roots = state.local_dirs(source_id).await?;
	let exclusions = state.local_exclusions(source_id).await?;
	let force = state.local_force_modified_time(source_id).await?;
	let (dir, rel) = super::resolve_key(&roots, manga_key)?;
	let scan = super::scan_chapters(dir, rel, &exclusions).await?;
	let mtime = if force { scan.all_mtime_ms } else { scan.dir_mtime_ms };
	let chapters: Vec<aidoku::Chapter> = scan.chapters.iter().map(super::to_aidoku_chapter).collect();
	let existing: Option<(i64, Option<i64>, Option<i64>)> =
		sqlx::query_as("SELECT id, trashed_at, scan_mtime FROM manga WHERE source_id = ? AND key = ?")
			.bind(source_id)
			.bind(manga_key)
			.fetch_optional(&state.pool)
			.await?;
	if let Some((id, None, Some(stored))) = existing
		&& stored == mtime
		&& !force_reconcile
	{
		return Ok((id, crate::source::ReconcileOutcome {
			new_ids: Vec::new(),
			previous_latest_chapter_number: None,
			trashed: 0,
			restored: 0,
		}, false));
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	let manga_id = match existing {
		Some((id, _, _)) => {
			if refresh_metadata {
				sqlx::query("UPDATE manga SET scan_mtime = ?, trashed_at = NULL, initialized = 0 WHERE id = ?")
					.bind(mtime)
					.bind(id)
					.execute(&mut *tx)
					.await?;
			} else {
				sqlx::query("UPDATE manga SET scan_mtime = ?, trashed_at = NULL WHERE id = ?")
					.bind(mtime)
					.bind(id)
					.execute(&mut *tx)
					.await?;
			}
			id
		}
		None => {
			sqlx::query("INSERT INTO manga (source_id, key, title, scan_mtime) VALUES (?, ?, ?, ?)")
				.bind(source_id)
				.bind(manga_key)
				.bind(super::title_of(manga_key))
				.bind(mtime)
				.execute(&mut *tx)
				.await?
				.last_insert_rowid()
		}
	};
	let reconciled = reconcile_chapters_detailed(&mut tx, manga_id, &chapters, &scan.failed, true).await?;
	tx.commit().await?;
	// The analysis pass owns archive-opening work; opening or refreshing an unanalyzed manga kicks it at
	// manual priority (the per-instance dedup id makes repeats free); harnesses without the manager skip
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

/// Registers the three local task kinds on the manager (called from build_app)
pub async fn register_handlers(manager: &crate::task::TaskManager) {
	manager
		.register(KIND_SCAN, Arc::new(|run| {
			Box::pin(async move {
				let source_id = run
					.task
					.source_id
					.ok_or_else(|| crate::error::ApiError::bad_request("local_scan needs a source id"))?;
				let mut refresh_metadata = serde_json::from_str::<Value>(&run.task.payload)?
					.get("refreshMetadata")
					.and_then(Value::as_bool)
					.unwrap_or(false);
				if run.is_cancelled() {
					return Ok(serde_json::to_value(ScanOutcome::default())?);
				}
				let mut outcome = scan_instance(&run.state, source_id, refresh_metadata, Some(&run.cancel_flag())).await?;
				// A settings write (or a manual refreshMetadata=true) requested while this task was live must
				// still take effect: the one-outstanding-execution contract keeps one live task, so when change
				// detection observed newer inputs we rerun with the fresh configuration. refreshMetadata is part
				// of change detection too — a manual refreshMetadata=true during a false run would otherwise be
				// acknowledged and then lost. The cap stops an edit-storm from spinning the scan forever
				let mut reruns = 0;
				loop {
					// reload the live payload after each pass and fold its refreshMetadata flag into the dirty
					// signal (enqueue keeps the payload pointed at the newest trigger)
					if !outcome.inputs_changed {
						let payload: String = sqlx::query_scalar("SELECT payload FROM task WHERE id = ?")
							.bind(&run.task.id)
							.fetch_one(&run.state.pool)
							.await?;
						if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&payload)
							&& let Some(flag) = parsed.get("refreshMetadata").and_then(Value::as_bool)
							&& flag != refresh_metadata
						{
							refresh_metadata = flag;
							outcome.inputs_changed = true;
						}
					}
					if !outcome.inputs_changed {
						break;
					}
					if reruns >= 3 {
						// never report success on a retry cap: newer work is still unapplied, so the task fails
						// loudly and the next trigger (or the next settings write) retries fresh
						return Err(crate::error::ApiError::conflict(
							"inputs_changing",
							"scan inputs kept changing; the scan did not converge. Retry after the configuration settles",
						));
					}
					outcome = scan_instance(&run.state, source_id, refresh_metadata, Some(&run.cancel_flag())).await?;
					reruns += 1;
				}
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

/// Enqueues a scan per local instance (or just the given one); the per-instance dedup id makes a repeat
/// trigger idempotent — kmrs's "a scheduled scan never stacks up". `refresh_metadata` rides the payload:
/// manual triggers and settings edits opt into metadata re-reads, scheduled runs follow the library setting
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
					json!({ "refreshMetadata": refresh_metadata }),
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
	let last: Option<String> = sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
		.bind(LOCAL_LAST_SCAN_AT)
		.fetch_optional(&state.pool)
		.await?;
	let last: Option<i64> = last.and_then(|raw| serde_json::from_str(&raw).ok());
	let due = last.is_none_or(|at| now.saturating_sub(at) >= i64::try_from(interval).unwrap_or(i64::MAX));
	if !due {
		return Ok(false);
	}
	// Stamp before enqueueing: a crash between them would otherwise double-run; at worst the stamp stands
	// and the next tick is one interval later
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(LOCAL_LAST_SCAN_AT)
	.bind(now.to_string())
	.execute(&state.pool)
	.await?;
	let settings = crate::update::LibraryUpdateSettings::load(&state.pool, crate::USER_ID, 0).await?;
	enqueue_scan(state, None, PRIORITY_SCHEDULED, settings.refresh_metadata).await?;
	tracing::info!("scheduled local scan enqueued");
	Ok(true)
}

/// Boot hook for local_scan_on_startup
pub async fn enqueue_startup_scan(state: &AppState) -> ApiResult<()> {
	if state.config.local_scan_on_startup {
		let settings = crate::update::LibraryUpdateSettings::load(&state.pool, crate::USER_ID, 0).await?;
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
/// The on-disk entry must exist and keep its manga shape (a directory for a dir-keyed row, an archive file
/// for a oneshot): accepting an arbitrary regular file would manufacture a fake oneshot chapter that later
/// scans then protect. The untrash itself is a conditional claim on the trashed row, so a purge racing the
/// restore cannot delete the row in between and leave restore reporting success on a row whose progress and
/// library links have already cascaded away — and scan_manga_entry updates the existing row instead of
/// re-inserting a new id
pub async fn restore_manga(state: &Arc<AppState>, manga_id: i64) -> ApiResult<()> {
	let row = crate::source::manga_row(&state.pool, manga_id).await?;
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
				"the directory is still missing on disk",
			));
		}
		Err(_) => {
			return Err(crate::error::ApiError::conflict(
				"gone",
				"the directory is not readable on disk",
			));
		}
		Ok(metadata) => {
			let shape_ok = metadata.is_dir() || (metadata.is_file() && is_archive_name(&row.key));
			if !shape_ok {
				return Err(crate::error::ApiError::conflict(
					"gone",
					"the on-disk entry is no longer a manga directory or archive",
				));
			}
		}
	}
	// claim the untrash conditionally: a purge between this check and the rescan must not win, and the row
	// must not be re-created through the ordinary insert path with a fresh id
	let untrashed = sqlx::query("UPDATE manga SET trashed_at = NULL WHERE id = ? AND trashed_at IS NOT NULL")
		.bind(manga_id)
		.execute(&state.pool)
		.await?
		.rows_affected();
	if untrashed == 0 {
		return Err(crate::error::ApiError::conflict(
			"not_trashed",
			"manga was purged or already restored",
		));
	}
	// entry-level rescan: the row is already untrashed, so force chapter reconciliation — the unchanged
	// fast path would otherwise leave every chapter trashed when the mtime did not move
	scan_manga_entry(state, row.source_id, &row.key, true, true).await?;
	Ok(())
}

/// Hard delete of one trashed manga: the row cascade removes chapters, progress, category memberships and
/// page rows (the same cascade as source deletion). The derived cover cache goes with it, or an id reused
/// after a rescan could show the previous manga's cover
pub async fn purge_manga(state: &AppState, manga_id: i64) -> ApiResult<()> {
	let deleted = sqlx::query("DELETE FROM manga WHERE id = ? AND trashed_at IS NOT NULL")
		.bind(manga_id)
		.execute(&state.pool)
	.await?
	.rows_affected();
	if deleted == 0 {
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
		let deleted = sqlx::query("DELETE FROM manga WHERE id = ? AND trashed_at IS NOT NULL")
			.bind(id)
			.execute(&run.state.pool)
			.await?
			.rows_affected();
		if deleted > 0 {
			crate::source::cover::invalidate_cover(&run.state.config.covers_dir, id).await;
			purged += 1;
		}
	}
	Ok(json!({ "purged": purged, "total": total }))
}
