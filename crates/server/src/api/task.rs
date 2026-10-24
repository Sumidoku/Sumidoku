use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::task::{STATUS_CANCELLED, STATUS_DONE, STATUS_FAILED, STATUS_PENDING, STATUS_RUNNING};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

const STATUSES: [&str; 5] = [STATUS_PENDING, STATUS_RUNNING, STATUS_DONE, STATUS_FAILED, STATUS_CANCELLED];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksQuery {
	/// Comma-separated status filter (pending,running,done,failed,cancelled); absent = every task
	pub status: Option<String>,
	pub kind: Option<String>,
	pub source_id: Option<i64>,
}

pub async fn list_tasks(
	State(state): State<Arc<AppState>>,
	Query(query): Query<TasksQuery>,
) -> ApiResult<Json<Value>> {
	let statuses = query
		.status
		.as_deref()
		.map(|raw| raw.split(',').map(|status| status.trim().to_string()).collect::<Vec<_>>());
	if let Some(statuses) = &statuses {
		for status in statuses {
			if !STATUSES.contains(&status.as_str()) {
				return Err(ApiError::bad_request(format!(
					"status must be one of {}; got {status}",
					STATUSES.join(",")
				)));
			}
		}
	}
	let tasks = state
		.tasks()
		.list(statuses.as_deref(), query.kind.as_deref(), query.source_id)
		.await?;
	let tasks: Vec<Value> = tasks.iter().map(|task| task.to_json()).collect();
	Ok(Json(json!(tasks)))
}

pub async fn cancel_task(
	State(state): State<Arc<AppState>>,
	Path(id): Path<String>,
) -> ApiResult<StatusCode> {
	state.tasks().cancel(&id).await?;
	Ok(StatusCode::NO_CONTENT)
}
