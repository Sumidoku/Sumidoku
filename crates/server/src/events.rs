use crate::download::DownloadManager;
use crate::update::UpdateScheduler;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

const EVENT_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct AppEvent {
	pub id: i64,
	pub kind: String,
	pub payload: String,
	pub user_id: Option<i64>,
}

struct EventState {
	next_id: i64,
	events: std::collections::VecDeque<AppEvent>,
}

pub struct EventHub {
	state: tokio::sync::Mutex<EventState>,
	sender: broadcast::Sender<AppEvent>,
}

impl EventHub {
	pub fn new() -> Arc<Self> {
		// A restart must not hand a reconnecting client an earlier run's events under the same cursor.
		let next_id = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_millis() as i64;
		Arc::new(Self {
			state: tokio::sync::Mutex::new(EventState { next_id, events: std::collections::VecDeque::new() }),
			sender: broadcast::channel(256).0,
		})
	}

	pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
		self.sender.subscribe()
	}

	async fn record(&self, kind: &str, payload: Value, user_id: Option<i64>) {
		let mut state = self.state.lock().await;
		state.next_id += 1;
		let event = AppEvent { id: state.next_id, kind: kind.to_string(), payload: payload.to_string(), user_id };
		if state.events.len() == EVENT_CAPACITY {
			state.events.pop_front();
		}
		state.events.push_back(event.clone());
		let _ = self.sender.send(event);
	}

	pub async fn after(&self, user_id: i64, after: i64) -> Vec<AppEvent> {
		self.state
			.lock()
			.await
			.events
			.iter()
			.filter(|event| event.id > after && event.user_id.is_none_or(|owner| owner == user_id))
			.cloned()
			.collect()
	}

	pub async fn latest_id(&self) -> i64 {
		self.state.lock().await.next_id
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
	async fn records_are_broadcast_in_id_order() {
		let hub = EventHub::new();
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

	#[tokio::test]
	async fn ring_keeps_only_the_latest_1024_events() {
		let hub = EventHub::new();
		for chapter_id in 0..1100 {
			hub.record("download", serde_json::json!({ "chapterId": chapter_id }), Some(1)).await;
		}
		let events = hub.after(1, 0).await;
		assert_eq!(events.len(), 1024);
		assert_eq!(events[0].id, events[1023].id - 1023);
		assert!(events.windows(2).all(|pair| pair[0].id < pair[1].id));
	}
}
