use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use crate::state::AppState;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
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
/// Read depth when picking the next task: enough to skip past blocked groups without unbounded fetching
const PICK_BATCH: i64 = 16;

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
	/// Group keys currently executing: a local source id, or 0 for the global (whole-library) group.
	/// A global task conflicts with every running task; a source task conflicts with its own source
	/// and with the global group (kmrs GROUP_ID semantics, with 0 reserved — real source ids start at 1)
	running_groups: HashSet<i64>,
	cancels: HashMap<String, Arc<AtomicBool>>,
	active: usize,
}

/// Persistent task queue: rows in the `task` table are the queue, so pending work survives a restart.
/// The in-memory side is only the dispatcher state (running groups, cancel flags).
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
				running_groups: HashSet::new(),
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

	/// Upsert by id: an already queued/running task keeps its slot and only gains priority (a scheduled
	/// scan arriving behind a manual one never piles up); a finished task is requeued fresh
	pub async fn enqueue(
		&self,
		id: &str,
		kind: &str,
		source_id: Option<i64>,
		payload: Value,
		priority: i64,
	) -> ApiResult<TaskRow> {
		sqlx::query(
			"INSERT INTO task (id, kind, source_id, payload, priority, status, created_at)
			 VALUES (?, ?, ?, ?, ?, 'pending', ?)
			 ON CONFLICT (id) DO UPDATE SET
			 	priority = MAX(task.priority, excluded.priority),
			 	kind = excluded.kind,
			 	source_id = excluded.source_id,
			 	status = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN 'pending' ELSE task.status END,
			 	payload = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.payload ELSE task.payload END,
			 	created_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN excluded.created_at ELSE task.created_at END,
			 	started_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.started_at END,
			 	finished_at = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.finished_at END,
			 	error = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.error END,
			 	result = CASE WHEN task.status IN ('done', 'failed', 'cancelled') THEN NULL ELSE task.result END",
		)
		.bind(id)
		.bind(kind)
		.bind(source_id)
		.bind(payload.to_string())
		.bind(priority)
		.bind(unix_now())
		.execute(&self.pool)
		.await?;
		let row = self.get(id).await?.expect("upserted task row");
		let _ = self.signals.send(row.clone());
		self.wake.notify_one();
		Ok(row)
	}

	pub async fn get(&self, id: &str) -> ApiResult<Option<TaskRow>> {
		let row = sqlx::query_as(
			"SELECT id, kind, source_id, payload, priority, status, created_at, started_at, finished_at, error, result
			 FROM task WHERE id = ?",
		)
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
		let mut rows: Vec<TaskRow> = sqlx::query_as(
			"SELECT id, kind, source_id, payload, priority, status, created_at, started_at, finished_at, error, result
			 FROM task
			 ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END,
			 	priority DESC, created_at ASC, finished_at DESC",
		)
		.fetch_all(&self.pool)
		.await?;
		rows.retain(|row| {
			statuses.is_none_or(|wanted| wanted.iter().any(|status| status == &row.status))
				&& kind.is_none_or(|wanted| row.kind == wanted)
				&& source_id.is_none_or(|wanted| row.source_id == Some(wanted))
		});
		Ok(rows)
	}

	/// Pending tasks cancel in place; a running task gets its flag set and the worker records the
	/// cancelled status when the handler returns
	pub async fn cancel(&self, id: &str) -> ApiResult<TaskRow> {
		let row = self.get(id).await?.ok_or_else(|| ApiError::not_found("task not found"))?;
		match row.status.as_str() {
			STATUS_PENDING => {
				sqlx::query("UPDATE task SET status = 'cancelled', finished_at = ? WHERE id = ? AND status = 'pending'")
					.bind(unix_now())
					.bind(id)
					.execute(&self.pool)
					.await?;
			}
			STATUS_RUNNING => {
				let flag = self.inner.lock().await.cancels.get(id).cloned();
				let Some(flag) = flag else {
					// The row says running but the dispatcher has no flag: it is between finishing and
					// cleanup, so there is nothing left to cancel
					return Err(ApiError::conflict("task_finishing", "task is already finishing"));
				};
				flag.store(true, Ordering::SeqCst);
			}
			_ => return Err(ApiError::conflict("task_finished", "task already finished")),
		}
		let row = self.get(id).await?.expect("cancelled task row");
		let _ = self.signals.send(row.clone());
		self.wake.notify_one();
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
			match self.pick_next().await {
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

	async fn pick_next(self: &Arc<Self>) -> ApiResult<Option<(TaskRow, Handler, Arc<AtomicBool>)>> {
		let candidates: Vec<TaskRow> = sqlx::query_as(
			"SELECT id, kind, source_id, payload, priority, status, created_at, started_at, finished_at, error, result
			 FROM task WHERE status = 'pending' ORDER BY priority DESC, created_at ASC LIMIT ?",
		)
		.bind(PICK_BATCH)
		.fetch_all(&self.pool)
		.await?;
		let mut inner = self.inner.lock().await;
		if inner.active >= self.workers {
			return Ok(None);
		}
		for mut task in candidates {
			let group = task.source_id.unwrap_or(0);
			let blocked = if group == 0 {
				!inner.running_groups.is_empty()
			} else {
				inner.running_groups.contains(&group) || inner.running_groups.contains(&0)
			};
			if blocked {
				continue;
			}
			let Some(handler) = inner.handlers.get(&task.kind).cloned() else {
				// A kind this build does not know (a downgrade over a newer database) must not sit
				// pending forever: fail it visibly instead
				sqlx::query("UPDATE task SET status = 'failed', finished_at = ?, error = ? WHERE id = ? AND status = 'pending'")
					.bind(unix_now())
					.bind(format!("no handler registered for task kind {}", task.kind))
					.bind(&task.id)
					.execute(&self.pool)
					.await?;
				let row = self.get(&task.id).await?.expect("failed task row");
				let _ = self.signals.send(row);
				continue;
			};
			// A cancel can land between the SELECT and this UPDATE; the status guard loses that race safely
			let started = sqlx::query("UPDATE task SET status = 'running', started_at = ? WHERE id = ? AND status = 'pending'")
				.bind(unix_now())
				.bind(&task.id)
				.execute(&self.pool)
				.await?
				.rows_affected();
			if started == 0 {
				continue;
			}
			task.status = STATUS_RUNNING.to_string();
			task.started_at = Some(unix_now());
			inner.active += 1;
			inner.running_groups.insert(group);
			let cancelled = Arc::new(AtomicBool::new(false));
			inner.cancels.insert(task.id.clone(), cancelled.clone());
			let _ = self.signals.send(task.clone());
			return Ok(Some((task, handler, cancelled)));
		}
		Ok(None)
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
		let finished = sqlx::query("UPDATE task SET status = ?, finished_at = ?, result = ?, error = ? WHERE id = ?")
			.bind(status)
			.bind(unix_now())
			.bind(result)
			.bind(error)
			.bind(&task.id)
			.execute(&self.pool)
			.await;
		if let Err(e) = finished {
			tracing::warn!(task = %task.id, "task finish write failed: {e}");
		}
		if let Ok(Some(row)) = self.get(&task.id).await {
			let _ = self.signals.send(row);
		}
		{
			let mut inner = self.inner.lock().await;
			inner.active -= 1;
			inner.running_groups.remove(&group);
			inner.cancels.remove(&task.id);
		}
		if let Err(e) = self.trim_finished().await {
			tracing::warn!("task history trim failed: {e}");
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
	async fn finished_task_requeues_fresh() {
		let (_state, manager, _tmp) = test_state(1).await;
		manager.register("noop", ok_handler()).await;
		tokio::spawn(manager.clone().run());
		manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_SCHEDULED)
			.await
			.expect("enqueue");
		let done = wait_status(&manager, "t:1", STATUS_DONE).await;
		assert_eq!(done.result.as_deref(), Some(r#"{"saw":"t:1"}"#));
		let row = manager
			.enqueue("t:1", "noop", None, serde_json::json!({}), PRIORITY_MANUAL)
			.await
			.expect("requeue");
		assert_eq!(row.status, STATUS_PENDING);
		assert_eq!(row.result, None);
		assert_eq!(row.finished_at, None);
		let done = wait_status(&manager, "t:1", STATUS_DONE).await;
		assert!(done.started_at.is_some());
	}

	#[tokio::test]
	async fn same_source_serializes_and_sources_run_in_parallel() {
		let (_state, manager, _tmp) = test_state(2).await;
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
	async fn cancel_pending_never_runs() {
		let (_state, manager, _tmp) = test_state(1).await;
		let ran = Arc::new(AtomicBool::new(false));
		let handler: Handler = Arc::new(move |_run| {
			let ran = ran.clone();
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
		manager.cancel("t:1").await.expect("cancel");
		let row = wait_status(&manager, "t:1", STATUS_CANCELLED).await;
		assert!(row.finished_at.is_some());
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
	async fn list_filters_by_status_kind_and_source() {
		let (_state, manager, _tmp) = test_state(1).await;
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
