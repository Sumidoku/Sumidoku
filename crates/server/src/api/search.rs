//! Cross-source aggregate search: runs the single-source search concurrently for every source in the request; a
//! single source's failure only lands in that source's error entry and never sinks the whole request. With NDJSON,
//! each source emits one source/sourceError event in completion order, and the terminal payload equals the buffered response.
//! Closing the stream aborts all unresolved source tasks instead of leaving a detached queue after the client is gone

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
	loop {
		let joined = match events {
			Some(tx) => tokio::select! {
				joined = tasks.join_next_with_id() => joined,
				_ = tx.closed() => {
					tasks.abort_all();
					while tasks.join_next().await.is_some() {}
					return Vec::new();
				}
			},
			None => tasks.join_next_with_id().await,
		};
		let Some(joined) = joined else {
			break;
		};
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
			// A disconnected client cannot use any more results. Abort the per-request semaphore queue and
			// every unresolved source call instead of leaving an invisible backlog behind a closed stream.
			if !partial::send_line(tx, &event) {
				tasks.abort_all();
				while tasks.join_next().await.is_some() {}
				return Vec::new();
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
			let (entries, has_next) = super::local_search(state, source_id, query.as_deref(), None, page as i64, 20).await?;
			return Ok((entries, has_next, false));
		}
		// opds sources same as the local arm; degraded marks a catalog without OpenSearch, where results are just traversal filtering
		if crate::source::source_is_opds(&state.pool, source_id).await? {
			let outcome = crate::internal::opds_source::search(state, source_id, query.as_deref(), page).await?;
			let entries = super::attach_internal_ids(state, source_id, outcome.entries).await?;
			return Ok((entries, outcome.has_next_page, outcome.degraded));
		}
		let handle = state.get_handle(source_id).await?;
		let result = handle.get_search_manga_list(query, page, Vec::new()).await?;
		#[cfg(test)]
		TEST_FINISHED_SOURCE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

#[cfg(test)]
static TEST_FINISHED_SOURCE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use std::{path::PathBuf, time::Duration};

	async fn test_state(tmp: &tempfile::TempDir) -> (Arc<AppState>, Vec<i64>) {
		let sources_dir = tmp.path().join("sources");
		for (index, key) in ["en.example-a", "en.example-b", "en.example-c"].into_iter().enumerate() {
			let dir = sources_dir.join(key);
			std::fs::create_dir_all(&dir).expect("source directory");
			std::fs::copy(
				PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wasm/tests/fixtures/example/main.wasm"),
				dir.join("main.wasm"),
			)
			.expect("fixture wasm");
			let doc = json!({"info": {"id": key, "name": format!("Example {index}"), "version": 1, "languages": ["en"]}});
			std::fs::write(dir.join("source.json"), doc.to_string()).expect("source manifest");
		}
		let config = crate::config::Config {
			database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
			config_dir: tmp.path().to_path_buf(),
			call_concurrency: 1,
			..crate::config::Config::default()
		};
		let pool = crate::db::create_pool(&config.database_url).await.expect("pool");
		let (defaults_tx, _defaults_rx) = mpsc::unbounded_channel();
		let (cookie_tx, _cookie_rx) = mpsc::unbounded_channel();
		let state = AppState::new(pool, config, defaults_tx, cookie_tx);
		assert!(state.tasks.set(crate::task::TaskManager::new(&state)).is_ok());
		crate::source::scan_sources_dir(&state, &sources_dir).await.expect("scan sources");
		let local = crate::internal::local::register_instance(
			&state,
			"Local",
			&[tmp.path().to_string_lossy().into_owned()],
			None,
			None,
			None,
			None,
			None,
		)
		.await
		.expect("local source");
		let mut ids = vec![local.id];
		let mut remote = sqlx::query_scalar("SELECT id FROM source WHERE kind = 'aidoku' ORDER BY key")
			.fetch_all(&state.pool)
			.await
			.expect("remote ids");
		ids.append(&mut remote);
		(state, ids)
	}

	#[tokio::test]
	async fn disconnect_aborts_sources_still_queued_behind_the_call_limit() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let (state, ids) = test_state(&tmp).await;
		TEST_FINISHED_SOURCE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
		let permit = state.call_semaphore.clone().acquire_owned().await.expect("call permit");
		let (tx, mut rx) = mpsc::unbounded_channel();
		let run = tokio::spawn({
			let state = state.clone();
			let ids = ids.clone();
			async move { run_global_search(&state, &ids, None, 1, Some(&tx)).await }
		});
		let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
			.await
			.expect("first local event timeout")
			.expect("first local event");
		let first: Value = serde_json::from_slice(&first).expect("local event json");
		assert_eq!(first["type"], "source");
		assert_eq!(first["source"]["sourceId"], ids[0]);

		// The local event is already delivered. Dropping the body wakes the disconnect branch immediately;
		// no remote source may start once the single global permit becomes available.
		drop(rx);
		drop(permit);
		let result = tokio::time::timeout(Duration::from_secs(5), run)
			.await
			.expect("aggregate join timeout")
			.expect("aggregate join");
		assert!(result.is_empty(), "a disconnected request has no terminal payload");
		assert_eq!(TEST_FINISHED_SOURCE_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
	}
}
