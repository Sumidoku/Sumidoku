use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, Notify, broadcast};

/// kmrs priorities: a manual trigger preempts scheduled work, and work derived from a scan trails both
pub const PRIORITY_MANUAL: i64 = 8;
pub const PRIORITY_SCHEDULED: i64 = 4;
pub const PRIORITY_DERIVED: i64 = 1;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";
const TERMINAL: [&str; 3] = [STATUS_DONE, STATUS_FAILED, STATUS_CANCELLED];

/// Finished rows kept for inspection; older ones are pruned as new tasks finish
const FINISHED_KEEP: i64 = 100;
/// Page size for keyset candidate selection: the cursor advances past failed rows too,
/// so they cannot hide runnable work behind them
const PICK_PAGE: i64 = 64;

const COLUMNS: &str = "id, kind, source_id, payload, priority, status, created_at, started_at, finished_at, error, result";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskRow {
	pub id: String,
	pub kind: String,
	pub source_id: Option<i64>,
	/// JSON text in the table; parsed on the way out by to_json
	pub payload: String,
	pub priority: i64,
	pub status: String,
	pub created_at: i64,
	pub started_at: Option<i64>,
	pub finished_at: Option<i64>,
	pub error: Option<String>,
	pub result: Option<String>,
}

impl TaskRow {
	pub fn is_terminal(&self) -> bool {
		TERMINAL.contains(&self.status.as_str())
	}

	/// The wire shape shared by the tasks API and the SSE `task` event
	pub fn to_json(&self) -> Value {
		let parse = |raw: &str| serde_json::from_str(raw).unwrap_or(Value::Null);
		serde_json::json!({
			"id": self.id,
			"kind": self.kind,
			"sourceId": self.source_id,
			"payload": parse(&self.payload),
			"priority": self.priority,
			"status": self.status,
			"createdAt": self.created_at,
			"startedAt": self.started_at,
			"finishedAt": self.finished_at,
			"error": self.error,
			"result": self.result.as_deref().map(parse),
		})
	}
}

/// One execution: the full app state, the row snapshot as dispatched, and the cancellation flag.
/// Handlers that iterate items must check is_cancelled between items; the flag wins over the outcome
/// either way, so a handler that never checks still ends up cancelled once finished.
pub struct TaskRun {
	pub state: Arc<AppState>,
	pub task: TaskRow,
	cancelled: Arc<AtomicBool>,
}

impl TaskRun {
	pub fn is_cancelled(&self) -> bool {
		self.cancelled.load(Ordering::SeqCst)
	}
}

pub type Handler =
	Arc<dyn Fn(TaskRun) -> Pin<Box<dyn Future<Output = ApiResult<Value>> + Send>> + Send + Sync>;

struct Inner {
	handlers: HashMap<String, Handler>,
	/// Group key → owning task id: a local source id, or 0 for the global (whole-library) group.
	/// A global task conflicts with every running task; a source task conflicts with its own source
	/// and with the global group (kmrs GROUP_ID semantics, with 0 reserved — real source ids start
	/// at 1). The owning id lets a finished execution remove only its own registration
	running_groups: HashMap<i64, String>,
	cancels: HashMap<String, Arc<AtomicBool>>,
	active: usize,
}

/// Persistent task queue: rows in the `task` table are the queue, so pending work survives a restart.
/// The in-memory side is only the dispatcher state (running groups, cancel flags). Every transition
/// publishes the row its own write returned (RETURNING), so events never re-read a state a later
/// transition has already replaced
pub struct TaskManager {
	pool: SqlitePool,
	// Weak prevents a reference cycle: AppState holds this manager through a OnceCell
	state: Weak<AppState>,
	signals: broadcast::Sender<TaskRow>,
	wake: Notify,
	inner: Mutex<Inner>,
	workers: usize,
}

impl TaskManager {
	pub fn new(state: &Arc<AppState>) -> Arc<Self> {
		Arc::new(Self {
			pool: state.pool.clone(),
			state: Arc::downgrade(state),
			signals: broadcast::channel(256).0,
			wake: Notify::new(),
			inner: Mutex::new(Inner {
				handlers: HashMap::new(),
				running_groups: HashMap::new(),
				cancels: HashMap::new(),
				active: 0,
			}),
			workers: state.config.task_pool_size.max(1),
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<TaskRow> {
		self.signals.subscribe()
	}

	pub async fn register(&self, kind: &str, handler: Handler) {
		self.inner.lock().await.handlers.insert(kind.to_string(), handler);
	}

	/// Upsert by id. A live (pending/running) task keeps its identity — kind, source and payload stay
	/// with the execution that owns them — and only gains priority; a terminal row is requeued fresh
	/// with the new trigger's identity and priority (a manual run must not permanently promote later
	/// scheduled runs)
	pub async fn enqueue(
		&self,
		id: &str,
		kind: &str,
		source_id: Option<i64>,
		payload: Value,
		priority: i64,
	) -> ApiResult<TaskRow> {
		// Requeues and cancellations publish under the same lock, so an older cancel cannot overtake new work
		let _inner = self.inner.lock().await;
		let row: TaskRow = sqlx::query_as(&format!(
			"INSERT INTO task (id, kind, source_id, payload, priority, status, created_at)
			 VALUES (?, ?, ?, ?, ?, 'pending', ?)
			 ON CONFLICT (id) DO UPDATE SET
				priority = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.priority ELSE MAX(task.priority, excluded.priority) END,
				kind = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.kind ELSE task.kind END,
				source_id = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.source_id ELSE task.source_id END,
				payload = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.payload ELSE task.payload END,
				status = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN 'pending' ELSE task.status END,
				created_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.created_at ELSE task.created_at END,
				started_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.started_at END,
				finished_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.finished_at END,
				error = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.error END,
				result = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.result END
			 RETURNING {COLUMNS}"
		))
		.bind(id)
		.bind(kind)
		.bind(source_id)
		.bind(payload.to_string())
		.bind(priority)
		.bind(unix_now())
		.fetch_one(&self.pool)
		.await?;
		let _ = self.signals.send(row.clone());
		self.wake.notify_one();
		Ok(row)
	}

	pub async fn get(&self, id: &str) -> ApiResult<Option<TaskRow>> {
		let row = sqlx::query_as(&format!("SELECT {COLUMNS} FROM task WHERE id = ?"))
			.bind(id)
			.fetch_optional(&self.pool)
			.await?;
		Ok(row)
	}

	/// Filters are applied in memory: the table is bounded by FINISHED_KEEP plus live work, so a full
	/// scan is cheaper than maintaining a dynamic query
	pub async fn list(
		&self,
		statuses: Option<&[String]>,
		kind: Option<&str>,
		source_id: Option<i64>,
	) -> ApiResult<Vec<TaskRow>> {
		let mut rows: Vec<TaskRow> = sqlx::query_as(&format!(
			"SELECT {COLUMNS} FROM task
			 ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END,
				priority DESC, created_at ASC, finished_at DESC"
		))
		.fetch_all(&self.pool)
		.await?;
		rows.retain(|row| {
			statuses.is_none_or(|wanted| wanted.iter().any(|status| status == &row.status))
				&& kind.is_none_or(|wanted| row.kind == wanted)
				&& source_id.is_none_or(|wanted| row.source_id == Some(wanted))
		});
		Ok(rows)
	}

	/// One atomic transition for pending and running alike: the row goes cancelled and the running
	/// handler's flag is set so it can stop between items. The worker's terminal write is guarded on
	/// status = 'running' and loses to this row state, so a mid-flight cancel can never be overwritten
	/// by a late done. Losing the race to a terminal transition re-reads and answers 409
	pub async fn cancel(&self, id: &str) -> ApiResult<TaskRow> {
		#[cfg(test)]
		if id == CANCEL_BEFORE_UPDATE.id {
			CANCEL_BEFORE_UPDATE.reached.notify_one();
			CANCEL_BEFORE_UPDATE.resume.notified().await;
		}
		let inner = self.inner.lock().await;
		let row = self.get(id).await?.ok_or_else(|| ApiError::not_found("task not found"))?;
		#[cfg(test)]
		if row.id == CANCEL_AFTER_READ.id {
			CANCEL_AFTER_READ.reached.notify_one();
			CANCEL_AFTER_READ.resume.notified().await;
		}
		if row.is_terminal() {
			return Err(ApiError::conflict("task_finished", "task already finished"));
		}
		let updated: Option<TaskRow> = sqlx::query_as(&format!(
			"UPDATE task SET status = 'cancelled', finished_at = ? WHERE id = ? AND status IN ('pending', 'running') RETURNING {COLUMNS}"
		))
		.bind(unix_now())
		.bind(id)
		.fetch_optional(&self.pool)
		.await?;
		let Some(row) = updated else {
			return Err(ApiError::conflict("task_finished", "task already finished"));
		};
		#[cfg(test)]
		if row.id == CANCEL_AFTER_UPDATE.id {
			CANCEL_AFTER_UPDATE.reached.notify_one();
			CANCEL_AFTER_UPDATE.resume.notified().await;
		}
		// A pending→running race can install the flag after the first read, so set whatever is registered now
		if let Some(flag) = inner.cancels.get(id) {
			flag.store(true, Ordering::SeqCst);
		}
		// Requeue uses this lock too, so its events and cancel flag cannot overtake the older cancellation
		let _ = self.signals.send(row.clone());
		self.wake.notify_one();
		drop(inner);
		#[cfg(test)]
		if row.id == CANCEL_BEFORE_REQUEUE.id {
			CANCEL_BEFORE_REQUEUE.reached.notify_one();
			CANCEL_BEFORE_REQUEUE.resume.notified().await;
		}
		self.trim_finished().await?;
		Ok(row)
	}

	/// Boot recovery: a task left running by a dead process goes back to pending and is re-dispatched
	/// (handlers must tolerate re-execution)
	pub async fn recover(&self) -> Result<(), sqlx::Error> {
		sqlx::query("UPDATE task SET status = 'pending', started_at = NULL WHERE status = 'running'")
			.execute(&self.pool)
			.await?;
		self.wake.notify_one();
		Ok(())
	}

	pub async fn run(self: Arc<Self>) {
		loop {
			self.dispatch().await;
			self.wake.notified().await;
		}
	}

	async fn dispatch(self: &Arc<Self>) {
		loop {
			match self.pick_next(None).await {
				Ok(Some((task, handler, cancelled))) => {
					let manager = self.clone();
					tokio::spawn(async move {
						manager.execute(task, handler, cancelled).await;
					});
				}
				Ok(None) => break,
				Err(e) => {
					tracing::warn!("task dispatch failed: {}", e.message());
					break;
				}
			}
		}
	}

	async fn pick_next(
		self: &Arc<Self>,
		after: Option<(i64, i64, String)>,
	) -> ApiResult<Option<(TaskRow, Handler, Arc<AtomicBool>)>> {
		let mut after = after;
		loop {
			let mut inner = self.inner.lock().await;
			if inner.active >= self.workers {
				return Ok(None);
			}
			let cursor = after.as_ref();
			let candidates: Vec<TaskRow> = sqlx::query_as(&format!(
				"SELECT {COLUMNS} FROM task WHERE status = 'pending'
				 AND (? IS NULL OR priority < ? OR (priority = ? AND created_at > ?) OR (priority = ? AND created_at = ? AND id > ?))
				 ORDER BY priority DESC, created_at ASC, id ASC LIMIT ?"
			))
			.bind(cursor.map(|c| c.0))
			.bind(cursor.map(|c| c.0))
			.bind(cursor.map(|c| c.0))
			.bind(cursor.map(|c| c.0))
			.bind(cursor.map(|c| c.1))
			.bind(cursor.map(|c| c.1))
			.bind(cursor.map(|c| c.2.clone()))
			.bind(PICK_PAGE)
			.fetch_all(&self.pool)
			.await?;
			if candidates.is_empty() {
				return Ok(None);
			}
			let next = candidates.last().map(|task| (task.priority, task.created_at, task.id.clone()));
			for task in candidates {
				let group = task.source_id.unwrap_or(0);
				let blocked = if group == 0 {
					!inner.running_groups.is_empty()
				} else {
					inner.running_groups.contains_key(&group) || inner.running_groups.contains_key(&0)
				};
				if blocked {
					continue;
				}
				let Some(handler) = inner.handlers.get(&task.kind).cloned() else {
					// A kind this build does not know (a downgrade over a newer database) must not sit
					// pending forever: fail it visibly instead
					let failed: Option<TaskRow> = sqlx::query_as(&format!(
						"UPDATE task SET status = 'failed', finished_at = ?, error = ? WHERE id = ? AND status = 'pending' RETURNING {COLUMNS}"
					))
					.bind(unix_now())
					.bind(format!("no handler registered for task kind {}", task.kind))
					.bind(&task.id)
					.fetch_optional(&self.pool)
					.await?;
					if let Some(row) = failed {
						let _ = self.signals.send(row);
					}
					self.trim_finished().await?;
					continue;
				};
				// A cancel can land between the SELECT and this UPDATE; the status guard loses that race safely
				let started: Option<TaskRow> = sqlx::query_as(&format!(
					"UPDATE task SET status = 'running', started_at = ? WHERE id = ? AND status = 'pending' RETURNING {COLUMNS}"
				))
				.bind(unix_now())
				.bind(&task.id)
				.fetch_optional(&self.pool)
				.await?;
				let Some(task) = started else {
					continue;
				};
				inner.active += 1;
				inner.running_groups.insert(group, task.id.clone());
				let cancelled = Arc::new(AtomicBool::new(false));
				inner.cancels.insert(task.id.clone(), cancelled.clone());
				let _ = self.signals.send(task.clone());
				return Ok(Some((task, handler, cancelled)));
			}
			// A full blocked page must not spin or recurse the dispatcher while it advances the cursor
			after = next;
			drop(inner);
			tokio::task::yield_now().await;
		}
	}

	async fn execute(self: Arc<Self>, task: TaskRow, handler: Handler, cancelled: Arc<AtomicBool>) {
		let group = task.source_id.unwrap_or(0);
		let outcome = match self.state.upgrade() {
			Some(state) => {
				let run = handler(TaskRun {
					state,
					task: task.clone(),
					cancelled: cancelled.clone(),
				});
				// One more spawn so a panicking handler lands on a JoinError instead of skipping cleanup
				match tokio::spawn(run).await {
					Ok(outcome) => outcome,
					Err(e) => Err(ApiError::internal(format!("task panicked: {e}"))),
				}
			}
			None => Err(ApiError::internal("app state gone")),
		};
		let (status, result, error) = if cancelled.load(Ordering::SeqCst) {
			(STATUS_CANCELLED, None, None)
		} else {
			match outcome {
				Ok(value) => (STATUS_DONE, Some(value.to_string()), None),
				Err(e) => (STATUS_FAILED, None, Some(e.message().to_string())),
			}
		};
		// The terminal write belongs to this execution only: a cancel that landed mid-flight already
		// wrote and published 'cancelled', and losing this guard means publishing nothing more
		let updated: Option<TaskRow> = sqlx::query_as(&format!(
			"UPDATE task SET status = ?, finished_at = ?, result = ?, error = ? WHERE id = ? AND status = 'running' RETURNING {COLUMNS}"
		))
		.bind(status)
		.bind(unix_now())
		.bind(result)
		.bind(error)
		.bind(&task.id)
		.fetch_optional(&self.pool)
		.await
		.unwrap_or_else(|e| {
			tracing::warn!(task = %task.id, "task finish write failed: {e}");
			None
		});
		if let Some(row) = updated {
			let _ = self.signals.send(row);
			if let Err(e) = self.trim_finished().await {
				tracing::warn!("task history trim failed: {e}");
			}
		}
		{
			let mut inner = self.inner.lock().await;
			inner.active -= 1;
			// A terminal requeue can already have dispatched the next execution of this group or id:
			// remove only the registrations this execution owns
			if inner.running_groups.get(&group) == Some(&task.id) {
				inner.running_groups.remove(&group);
			}
			if let Some(flag) = inner.cancels.get(&task.id)
				&& Arc::ptr_eq(flag, &cancelled)
			{
				inner.cancels.remove(&task.id);
			}
		}
		self.wake.notify_one();
	}

	async fn trim_finished(&self) -> Result<(), sqlx::Error> {
		sqlx::query(
			"DELETE FROM task WHERE status IN ('done', 'failed', 'cancelled') AND id NOT IN (
				SELECT id FROM task WHERE status IN ('done', 'failed', 'cancelled')
				ORDER BY finished_at DESC, id DESC LIMIT ?
			)",
		)
		.bind(FINISHED_KEEP)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
}

#[cfg(test)]
struct CancelTestGate {
	id: &'static str,
	reached: Notify,
	resume: Notify,
}

#[cfg(test)]
static CANCEL_BEFORE_UPDATE: CancelTestGate = CancelTestGate {
	id: "cancel-race-test",
	reached: Notify::const_new(),
	resume: Notify::const_new(),
};
#[cfg(test)]
static CANCEL_AFTER_READ: CancelTestGate = CancelTestGate {
	id: "cancel-after-read-test",
	reached: Notify::const_new(),
	resume: Notify::const_new(),
};
#[cfg(test)]
static CANCEL_AFTER_UPDATE: CancelTestGate = CancelTestGate {
	id: "cancel-new-execution-test",
	reached: Notify::const_new(),
	resume: Notify::const_new(),
};
#[cfg(test)]
static CANCEL_BEFORE_REQUEUE: CancelTestGate = CancelTestGate {
	id: "cancel-requeue-test",
	reached: Notify::const_new(),
	resume: Notify::const_new(),
};

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::AtomicI64;
	use tokio::sync::mpsc;

	async fn test_state(workers: usize) -> (Arc<AppState>, Arc<TaskManager>, tempfile::TempDir) {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		let config = crate::config::Config {
			task_pool_size: workers,
			..crate::config::Config::default()
		};
		let state = AppState::new(pool, config, defaults_tx, cookie_tx);
		let manager = TaskManager::new(&state);
		assert!(state.tasks.set(manager.clone()).is_ok());
		(state, manager, tmp)
	}

	/// task.source_id references source rows: the seeded default local instance is id 1; further ids need rows
	async fn insert_sources(manager: &TaskManager, count: i64) {
		for index in 2..=count {
			sqlx::query("INSERT INTO source (key, name, kind, wasm_path) VALUES (?, 'T', 'local', '')")
				.bind(format!("local.t{index}"))
				.execute(&manager.pool)
				.await
				.expect("insert source row");
		}
	}

	async fn wait_status(manager: &TaskManager, id: &str, status: &str) -> TaskRow {
		let started = std::time::Instant::now();
		loop {
			if let Some(row) = manager.get(id).await.expect("task row")
				&& row.status == status
			{
				return row;
			}
			assert!(started.elapsed() < std::time::Duration::from_secs(5), "timed out waiting for {id} = {status}");
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	}

	fn ok_handler() -> Handler {
		Arc::new(|run| {
			Box::pin(async move { Ok(serde_json::json!({ "saw": run.task.id })) })
		})
	}

	#[tokio::test]
	async fn enqueue_dedups_and_bumps_priority() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		// No run loop: the task stays queued so both enqueues hit the same pending row
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("first enqueue");
		let row = manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("second enqueue");
		assert_eq!(row.status, STATUS_PENDING);
		assert_eq!(row.priority, PRIORITY_MANUAL);
		let rows = manager.list(None, None, None).await.expect("list");
		assert_eq!(rows.len(), 1, "a duplicate id must not pile up");
	}

	#[tokio::test]
	async fn live_enqueue_keeps_identity_terminal_requeue_resets_it() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		// A live row's identity belongs to the execution that owns it: only the priority moves
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({"a": 1}), PRIORITY_SCHEDULED)
			.await
			.expect("enqueue");
		let row = manager
			.enqueue("t:1", "other", Some(1), serde_json::json!({"b": 2}), PRIORITY_MANUAL)
			.await
			.expect("live re-enqueue");
		assert_eq!((row.kind.as_str(), row.source_id, row.payload.as_str()), ("noop", None, r#"{"a":1}"#));
		assert_eq!(row.priority, PRIORITY_MANUAL);

		tokio::spawn(manager.clone().run());
		wait_status(&manager, "t:1", STATUS_DONE).await;
		// A terminal requeue takes the new trigger's identity AND priority (no permanent promotion)
		let row = manager
			.enqueue("t:1", "noop", Some(1), serde_json::json!({"b": 2}), PRIORITY_DERIVED)
			.await
			.expect("terminal requeue");
		assert_eq!(row.priority, PRIORITY_DERIVED);
		assert_eq!(row.source_id, Some(1));
		assert_eq!(row.status, STATUS_PENDING);
		assert_eq!(row.result, None);
		assert_eq!(row.finished_at, None);
		wait_status(&manager, "t:1", STATUS_DONE).await;
	}

	#[tokio::test]
	async fn enqueue_while_running_queues_no_follow_up() {
		let (_state, manager, _tmp) = test_state(1).await;
		let runs = Arc::new(AtomicI64::new(0));
		let handler_runs = runs.clone();
		let handler: Handler = Arc::new(move |_run| {
			let runs = handler_runs.clone();
			Box::pin(async move {
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
				runs.fetch_add(1, Ordering::SeqCst);
				Ok(serde_json::json!({}))
			})
		});
		manager.register("slow", handler).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "slow", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("enqueue");
		wait_status(&manager, "t:1", STATUS_RUNNING).await;
		// The contract (and kmrs's queue): one outstanding execution per id — a duplicate arriving
		// mid-run bumps priority and queues no follow-up run
		manager
			.enqueue("t:1", "slow", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("re-enqueue while running");
		wait_status(&manager, "t:1", STATUS_DONE).await;
		tokio::time::sleep(std::time::Duration::from_millis(150)).await;
		assert_eq!(runs.load(Ordering::SeqCst), 1, "no follow-up run after a mid-run duplicate");
		let row = manager.get("t:1").await.expect("row").expect("exists");
		assert_eq!(row.status, STATUS_DONE);
	}

	#[tokio::test]
	async fn same_source_serializes_and_sources_run_in_parallel() {
		let (_state, manager, _tmp) = test_state(2).await;
		insert_sources(&manager, 2).await;
		// Per-group in-flight counters: group peaks prove serialization, the global peak proves parallelism
		let peak_one = Arc::new(AtomicI64::new(0));
		let peak_two = Arc::new(AtomicI64::new(0));
		let global_peak = Arc::new(AtomicI64::new(0));
		let counter = |peak: Arc<AtomicI64>| {
			let current = Arc::new(AtomicI64::new(0));
			move || (current.clone(), peak.clone())
		};
		let counter_one = counter(peak_one.clone());
		let counter_two = counter(peak_two.clone());
		let global_current = Arc::new(AtomicI64::new(0));
		let peak_seen = global_peak.clone();
		let handler: Handler = Arc::new(move |run| {
			let (current, peak) = match run.task.source_id {
				Some(1) => counter_one(),
				_ => counter_two(),
			};
			let global_current = global_current.clone();
			let global_peak = global_peak.clone();
			Box::pin(async move {
				peak.fetch_max(current.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
				global_peak.fetch_max(global_current.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
				current.fetch_sub(1, Ordering::SeqCst);
				global_current.fetch_sub(1, Ordering::SeqCst);
				Ok(serde_json::json!({}))
			})
		});
		manager.register("grouped", handler).await;
		tokio::spawn(manager.clone().run());
		for (id, source) in [("a", Some(1)), ("b", Some(1)), ("c", Some(2)), ("d", Some(2))] {
			manager
				.enqueue(id, "grouped", source, serde_json::json!({}), PRIORITY_MANUAL)
				.await
				.expect("enqueue");
		}
		for id in ["a", "b", "c", "d"] {
			wait_status(&manager, id, STATUS_DONE).await;
		}
		assert_eq!(peak_one.load(Ordering::SeqCst), 1, "source 1's tasks must never overlap");
		assert_eq!(peak_two.load(Ordering::SeqCst), 1, "source 2's tasks must never overlap");
		assert_eq!(peak_seen.load(Ordering::SeqCst), 2, "different sources run in parallel");
	}

	#[tokio::test]
	async fn a_blocked_group_never_starves_runnable_work() {
		let (_state, manager, _tmp) = test_state(2).await;
		insert_sources(&manager, 2).await;
		let handler: Handler = Arc::new(|run| {
			Box::pin(async move {
				if run.task.id == "t:blocker" {
					tokio::time::sleep(std::time::Duration::from_millis(300)).await;
				}
				Ok(serde_json::json!({}))
			})
		});
		manager.register("grouped", handler).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:blocker", "grouped", Some(1), serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("blocker");
		wait_status(&manager, "t:blocker", STATUS_RUNNING).await;
		// One full page of blocked same-source pendings in front of the runnable one
		for index in 0..64 {
			manager
				.enqueue(&format!("t:same-{index}"), "grouped", Some(1), serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("enqueue same-source");
		}
		manager
			.enqueue("t:runnable", "grouped", Some(2), serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("enqueue runnable");
		// The runnable task finishes while the blocker is still holding its group
		wait_status(&manager, "t:runnable", STATUS_DONE).await;
		let blocker = manager.get("t:blocker").await.expect("row").expect("exists");
		assert_eq!(blocker.status, STATUS_RUNNING, "the blocker outlives the runnable task");
	}

	#[tokio::test]
	async fn cancel_pending_never_runs() {
		let (_state, manager, _tmp) = test_state(1).await;
		let ran = Arc::new(AtomicBool::new(false));
		let ran_in_handler = ran.clone();
		let handler: Handler = Arc::new(move |_run| {
			let ran = ran_in_handler.clone();
			Box::pin(async move {
				ran.store(true, Ordering::SeqCst);
				Ok(serde_json::json!({}))
			})
		});
		manager.register("noop", handler).await;
		// No run loop started yet: cancel while pending
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		let row = manager.cancel("t:1").await.expect("cancel");
		assert_eq!(row.status, STATUS_CANCELLED);
		assert!(row.finished_at.is_some());
		tokio::spawn(manager.clone().run());
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		assert!(!ran.load(Ordering::SeqCst));
		let row = manager.get("t:1").await.expect("row").expect("exists");
		assert_eq!(row.status, STATUS_CANCELLED);
	}

	#[tokio::test]
	async fn cancel_running_stops_between_items() {
		let (_state, manager, _tmp) = test_state(1).await;
		let items_done = Arc::new(AtomicI64::new(0));
		let counted = items_done.clone();
		let handler: Handler = Arc::new(move |run| {
			let items_done = counted.clone();
			Box::pin(async move {
				loop {
					if run.is_cancelled() {
						return Ok(serde_json::json!({ "stopped": true }));
					}
					items_done.fetch_add(1, Ordering::SeqCst);
					tokio::time::sleep(std::time::Duration::from_millis(10)).await;
				}
			})
		});
		manager.register("loop", handler).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "loop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		wait_status(&manager, "t:1", STATUS_RUNNING).await;
		let row = manager.cancel("t:1").await.expect("cancel");
		assert_eq!(row.status, STATUS_CANCELLED, "the row is cancelled the moment cancel lands");
		tokio::time::sleep(std::time::Duration::from_millis(150)).await;
		let row = manager.get("t:1").await.expect("row").expect("exists");
		assert_eq!(row.status, STATUS_CANCELLED, "the worker's finish must not overwrite the cancel");
		assert!(items_done.load(Ordering::SeqCst) > 0);
	}

	#[tokio::test]
	async fn cancel_terminal_and_unknown_ids_are_rejected() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		wait_status(&manager, "t:1", STATUS_DONE).await;
		let error = manager.cancel("t:1").await.expect_err("finished task");
		assert_eq!(error.code(), "task_finished");
		let error = manager.cancel("t:nope").await.expect_err("unknown task");
		assert_eq!(error.code(), "not_found");
	}

	#[tokio::test]
	async fn recover_requeues_interrupted_running_tasks() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		// Simulate a process dying mid-run: the row is stuck in running
		sqlx::query("UPDATE task SET status = 'running', started_at = 1 WHERE id = 't:1'")
			.execute(&manager.pool)
			.await
			.expect("stuck running");
		manager.recover().await.expect("recover");
		tokio::spawn(manager.clone().run());
		let row = wait_status(&manager, "t:1", STATUS_DONE).await;
		assert!(row.started_at.is_some_and(|at| at > 1));
	}

	#[tokio::test]
	async fn unknown_kind_fails_visibly() {
		let (_state, manager, _tmp) = test_state(1).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "ghost", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		let row = wait_status(&manager, "t:1", STATUS_FAILED).await;
		assert!(row.error.as_deref().is_some_and(|error| error.contains("ghost")));
	}

	#[tokio::test]
	async fn history_is_trimmed_to_the_latest_finished() {
		let (_state, manager, _tmp) = test_state(2).await;
		insert_sources(&manager, 1).await;
		manager.register("noop", ok_handler()).await;
		tokio::spawn(manager.clone().run());
		for index in 0..(FINISHED_KEEP + 10) {
			manager
				.enqueue(&format!("t:{index}"), "noop", Some(1), serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("enqueue");
		}
		wait_status(&manager, &format!("t:{}", FINISHED_KEEP + 9), STATUS_DONE).await;
		let started = std::time::Instant::now();
		loop {
			let finished: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE status IN ('done', 'failed', 'cancelled')")
				.fetch_one(&manager.pool)
				.await
				.expect("finished count");
			if finished <= FINISHED_KEEP {
				break;
			}
			assert!(started.elapsed() < std::time::Duration::from_secs(5), "history not trimmed: {finished}");
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	}

	#[tokio::test]
	async fn pending_cancellation_is_also_trimmed() {
		let (_state, manager, _tmp) = test_state(1).await;
		// No run loop: every task is cancelled out of the pending state
		for index in 0..(FINISHED_KEEP + 10) {
			manager
				.enqueue(&format!("t:{index}"), "noop", None, serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("enqueue");
			manager.cancel(&format!("t:{index}")).await.expect("cancel");
		}
		let finished: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE status = 'cancelled'")
			.fetch_one(&manager.pool)
			.await
			.expect("cancelled count");
		assert_eq!(finished, FINISHED_KEEP, "pending cancels trim to the same bound");
	}

	#[tokio::test]
	async fn transitions_are_broadcast_in_order() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		let mut rx = manager.subscribe();
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		let mut statuses = Vec::new();
		while let Ok(event) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
			let event = event.expect("signal");
			if event.id != "t:1" {
				continue;
			}
			statuses.push(event.status.clone());
			if event.status == STATUS_DONE {
				break;
			}
		}
		assert_eq!(statuses, [STATUS_PENDING, STATUS_RUNNING, STATUS_DONE]);
	}

	#[tokio::test]
	async fn failed_kinds_do_not_skip_pending_candidates() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		for index in 0..193 {
			manager
				.enqueue(&format!("ghost-{index:03}"), "ghost", None, serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("enqueue unknown kind");
		}
		manager
			.enqueue("valid", "noop", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("enqueue valid task");
		manager.dispatch().await;
		let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE kind = 'ghost' AND status = 'pending'")
			.fetch_one(&manager.pool)
			.await
			.expect("pending unknown kinds");
		assert_eq!(pending, 0, "failed kinds must not advance past runnable candidates");
		wait_status(&manager, "valid", STATUS_DONE).await;
	}

	#[tokio::test]
	async fn cancel_during_pending_to_running_sets_handler_flag() {
		let (_state, manager, _tmp) = test_state(1).await;
		let started = Arc::new(Notify::new());
		let release = Arc::new(Notify::new());
		let finished = Arc::new(Notify::new());
		let saw_cancel = Arc::new(AtomicBool::new(false));
		let handler: Handler = Arc::new({
			let started = started.clone();
			let release = release.clone();
			let finished = finished.clone();
			let saw_cancel = saw_cancel.clone();
			move |run| {
				let started = started.clone();
				let release = release.clone();
				let finished = finished.clone();
				let saw_cancel = saw_cancel.clone();
				Box::pin(async move {
					started.notify_one();
					release.notified().await;
					saw_cancel.store(run.is_cancelled(), Ordering::SeqCst);
					finished.notify_one();
					Ok(serde_json::json!({}))
				})
			}
		});
		manager.register("race", handler).await;
		manager
			.enqueue("cancel-race-test", "race", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");

		let cancel_manager = manager.clone();
		let cancel = tokio::spawn(async move { cancel_manager.cancel("cancel-race-test").await });
		CANCEL_BEFORE_UPDATE.reached.notified().await;
		manager.dispatch().await;
		started.notified().await;
		CANCEL_BEFORE_UPDATE.resume.notify_one();
		let row = cancel.await.expect("cancel task").expect("cancel");
		assert_eq!(row.status, STATUS_CANCELLED);
		release.notify_one();
		tokio::time::timeout(std::time::Duration::from_secs(5), finished.notified())
			.await
			.expect("handler did not finish");
		assert!(saw_cancel.load(Ordering::SeqCst), "the handler must observe a cancel that raced its dispatch");
	}

	#[tokio::test]
	async fn cancellation_is_published_before_terminal_requeue() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		let mut rx = manager.subscribe();
		manager
			.enqueue("cancel-requeue-test", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		let cancel_manager = manager.clone();
		let cancel = tokio::spawn(async move { cancel_manager.cancel("cancel-requeue-test").await });
		CANCEL_BEFORE_REQUEUE.reached.notified().await;
		manager
			.enqueue("cancel-requeue-test", "noop", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("requeue");
		CANCEL_BEFORE_REQUEUE.resume.notify_one();
		cancel.await.expect("cancel task").expect("cancel");

		let mut statuses = Vec::new();
		for _ in 0..3 {
			let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
				.await
				.expect("missing task event")
				.expect("event");
			assert_eq!(event.id, "cancel-requeue-test");
			statuses.push(event.status);
		}
		assert_eq!(statuses, [STATUS_PENDING, STATUS_CANCELLED, STATUS_PENDING]);
	}

	#[tokio::test]
	async fn keyset_cursor_covers_priority_when_created_ties() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("grouped", ok_handler()).await;
		manager.inner.lock().await.running_groups.insert(1, "blocker".into());
		for (id, priority) in [("a", 8), ("b", 1), ("c", 8)] {
			manager
				.enqueue(id, "grouped", Some(1), serde_json::json!({}), priority)
				.await
				.expect("enqueue");
		}
		sqlx::query("UPDATE task SET created_at = 1")
			.execute(&manager.pool)
			.await
			.expect("tie creation times");
		let next = tokio::time::timeout(
			std::time::Duration::from_millis(250),
			manager.pick_next(Some((1, 1, "b".to_string()))),
		)
		.await
		.expect("cursor must not bounce between priorities")
		.expect("pick next");
		assert!(next.is_none(), "no runnable candidate follows the cursor");
	}

	async fn assert_requeue_does_not_inherit_cancel(gate: &CancelTestGate) {
		let id = gate.id;
		let (_state, manager, _tmp) = test_state(1).await;
		let started = Arc::new(Notify::new());
		let release = Arc::new(Notify::new());
		let finished = Arc::new(Notify::new());
		let saw_cancel = Arc::new(AtomicBool::new(false));
		let handler: Handler = Arc::new({
			let started = started.clone();
			let release = release.clone();
			let finished = finished.clone();
			let saw_cancel = saw_cancel.clone();
			move |run| {
				let started = started.clone();
				let release = release.clone();
				let finished = finished.clone();
				let saw_cancel = saw_cancel.clone();
				Box::pin(async move {
					started.notify_one();
					release.notified().await;
					saw_cancel.store(run.is_cancelled(), Ordering::SeqCst);
					finished.notify_one();
					Ok(serde_json::json!({}))
				})
			}
		});
		manager.register("race", handler).await;
		manager
			.enqueue(id, "race", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");

		let cancel_manager = manager.clone();
		let cancel = tokio::spawn(async move { cancel_manager.cancel(id).await });
		gate.reached.notified().await;
		let requeue_done = Arc::new(AtomicBool::new(false));
		let requeue_manager = manager.clone();
		let requeue_flag = requeue_done.clone();
		let requeue = tokio::spawn(async move {
			requeue_manager
				.enqueue(id, "race", None, serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("requeue");
			requeue_flag.store(true, Ordering::SeqCst);
		});
		let wait_started = std::time::Instant::now();
		while !requeue_done.load(Ordering::SeqCst) && wait_started.elapsed() < std::time::Duration::from_millis(100) {
			tokio::time::sleep(std::time::Duration::from_millis(1)).await;
		}
		let dispatched_before_resume = requeue_done.load(Ordering::SeqCst);
		if dispatched_before_resume {
			manager.dispatch().await;
		}
		gate.resume.notify_one();
		cancel.await.expect("cancel task").expect("cancel");
		requeue.await.expect("requeue task");
		if !dispatched_before_resume {
			manager.dispatch().await;
		}
		started.notified().await;
		release.notify_one();
		tokio::time::timeout(std::time::Duration::from_secs(5), finished.notified())
			.await
			.expect("handler did not finish");
		assert!(!saw_cancel.load(Ordering::SeqCst), "the old cancel must not cancel the new execution");
	}

	#[tokio::test]
	async fn cancel_after_read_does_not_flag_a_terminal_requeue() {
		let (_state, manager, _tmp) = test_state(1).await;
		let old_started = Arc::new(Notify::new());
		let finish_old = Arc::new(Notify::new());
		let new_started = Arc::new(Notify::new());
		let release_new = Arc::new(Notify::new());
		let new_finished = Arc::new(Notify::new());
		let saw_cancel = Arc::new(AtomicBool::new(false));
		let executions = Arc::new(AtomicI64::new(0));
		let handler: Handler = Arc::new({
			let old_started = old_started.clone();
			let finish_old = finish_old.clone();
			let new_started = new_started.clone();
			let release_new = release_new.clone();
			let new_finished = new_finished.clone();
			let saw_cancel = saw_cancel.clone();
			let executions = executions.clone();
			move |run| {
				let old_started = old_started.clone();
				let finish_old = finish_old.clone();
				let new_started = new_started.clone();
				let release_new = release_new.clone();
				let new_finished = new_finished.clone();
				let saw_cancel = saw_cancel.clone();
				let executions = executions.clone();
				Box::pin(async move {
					if executions.fetch_add(1, Ordering::SeqCst) == 0 {
						old_started.notify_one();
						finish_old.notified().await;
					} else {
						new_started.notify_one();
						release_new.notified().await;
						saw_cancel.store(run.is_cancelled(), Ordering::SeqCst);
						new_finished.notify_one();
					}
					Ok(serde_json::json!({}))
				})
			}
		});
		manager.register("race", handler).await;
		manager
			.enqueue(CANCEL_AFTER_READ.id, "race", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue");
		manager.dispatch().await;
		old_started.notified().await;

		let cancel_manager = manager.clone();
		let cancel = tokio::spawn(async move { cancel_manager.cancel(CANCEL_AFTER_READ.id).await });
		CANCEL_AFTER_READ.reached.notified().await;
		finish_old.notify_one();
		wait_status(&manager, CANCEL_AFTER_READ.id, STATUS_DONE).await;
		let requeue_done = Arc::new(AtomicBool::new(false));
		let requeue_manager = manager.clone();
		let requeue_flag = requeue_done.clone();
		let requeue = tokio::spawn(async move {
			requeue_manager
				.enqueue(CANCEL_AFTER_READ.id, "race", None, serde_json::json!({}), PRIORITY_SCHEDULED)
				.await
				.expect("requeue");
			requeue_flag.store(true, Ordering::SeqCst);
		});
		let wait_started = std::time::Instant::now();
		while !requeue_done.load(Ordering::SeqCst) && wait_started.elapsed() < std::time::Duration::from_millis(100) {
			tokio::time::sleep(std::time::Duration::from_millis(1)).await;
		}
		let dispatched_before_resume = requeue_done.load(Ordering::SeqCst);
		if dispatched_before_resume {
			manager.dispatch().await;
		}
		CANCEL_AFTER_READ.resume.notify_one();
		let outcome = cancel.await.expect("cancel task");
		assert_eq!(outcome.expect_err("after-read cancel must lose to the terminal transition").code(), "task_finished");
		requeue.await.expect("requeue task");
		if !dispatched_before_resume {
			manager.dispatch().await;
		}
		new_started.notified().await;
		release_new.notify_one();
		tokio::time::timeout(std::time::Duration::from_secs(5), new_finished.notified())
			.await
			.expect("new handler did not finish");
		assert!(!saw_cancel.load(Ordering::SeqCst), "the old cancel must not cancel the new execution");
	}

	#[tokio::test]
	async fn cancel_after_update_does_not_flag_a_terminal_requeue() {
		assert_requeue_does_not_inherit_cancel(&CANCEL_AFTER_UPDATE).await;
	}

	#[tokio::test]
	async fn list_filters_by_status_kind_and_source() {
		let (_state, manager, _tmp) = test_state(1).await;
		insert_sources(&manager, 2).await;
		manager.register("noop", ok_handler()).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "noop", Some(1), serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue 1");
		manager
			.enqueue("t:2", "noop", Some(2), serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("enqueue 2");
		wait_status(&manager, "t:1", STATUS_DONE).await;
		wait_status(&manager, "t:2", STATUS_DONE).await;
		let done = manager
			.list(Some(&[STATUS_DONE.to_string()]), None, None)
			.await
			.expect("done filter");
		assert_eq!(done.len(), 2);
		let running = manager
			.list(Some(&[STATUS_RUNNING.to_string()]), None, None)
			.await
			.expect("running filter");
		assert!(running.is_empty());
		let sourced = manager.list(None, None, Some(2)).await.expect("source filter");
		assert_eq!(sourced.len(), 1);
		assert_eq!(sourced[0].id, "t:2");
		let kind = manager.list(None, Some("noop"), None).await.expect("kind filter");
		assert_eq!(kind.len(), 2);
		let kind = manager.list(None, Some("other"), None).await.expect("kind filter");
		assert!(kind.is_empty());
	}
}
