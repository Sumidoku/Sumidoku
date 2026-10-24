use crate::events::AppEvent;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::response::Response;
use futures_core::Stream;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Duration, MissedTickBehavior, interval};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

enum Wire {
	Comment(&'static str),
	Event(AppEvent),
	Snapshot(Value),
}

impl Wire {
	fn encode(self) -> Bytes {
		match self {
			Self::Comment(comment) => Bytes::from(format!(":{comment}\n\n")),
			Self::Event(event) => Bytes::from(format!("event:{}\nid:{}\ndata:{}\n\n", event.kind, event.id, event.payload)),
			Self::Snapshot(value) => Bytes::from(format!("event:snapshot\ndata:{value}\n\n")),
		}
	}
}

struct SseStream {
	rx: mpsc::Receiver<Wire>,
}

impl Stream for SseStream {
	type Item = Result<Bytes, Infallible>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.rx.poll_recv(cx).map(|value| value.map(|wire| Ok(wire.encode())))
	}
}

async fn snapshot(state: &Arc<AppState>, user_id: i64) -> Result<Value, crate::error::ApiError> {
	let manager = state.downloads();
	let scheduler = state.updater();
	let downloads = crate::api::download::download_entries(state, &manager, user_id).await?;
	Ok(json!({
		"type": "snapshot",
		"running": manager.is_running(),
		"downloads": downloads,
		"updateSummary": scheduler.summary().await.visible_to(user_id),
	}))
}

pub async fn events(State(state): State<Arc<AppState>>, req: Request) -> Response {
	let after = req
		.headers()
		.get("last-event-id")
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.parse::<i64>().ok())
		.filter(|id| *id >= 0);
	let user = req.extensions().get::<crate::auth::AuthUser>().expect("auth user").clone();
	let hub = state.events();
	let (tx, rx) = mpsc::channel::<Wire>(64);
	tokio::spawn(async move {
		let mut live = hub.subscribe();
		let mut reset = hub.subscribe_reset();
		let mut last_sent = after.unwrap_or(0);
		if after.is_some() {
			let latest = match hub.latest_id().await {
				Ok(latest) => latest,
				Err(e) => {
					tracing::warn!("event cursor check failed: {e}");
					return;
				}
			};
			if last_sent > latest {
				last_sent = latest;
			} else {
				match hub.after(user.id, last_sent).await {
					Ok(events) => {
						for event in events {
							last_sent = last_sent.max(event.id);
							if event.kind == "resync" {
								continue;
							}
							if tx.send(Wire::Event(event)).await.is_err() {
								return;
							}
						}
					}
					Err(e) => {
						tracing::warn!("event replay failed: {e}");
						return;
					}
				}
			}
		}
		let current = match snapshot(&state, user.id).await {
			Ok(current) => current,
			Err(e) => {
				tracing::warn!("event snapshot failed: {}", e.message());
				return;
			}
		};
		if tx.send(Wire::Snapshot(current)).await.is_err() {
			return;
		}
		let mut heartbeat = interval(HEARTBEAT_INTERVAL);
		heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
		loop {
			tokio::select! {
				_ = reset.recv() => break,
				_ = heartbeat.tick() => {
					if tx.send(Wire::Comment("heartbeat")).await.is_err() {
						break;
					}
				}
				event = live.recv() => {
					match event {
						Ok(event) => {
							if event.user_id.is_some_and(|owner| owner != user.id) || event.id <= last_sent {
								continue;
							}
							last_sent = event.id;
							let wire = if event.kind == "resync" {
								match snapshot(&state, user.id).await {
									Ok(current) => Wire::Snapshot(current),
									Err(e) => {
										tracing::warn!("event snapshot failed: {}", e.message());
										break;
									}
								}
							} else {
								Wire::Event(event)
							};
							if tx.send(wire).await.is_err() {
								break;
							}
						}
						Err(broadcast::error::RecvError::Lagged(_)) => break,
						Err(_) => break,
					}
				}
			}
		}
	});
	Response::builder()
		.header(axum::http::header::CONTENT_TYPE, "text/event-stream")
		.header(axum::http::header::CACHE_CONTROL, "no-cache")
		.header("x-accel-buffering", "no")
		.body(Body::from_stream(SseStream { rx }))
		.unwrap()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn lagged_connection_closes_for_eventsource_reconnect() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		let state = AppState::new(pool, crate::config::Config::default(), defaults_tx, cookie_tx);
		assert!(state.downloads.set(crate::download::DownloadManager::new(&state)).is_ok());
		assert!(state.updater.set(crate::update::UpdateScheduler::new(&state)).is_ok());
		let hub = crate::events::EventHub::new(state.pool.clone());
		assert!(state.events.set(hub.clone()).is_ok());
		let mut request = Request::builder().uri("/api/v1/events").body(Body::empty()).expect("request");
		request.extensions_mut().insert(crate::auth::AuthUser { id: 1, username: "admin".into() });
		let response = events(State(state), request).await;
		assert_eq!(response.status(), axum::http::StatusCode::OK);
		let mut body = std::pin::pin!(response.into_body().into_data_stream());
		let snapshot = tokio::time::timeout(
			Duration::from_secs(2),
			std::future::poll_fn(|cx| body.as_mut().poll_next(cx)),
		)
		.await
		.expect("snapshot timeout")
		.expect("stream ended before snapshot")
		.expect("snapshot body");
		assert!(String::from_utf8_lossy(&snapshot).contains("event:snapshot"));

		for chapter_id in 0..400 {
			hub.record_for_test("download", json!({ "chapterId": chapter_id }), Some(1)).await;
		}
		let closed = tokio::time::timeout(Duration::from_secs(3), async {
			loop {
				match std::future::poll_fn(|cx| body.as_mut().poll_next(cx)).await {
					Some(Ok(_)) => {}
					Some(Err(e)) => panic!("stream error: {e}"),
					None => break,
				}
			}
		})
		.await;
		assert!(closed.is_ok(), "lagged stream stayed open instead of reconnecting");
	}

	#[tokio::test]
	async fn snapshot_failure_closes_without_empty_snapshot() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		let state = AppState::new(pool.clone(), crate::config::Config::default(), defaults_tx, cookie_tx);
		assert!(state.downloads.set(crate::download::DownloadManager::new(&state)).is_ok());
		assert!(state.updater.set(crate::update::UpdateScheduler::new(&state)).is_ok());
		assert!(state.events.set(crate::events::EventHub::new(pool.clone())).is_ok());
		sqlx::query("DROP TABLE download").execute(&pool).await.expect("drop download table");
		let mut request = Request::builder().uri("/api/v1/events").body(Body::empty()).expect("request");
		request.extensions_mut().insert(crate::auth::AuthUser { id: 1, username: "admin".into() });
		let response = events(State(state), request).await;
		assert_eq!(response.status(), axum::http::StatusCode::OK);
		let mut body = std::pin::pin!(response.into_body().into_data_stream());
		let first = tokio::time::timeout(
			Duration::from_secs(2),
			std::future::poll_fn(|cx| body.as_mut().poll_next(cx)),
		)
		.await
		.expect("snapshot timeout");
		assert!(first.is_none(), "a failed snapshot must not become an empty snapshot");
	}

	#[tokio::test]
	async fn journal_failure_closes_for_resync() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let pool = crate::db::create_pool(&format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()))
			.await
			.expect("pool");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		let state = AppState::new(pool.clone(), crate::config::Config::default(), defaults_tx, cookie_tx);
		assert!(state.downloads.set(crate::download::DownloadManager::new(&state)).is_ok());
		assert!(state.updater.set(crate::update::UpdateScheduler::new(&state)).is_ok());
		let hub = crate::events::EventHub::new(pool.clone());
		assert!(state.events.set(hub.clone()).is_ok());
		let mut request = Request::builder().uri("/api/v1/events").body(Body::empty()).expect("request");
		request.extensions_mut().insert(crate::auth::AuthUser { id: 1, username: "admin".into() });
		let response = events(State(state), request).await;
		let mut body = std::pin::pin!(response.into_body().into_data_stream());
		let snapshot = tokio::time::timeout(
			Duration::from_secs(2),
			std::future::poll_fn(|cx| body.as_mut().poll_next(cx)),
		)
		.await
		.expect("snapshot timeout")
		.expect("stream ended before snapshot")
		.expect("snapshot body");
		assert!(String::from_utf8_lossy(&snapshot).contains("event:snapshot"));
		sqlx::query("DROP TABLE app_event").execute(&pool).await.expect("drop event table");
		hub.record_for_test("updateComplete", json!({ "running": false }), Some(1)).await;
		let closed = tokio::time::timeout(Duration::from_secs(2), async {
			loop {
				match std::future::poll_fn(|cx| body.as_mut().poll_next(cx)).await {
					Some(Ok(_)) => {}
					Some(Err(e)) => panic!("stream error: {e}"),
					None => break,
				}
			}
		})
		.await;
		assert!(closed.is_ok(), "journal failure left the stream running without resync");
	}
}
