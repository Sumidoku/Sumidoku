use crate::api::{PageQuery, chapter_json, manga_json, progress_of};
use crate::error::{ApiError, ApiResult};
use crate::model::ChapterUserRow;
use crate::service::manga_row;
use crate::state::AppState;
use crate::PAGE_SIZE;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
	pub category_id: Option<i64>,
}

pub async fn fetch_updates(
	State(state): State<Arc<AppState>>,
	body: Option<Json<FetchRequest>>,
) -> Response {
	let scheduler = state.updater();
	let category_id = body.and_then(|Json(body)| body.category_id);
	if let Some(id) = category_id {
		let exists: Option<(i64,)> =
			match sqlx::query_as("SELECT 1 FROM category WHERE id = ? AND user_id = 1")
				.bind(id)
				.fetch_optional(&state.pool)
				.await
			{
				Ok(row) => row,
				Err(e) => return ApiError::from(e).into_response(),
			};
		if exists.is_none() {
			return ApiError::not_found("category not found").into_response();
		}
	}
	let Some(guard) = scheduler.try_acquire() else {
		return (
			StatusCode::CONFLICT,
			Json(json!({"error": {"code": "busy", "message": "update already running"}})),
		)
			.into_response();
	};
	tokio::spawn(async move {
		scheduler.fetch(category_id, guard).await;
	});
	Json(json!({ "started": true })).into_response()
}

pub async fn get_summary(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	Ok(Json(serde_json::to_value(state.updater().summary().await)?))
}

pub async fn get_recent(
	State(state): State<Arc<AppState>>,
	Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
	let page = query.page.unwrap_or(1);
	if page < 1 {
		return Err(ApiError::bad_request("page must be >= 1"));
	}
	let rows: Vec<ChapterUserRow> = sqlx::query_as(
		"SELECT c.*, cu.last_page_read, cu.is_read, cu.is_bookmarked, cu.last_read_at
		 FROM chapter c
		 JOIN manga_user mu ON mu.manga_id = c.manga_id AND mu.user_id = 1 AND mu.in_library = 1
		 LEFT JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = 1
		 ORDER BY c.fetched_at DESC, c.id DESC LIMIT ? OFFSET ?",
	)
	.bind(PAGE_SIZE + 1)
	.bind((page as i64 - 1) * PAGE_SIZE)
	.fetch_all(&state.pool)
	.await?;
	let has_next_page = rows.len() as i64 > PAGE_SIZE;
	let mut entries = Vec::with_capacity(rows.len().min(PAGE_SIZE as usize));
	for row in rows.iter().take(PAGE_SIZE as usize) {
		let manga = manga_row(&state.pool, row.chapter.manga_id).await?;
		entries.push(json!({
			"chapter": chapter_json(&row.chapter, progress_of(row).as_ref())?,
			"manga": manga_json(&state, &manga).await?,
			"fetchedAt": row.chapter.fetched_at,
		}));
	}
	Ok(Json(json!({
		"entries": entries,
		"hasNextPage": has_next_page,
	})))
}

pub async fn updates_ws(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
	let scheduler = state.updater();
	ws.on_upgrade(move |mut socket: WebSocket| async move {
		let mut snapshot = serde_json::to_value(scheduler.summary().await).unwrap_or_default();
		if let Some(object) = snapshot.as_object_mut() {
			object.insert("type".into(), json!("summary"));
		}
		if socket
			.send(Message::Text(snapshot.to_string().into()))
			.await
			.is_err()
		{
			return;
		}
		let mut rx = scheduler.subscribe();
		loop {
			tokio::select! {
				event = rx.recv() => {
					match event {
						Ok(event) => {
							let text = serde_json::to_string(&event).unwrap_or_default();
							if socket.send(Message::Text(text.into())).await.is_err() {
								return;
							}
						}
						Err(broadcast::error::RecvError::Lagged(_)) => continue,
						Err(_) => return,
					}
				}
				message = socket.recv() => {
					match message {
						Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
						Some(Ok(_)) => {}
					}
				}
			}
		}
	})
}
