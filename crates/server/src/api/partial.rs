//! send_partial_result 的 NDJSON 流式下发：客户端用 `Accept: application/x-ndjson`
//! 显式 opt-in，同一请求内 partial 事件按到达顺序写行，终态事件收尾；
//! 不带该 Accept 的请求完全走原有缓冲路径。连接即 key：客户端断开时
//! body 流被丢弃，后续事件发送失败即止，源调用继续跑完、结果丢弃

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

// Value 序列化不会失败；发送失败即客户端断开，返回 false 让转发任务收尾
fn send_line(tx: &mpsc::UnboundedSender<Bytes>, event: &Value) -> bool {
	let mut buf = serde_json::to_vec(event).unwrap_or_default();
	buf.push(b'\n');
	tx.send(Bytes::from(buf)).is_ok()
}

/// 流已经开始、状态码已提交，源调用失败只能以终态 error 事件表达
fn source_error_event(e: SourceError) -> Value {
	let e = ApiError::from(e);
	json!({"type": "error", "error": {"code": e.code(), "message": e.message()}})
}

fn panic_event() -> Value {
	json!({"type": "error", "error": {"code": "source_error", "message": "source call panicked"}})
}

/// 终态事件的 payload 与不带 Accept 的响应体是同一个 JSON 值
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

/// manga 更新（register/refresh）的转发：partial 与搜索同形（完整 Manga 快照，
/// 对齐上游 partialMangaPublisher）；终态 payload 是各端点自己的响应体，
/// 源调用与落库都在 call 里，失败以终态 error 事件表达
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
