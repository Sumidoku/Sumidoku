//! Cross-source aggregate search: runs the single-source search concurrently for every source in the request; a
//! single source's failure only lands in that source's error entry and never sinks the whole request. With NDJSON,
//! each source emits one source/sourceError event in completion order, and the terminal payload equals the buffered response

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::partial;

/// Per-request concurrency cap for source searches; global admission is back-pressured by the instance-shared call_semaphore, this only limits one request's fan-out
const GLOBAL_SEARCH_CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSearchRequest {
	pub query: Option<String>,
	pub page: Option<i32>,
	pub source_ids: Vec<i64>,
}

pub async fn search_all(
	State(state): State<Arc<AppState>>,
	headers: HeaderMap,
	Json(body): Json<GlobalSearchRequest>,
) -> ApiResult<Response> {
	let page = body.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	// A duplicated id is searched only once, ordered by first appearance; a broken client filter state must not fire double calls at the same source
	let mut ids: Vec<i64> = Vec::with_capacity(body.source_ids.len());
	for id in body.source_ids {
		if !ids.contains(&id) {
			ids.push(id);
		}
	}
	if !partial::wants_ndjson(&headers) {
		let sources = run_global_search(&state, &ids, body.query, page, None).await;
		return Ok(Json(json!({ "sources": sources })).into_response());
	}
	let (line_tx, line_rx) = mpsc::unbounded_channel();
	tokio::spawn(async move {
		let sources = run_global_search(&state, &ids, body.query, page, Some(&line_tx)).await;
		let _ = partial::send_line(&line_tx, &json!({"type": "final", "payload": {"sources": sources}}));
	});
	Ok(partial::ndjson_response(line_rx))
}

/// Aggregate execution: one task per source, one event per completion (events = None is the pure buffered path);
/// the return values land back in request order, independent of the events' completion order
pub(crate) async fn run_global_search(
	state: &Arc<AppState>,
	ids: &[i64],
	query: Option<String>,
	page: i32,
	events: Option<&mpsc::UnboundedSender<Bytes>>,
) -> Vec<Value> {
	let semaphore = Arc::new(Semaphore::new(GLOBAL_SEARCH_CONCURRENCY));
	let mut tasks = JoinSet::new();
	let mut task_index: HashMap<tokio::task::Id, usize> = HashMap::new();
	for (index, &source_id) in ids.iter().enumerate() {
		let state = state.clone();
		let semaphore = semaphore.clone();
		let query = query.clone();
		let handle = tasks.spawn(async move {
			let _permit = semaphore.acquire_owned().await.expect("semaphore open");
			(index, search_one(&state, source_id, query, page).await)
		});
		task_index.insert(handle.id(), index);
	}
	let mut results: Vec<Option<Value>> = ids.iter().map(|_| None).collect();
	let mut events = events;
	while let Some(joined) = tasks.join_next_with_id().await {
		let (index, entry) = match joined {
			Ok((_, pair)) => pair,
			Err(e) => {
				// A panic likewise only lands in that source's error entry; the id was registered at spawn time and is necessarily found
				let index = task_index[&e.id()];
				(index, error_entry(ids[index], "source_error", "source call panicked"))
			}
		};
		if let Some(tx) = events {
			let event = match entry.get("error") {
				Some(error) => json!({"type": "sourceError", "sourceId": ids[index], "error": error}),
				None => json!({"type": "source", "source": entry}),
			};
			// Stop sending once the client disconnects; tasks still run to completion and results are discarded, same convention as the existing streaming endpoints
			if !partial::send_line(tx, &event) {
				events = None;
			}
		}
		results[index] = Some(entry);
	}
	results
		.into_iter()
		.map(|entry| entry.expect("all search tasks joined"))
		.collect()
}

/// Execution and shaping of a single-source search: any failure in handle resolution, the source call, or id
/// attaching is folded into that source's error entry; the success shape matches the non-streaming single-source search response, plus sourceId
async fn search_one(state: &Arc<AppState>, source_id: i64, query: Option<String>, page: i32) -> Value {
	let outcome: ApiResult<(Vec<Value>, bool, bool)> = async {
		// The local source has no wasm instance; scan the directory on the same arm as the single-source endpoint, without degrading the aggregate into an error entry
		if crate::source::source_is_local(&state.pool, source_id).await? {
			let entries = super::local_search(state, source_id, query.as_deref()).await?;
			return Ok((entries, false, false));
		}
		// opds sources same as the local arm; degraded marks a catalog without OpenSearch, where results are just traversal filtering
		if crate::source::source_is_opds(&state.pool, source_id).await? {
			let outcome = crate::internal::opds_source::search(state, source_id, query.as_deref()).await?;
			let entries = super::attach_internal_ids(state, source_id, outcome.entries).await?;
			return Ok((entries, false, outcome.degraded));
		}
		let handle = state.get_handle(source_id).await?;
		let result = handle.get_search_manga_list(query, page, Vec::new()).await?;
		let entries = super::attach_internal_ids(state, source_id, result.entries).await?;
		Ok((entries, result.has_next_page, false))
	}
	.await;
	match outcome {
		Ok((entries, has_next_page, degraded)) => {
			let mut value = json!({
				"sourceId": source_id,
				"entries": entries,
				"hasNextPage": has_next_page,
			});
			// degraded appears only on the opds traversal fallback; don't add a new field to the wasm/local arms' results
			if degraded {
				value["degraded"] = json!(true);
			}
			value
		}
		Err(e) => error_entry(source_id, e.code(), e.message()),
	}
}

fn error_entry(source_id: i64, code: &str, message: &str) -> Value {
	json!({
		"sourceId": source_id,
		"entries": [],
		"hasNextPage": false,
		"error": {"code": code, "message": message},
	})
}
