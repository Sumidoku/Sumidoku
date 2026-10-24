use crate::download::DownloadManager;
use crate::update::UpdateScheduler;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

pub const EVENT_RETENTION_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AppEvent {
	pub id: i64,
	pub kind: String,
	pub payload: String,
	pub user_id: Option<i64>,
}

pub struct EventHub {
	pool: sqlx::SqlitePool,
	sender: broadcast::Sender<AppEvent>,
	record_lock: tokio::sync::Mutex<()>,
}

impl EventHub {
	pub fn new(pool: sqlx::SqlitePool) -> Arc<Self> {
		Arc::new(Self {
			pool,
			sender: broadcast::channel(256).0,
			record_lock: tokio::sync::Mutex::new(()),
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
		self.sender.subscribe()
	}

	async fn record(&self, kind: &str, payload: Value, user_id: Option<i64>) {
		let _guard = self.record_lock.lock().await;
		let now = crate::model::unix_now();
		let result = sqlx::query("INSERT INTO app_event (kind, payload, user_id, created_at) VALUES (?, ?, ?, ?)")
			.bind(kind)
			.bind(payload.to_string())
			.bind(user_id)
			.bind(now)
			.execute(&self.pool)
			.await;
		let id = match result {
			Ok(result) => result.last_insert_rowid(),
			Err(e) => {
				tracing::warn!(kind, "event journal write failed: {e}");
				return;
			}
		};
		let _ = sqlx::query("DELETE FROM app_event WHERE created_at < ?")
			.bind(now - EVENT_RETENTION_SECS)
			.execute(&self.pool)
			.await;
		let _ = self.sender.send(AppEvent { id, kind: kind.to_string(), payload: payload.to_string(), user_id });
	}

	pub async fn after(&self, user_id: i64, after: i64) -> Result<Vec<AppEvent>, sqlx::Error> {
		sqlx::query_as::<_, AppEvent>(
			"SELECT id, kind, payload, user_id FROM app_event
			 WHERE id > ? AND created_at >= ? AND (user_id = ? OR user_id IS NULL) ORDER BY id",
		)
		.bind(after)
		.bind(crate::model::unix_now() - EVENT_RETENTION_SECS)
		.bind(user_id)
		.fetch_all(&self.pool)
		.await
	}

	#[cfg(test)]
	pub(crate) async fn record_for_test(&self, kind: &str, payload: Value, user_id: Option<i64>) {
		self.record(kind, payload, user_id).await;
	}

	pub fn forward_downloads(self: &Arc<Self>, manager: Arc<DownloadManager>) {
		let mut rx = manager.subscribe();
		let hub = self.clone();
		tokio::spawn(async move {
			loop {
				match rx.recv().await {
					Ok(event) => {
						if event.kind == "running" {
							hub.record("running", json!({ "running": event.running }), event.user_id).await;
						} else {
							let payload = serde_json::to_value(&event).unwrap_or_default();
							hub.record("download", payload, event.user_id).await;
						}
					}
					Err(broadcast::error::RecvError::Lagged(_)) => {
						hub.record("resync", Value::Null, None).await;
					}
					Err(_) => break,
				}
			}
		});
	}

	pub fn forward_update(self: &Arc<Self>, scheduler: Arc<UpdateScheduler>) {
		let mut rx = scheduler.subscribe_signals();
		let hub = self.clone();
		tokio::spawn(async move {
			loop {
				match rx.recv().await {
					Ok(crate::update::UpdateSignal::Job(event)) => {
						let payload = serde_json::to_value(&event).unwrap_or_default();
						hub.record("update", payload, event.user_id).await;
					}
					Ok(crate::update::UpdateSignal::Summary(summary)) => {
						let kind = if summary.running { "updateSummary" } else { "updateComplete" };
						let user_id = summary.user_id;
						let payload = serde_json::to_value(summary).unwrap_or_default();
						hub.record(kind, payload, user_id).await;
					}
					Err(broadcast::error::RecvError::Lagged(_)) => {
						hub.record("resync", Value::Null, None).await;
					}
					Err(_) => break,
				}
			}
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
	async fn records_are_broadcast_in_journal_order() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let hub = EventHub::new(pool);
		let mut rx = hub.subscribe();
		for _ in 0..8 {
			let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(64));
			let mut tasks = tokio::task::JoinSet::new();
			for chapter_id in 0..64 {
				let hub = hub.clone();
				let barrier = barrier.clone();
				tasks.spawn(async move {
					barrier.wait().await;
					hub.record("download", serde_json::json!({ "chapterId": chapter_id }), Some(1)).await;
				});
			}
			while tasks.join_next().await.is_some() {}
			let mut ids = Vec::new();
			for _ in 0..64 {
				ids.push(rx.try_recv().expect("recorded event").id);
			}
			assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");
		}
	}
}
