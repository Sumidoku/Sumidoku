//! 跨源聚合搜索：对请求里的每个源并发执行单源搜索，单源失败只落该源的
//! error 项，不拖垮整请求。NDJSON 时按完成序每源发一个 source/sourceError
//! 事件，终态 payload 与缓冲响应同值

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

/// 每请求的源搜索并发上限；全局准入由实例共享的 call_semaphore 背压，这里只限单请求扇出
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
	// 重复 id 只搜一次，顺序按首现归位；客户端筛选态异常时不至于对同一源发双倍调用
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

/// 聚合执行：每源一个任务，完成一个发一个事件（events 为 None 即纯缓冲路径）；
/// 返回值按请求顺序归位，与事件的完成序无关
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
				// panic 同样只落该源的 error 项；id 在 spawn 时已登记，必然查得到
				let index = task_index[&e.id()];
				(index, error_entry(ids[index], "source_error", "source call panicked"))
			}
		};
		if let Some(tx) = events {
			let event = match entry.get("error") {
				Some(error) => json!({"type": "sourceError", "sourceId": ids[index], "error": error}),
				None => json!({"type": "source", "source": entry}),
			};
			// 客户端断开即止发；任务照常跑完，结果丢弃与既有流式端点同一约定
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

/// 单源搜索的执行与落形：handle 解析、源调用、id 附加任何一步失败都归成该源的
/// error 项；成功形状与非流式单源搜索响应一致，外加 sourceId
async fn search_one(state: &Arc<AppState>, source_id: i64, query: Option<String>, page: i32) -> Value {
	let outcome: ApiResult<(Vec<Value>, bool)> = async {
		let handle = state.get_handle(source_id).await?;
		let result = handle.get_search_manga_list(query, page, Vec::new()).await?;
		let entries = super::attach_internal_ids(state, source_id, result.entries).await?;
		Ok((entries, result.has_next_page))
	}
	.await;
	match outcome {
		Ok((entries, has_next_page)) => json!({
			"sourceId": source_id,
			"entries": entries,
			"hasNextPage": has_next_page,
		}),
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
