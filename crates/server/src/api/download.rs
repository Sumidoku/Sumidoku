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
}

async fn download_entries(state: &AppState, manager: &DownloadManager, user_id: i64) -> ApiResult<Vec<Value>> {
	let rows: Vec<DownloadRow> = sqlx::query_as(
		"SELECT d.chapter_id, d.status, d.format, d.path, d.error, d.retries,
		 d.created_at, d.updated_at, m.id AS manga_id, m.title AS manga_title, c.title AS chapter_title
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
		// 进行中的 downloaded/total 只在 manager 内存里，其余状态给 null
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

/// 产物可能是 cbz 文件也可能是 folder 目录
async fn remove_product(downloads_dir: &std::path::Path, rel_path: &str) {
	let target = downloads_dir.join(rel_path);
	if target.is_dir() {
		let _ = tokio::fs::remove_dir_all(&target).await;
	} else {
		let _ = tokio::fs::remove_file(&target).await;
	}
}

/// 另一用户还引用同一产物路径时文件留着（产物共享，删行不删别人的货）
async fn product_orphaned(pool: &sqlx::SqlitePool, path: &str, user_id: i64) -> ApiResult<bool> {
	let referenced: Option<(i64,)> =
		sqlx::query_as("SELECT 1 FROM download WHERE path = ? AND user_id != ? LIMIT 1")
			.bind(path)
			.bind(user_id)
			.fetch_optional(pool)
			.await?;
	Ok(referenced.is_none())
}

/// 同一章还有别家的在途行时，tmp 目录归那个任务管，不能清
async fn chapter_in_flight(pool: &sqlx::SqlitePool, chapter_id: i64, user_id: i64) -> ApiResult<bool> {
	let active: Option<(i64,)> = sqlx::query_as(
		"SELECT 1 FROM download WHERE chapter_id = ? AND user_id != ? AND status IN ('queued', 'downloading') LIMIT 1",
	)
	.bind(chapter_id)
	.bind(user_id)
	.fetch_optional(pool)
	.await?;
	Ok(active.is_some())
}

pub async fn clear_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	let manager = state.downloads();
	let rows: Vec<(i64, Option<String>)> =
		sqlx::query_as("SELECT chapter_id, path FROM download WHERE status IN ('finished', 'error') AND user_id = ?")
			.bind(user.id)
			.fetch_all(&state.pool)
			.await?;
	let mut cleared = 0i64;
	for (chapter_id, path) in rows {
		if let Some(path) = path
			&& product_orphaned(&state.pool, &path, user.id).await?
		{
			remove_product(manager.downloads_dir(), &path).await;
		}
		if !chapter_in_flight(&state.pool, chapter_id, user.id).await? {
			manager.cleanup_tmp(chapter_id).await;
		}
		sqlx::query("DELETE FROM download WHERE chapter_id = ? AND user_id = ?")
			.bind(chapter_id)
			.bind(user.id)
			.execute(&state.pool)
			.await?;
		sqlx::query(
			"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 0)
			 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 0",
		)
		.bind(chapter_id)
		.bind(user.id)
		.execute(&state.pool)
		.await?;
		cleared += 1;
	}
	Ok(Json(json!({ "cleared": cleared })))
}

pub async fn delete_download(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(chapter_id): Path<i64>,
) -> ApiResult<StatusCode> {
	let manager = state.downloads();
	let row: Option<(String, Option<String>)> =
		sqlx::query_as("SELECT status, path FROM download WHERE chapter_id = ? AND user_id = ?")
			.bind(chapter_id)
			.bind(user.id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((status, path)) = row else {
		return Err(ApiError::not_found("download not found"));
	};
	// 在跑的先标记取消，worker 在下一页前看到就停
	if status == "downloading" {
		manager.mark_cancel(chapter_id).await;
	}
	manager.remove_from_queue(chapter_id, user.id).await;
	if let Some(path) = path
		&& product_orphaned(&state.pool, &path, user.id).await?
	{
		remove_product(manager.downloads_dir(), &path).await;
	}
	if !chapter_in_flight(&state.pool, chapter_id, user.id).await? {
		manager.cleanup_tmp(chapter_id).await;
	}
	sqlx::query("DELETE FROM download WHERE chapter_id = ? AND user_id = ?")
		.bind(chapter_id)
		.bind(user.id)
		.execute(&state.pool)
		.await?;
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_downloaded) VALUES (?, ?, 0)
		 ON CONFLICT (chapter_id, user_id) DO UPDATE SET is_downloaded = 0",
	)
	.bind(chapter_id)
	.bind(user.id)
	.execute(&state.pool)
	.await?;
	Ok(StatusCode::NO_CONTENT)
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
	let bytes = match tokio::fs::read(manager.downloads_dir().join(&path)).await {
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
							// 行事件只发行属用户；running 是全局管理器状态，全员广播
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
