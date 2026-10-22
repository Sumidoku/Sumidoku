use crate::download::DownloadManager;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRequest {
	pub chapter_ids: Vec<i64>,
}

pub async fn enqueue_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<EnqueueRequest>,
) -> ApiResult<Json<Value>> {
	let queued = state.downloads().enqueue(&body.chapter_ids, user.id).await;
	Ok(Json(json!({ "queued": queued })))
}

#[derive(Debug, sqlx::FromRow)]
struct DownloadRow {
	chapter_id: i64,
	status: String,
	format: String,
	path: Option<String>,
	error: Option<String>,
	retries: i64,
	created_at: i64,
	updated_at: i64,
	manga_id: i64,
	manga_title: String,
	chapter_title: Option<String>,
	chapter_number: Option<f64>,
	volume_number: Option<f64>,
}

async fn download_entries(state: &AppState, manager: &DownloadManager, user_id: i64) -> ApiResult<Vec<Value>> {
	let rows: Vec<DownloadRow> = sqlx::query_as(
		"SELECT d.chapter_id, d.status, d.format, d.path, d.error, d.retries,
		 d.created_at, d.updated_at, m.id AS manga_id, m.title AS manga_title, c.title AS chapter_title,
		 c.number AS chapter_number, c.volume AS volume_number
		 FROM download d
		 JOIN chapter c ON c.id = d.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 WHERE d.user_id = ?
		 ORDER BY CASE d.status
		 WHEN 'queued' THEN 0 WHEN 'downloading' THEN 1 WHEN 'paused' THEN 2
		 WHEN 'error' THEN 3 ELSE 4 END, d.created_at ASC, d.chapter_id ASC",
	)
	.bind(user_id)
	.fetch_all(&state.pool)
	.await?;
	let mut entries = Vec::with_capacity(rows.len());
	for row in rows {
		// downloaded/total of an in-flight download lives only in manager memory; other states get null
		let (downloaded, total) = if row.status == "downloading" {
			match manager.progress_of(row.chapter_id).await {
				Some((done, total)) => (json!(done), json!(total)),
				None => (Value::Null, Value::Null),
			}
		} else {
			(Value::Null, Value::Null)
		};
		entries.push(json!({
			"chapterId": row.chapter_id,
			"mangaId": row.manga_id,
			"mangaTitle": row.manga_title,
			"chapterTitle": row.chapter_title,
			"chapterNumber": row.chapter_number,
			"volumeNumber": row.volume_number,
			"status": row.status,
			"format": row.format,
			"path": row.path,
			"error": row.error,
			"retries": row.retries,
			"downloaded": downloaded,
			"total": total,
			"createdAt": row.created_at,
			"updatedAt": row.updated_at,
		}));
	}
	Ok(entries)
}

pub async fn list_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let manager = state.downloads();
	Ok(Json(json!(download_entries(&state, &manager, user.id).await?)))
}

pub async fn start_downloads(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	state.downloads().set_running(true).await;
	Ok(Json(json!({ "running": true })))
}

pub async fn stop_downloads(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
	state.downloads().set_running(false).await;
	Ok(Json(json!({ "running": false })))
}

pub async fn clear_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let manager = state.downloads();
	let rows: Vec<i64> = sqlx::query_scalar(
		"SELECT chapter_id FROM download WHERE status IN ('finished', 'error') AND user_id = ?",
	)
	.bind(user.id)
	.fetch_all(&state.pool)
	.await?;
	let mut cleared = 0i64;
	for chapter_id in rows {
		if manager.delete(chapter_id, user.id).await? {
			cleared += 1;
		}
	}
	Ok(Json(json!({ "cleared": cleared })))
}

pub async fn delete_download(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	if state.downloads().delete(id, user.id).await? {
		Ok(StatusCode::NO_CONTENT)
	} else {
		Err(ApiError::not_found("download not found"))
	}
}

pub async fn get_chapter_file(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<Response> {
	let manager = state.downloads();
	let row: Option<(String, String, Option<String>)> =
		sqlx::query_as("SELECT status, format, path FROM download WHERE chapter_id = ? AND user_id = ?")
			.bind(id)
			.bind(user.id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((status, format, path)) = row else {
		return Err(ApiError::not_found("download not found"));
	};
	if status != "finished" {
		return Err(ApiError::not_found("chapter file not ready"));
	}
	if format != "cbz" {
		return Ok((
			StatusCode::UNSUPPORTED_MEDIA_TYPE,
			Json(json!({"error": {"code": "unsupported", "message": "download is not a cbz archive"}})),
		)
			.into_response());
	}
	let Some(path) = path else {
		return Err(ApiError::not_found("chapter file not found"));
	};
	let bytes = match tokio::fs::read(crate::download::product_path(manager.downloads_dir(), &path)).await {
		Ok(bytes) => bytes,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			return Err(ApiError::not_found("chapter file not found"));
		}
		Err(e) => return Err(ApiError::from(sqlx::Error::Io(e))),
	};
	Ok(([(header::CONTENT_TYPE, "application/zip")], bytes).into_response())
}

pub async fn downloads_ws(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	ws: WebSocketUpgrade,
) -> Response {
	let manager = state.downloads();
	ws.on_upgrade(move |mut socket: WebSocket| async move {
		let snapshot = match download_entries(&state, &manager, user.id).await {
			Ok(entries) => json!({ "type": "snapshot", "running": manager.is_running(), "downloads": entries }),
			Err(_) => json!({ "type": "snapshot", "running": manager.is_running(), "downloads": [] }),
		};
		if socket
			.send(Message::Text(snapshot.to_string().into()))
			.await
			.is_err()
		{
			return;
		}
		let mut rx = manager.subscribe();
		loop {
			tokio::select! {
				event = rx.recv() => {
					match event {
						Ok(event) => {
							// Row events go only to their owning user; running is global manager state, broadcast to all
							if event.user_id.is_some_and(|owner| owner != user.id) {
								continue;
							}
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
