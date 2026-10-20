//! NDJSON streaming of send_partial_result: the client explicitly opts in with `Accept: application/x-ndjson`;
//! within one request, partial events are written as lines in arrival order and a terminal event closes the stream;
//! requests without that Accept take the original buffered path untouched. The connection is the key: when the
//! client disconnects, the body stream is dropped, later event sends fail and stop — the source call runs to completion and its result is discarded

use crate::error::{ApiError, ApiResult};
use aidoku::{HomeLayout, Manga, MangaPageResult};
use aidoku_wasm::{Error as SourceError, HomePartial};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use futures_core::Stream;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const NDJSON: &str = "application/x-ndjson";

pub fn wants_ndjson(headers: &HeaderMap) -> bool {
	headers
		.get(header::ACCEPT)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|v| v.contains(NDJSON))
}

struct EventStream {
	rx: mpsc::UnboundedReceiver<Bytes>,
}

impl Stream for EventStream {
	type Item = Result<Bytes, Infallible>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.rx.poll_recv(cx).map(|v| v.map(Ok))
	}
}

pub fn ndjson_response(rx: mpsc::UnboundedReceiver<Bytes>) -> Response {
	Response::builder()
		.status(StatusCode::OK)
		.header(header::CONTENT_TYPE, NDJSON)
		.header(header::CACHE_CONTROL, "no-cache")
		.body(Body::from_stream(EventStream { rx }))
		.expect("ndjson response")
}

// Value serialization cannot fail; a send failure means the client disconnected, so return false and let the forward task wind down
pub(crate) fn send_line(tx: &mpsc::UnboundedSender<Bytes>, event: &Value) -> bool {
	let mut buf = serde_json::to_vec(event).unwrap_or_default();
	buf.push(b'\n');
	tx.send(Bytes::from(buf)).is_ok()
}

/// The stream has started and the status code is committed, so a source-call failure can only be expressed as a terminal error event
fn source_error_event(e: SourceError) -> Value {
	let e = ApiError::from(e);
	json!({"type": "error", "error": {"code": e.code(), "message": e.message()}})
}

fn panic_event() -> Value {
	json!({"type": "error", "error": {"code": "source_error", "message": "source call panicked"}})
}

/// The terminal event's payload is the same JSON value as the response body without the Accept header
pub async fn forward_home(
	mut partials: mpsc::UnboundedReceiver<HomePartial>,
	call: JoinHandle<Result<HomeLayout, SourceError>>,
	tx: mpsc::UnboundedSender<Bytes>,
) {
	while let Some(partial) = partials.recv().await {
		let event = match partial {
			HomePartial::Layout(layout) => json!({"type": "layout", "layout": layout}),
			HomePartial::Component { index, component } => {
				json!({"type": "component", "index": index, "component": component})
			}
		};
		if !send_line(&tx, &event) {
			return;
		}
	}
	let event = match call.await {
		Ok(Ok(layout)) => json!({"type": "final", "payload": layout}),
		Ok(Err(e)) => source_error_event(e),
		Err(_) => panic_event(),
	};
	let _ = send_line(&tx, &event);
}

pub async fn forward_search(
	mut partials: mpsc::UnboundedReceiver<Manga>,
	call: JoinHandle<Result<MangaPageResult, SourceError>>,
	tx: mpsc::UnboundedSender<Bytes>,
	source_id: i64,
	known: HashMap<String, i64>,
) {
	while let Some(manga) = partials.recv().await {
		let event = match super::manga_json_with_ids(manga, source_id, &known) {
			Ok(value) => json!({"type": "manga", "manga": value}),
			Err(e) => json!({"type": "error", "error": {"code": e.code(), "message": e.message()}}),
		};
		if !send_line(&tx, &event) {
			return;
		}
	}
	let event = match call.await {
		Ok(Ok(result)) => {
			let entries: ApiResult<Vec<Value>> = result
				.entries
				.into_iter()
				.map(|m| super::manga_json_with_ids(m, source_id, &known))
				.collect();
			match entries {
				Ok(entries) => json!({
					"type": "final",
					"payload": {"entries": entries, "hasNextPage": result.has_next_page},
				}),
				Err(e) => json!({"type": "error", "error": {"code": e.code(), "message": e.message()}}),
			}
		}
		Ok(Err(e)) => source_error_event(e),
		Err(_) => panic_event(),
	};
	let _ = send_line(&tx, &event);
}

/// Forwarding for manga updates (register/refresh): partials have the same shape as search (full Manga snapshots,
/// aligned with upstream's partialMangaPublisher); the terminal payload is each endpoint's own response body.
/// The source call and the DB writes live in call; failures are expressed as a terminal error event
pub async fn forward_manga_update(
	mut partials: mpsc::UnboundedReceiver<Manga>,
	call: JoinHandle<ApiResult<Value>>,
	tx: mpsc::UnboundedSender<Bytes>,
) {
	while let Some(manga) = partials.recv().await {
		if !send_line(&tx, &json!({"type": "manga", "manga": manga})) {
			return;
		}
	}
	let event = match call.await {
		Ok(Ok(payload)) => json!({"type": "final", "payload": payload}),
		Ok(Err(e)) => json!({"type": "error", "error": {"code": e.code(), "message": e.message()}}),
		Err(_) => panic_event(),
	};
	let _ = send_line(&tx, &event);
}
