use crate::download::DownloadManager;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRequest {
	#[serde(default)]
	pub chapter_ids: Vec<i64>,
	#[serde(default)]
	pub manga_ids: Vec<i64>,
}

/// Whole-manga selection expands to the manga's untrashed chapters of remote sources: local
/// chapters are already on disk, so queueing them would be meaningless (same exclusion as
/// reader-ahead). Explicit chapter ids are passed through untouched.
async fn expand_manga_chapters(pool: &sqlx::SqlitePool, manga_ids: &[i64]) -> ApiResult<Vec<i64>> {
	let mut chapter_ids = Vec::new();
	for chunk in manga_ids.chunks(crate::api::MANGA_ID_QUERY_CHUNK) {
		let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let sql = format!(
			"SELECT c.id FROM chapter c JOIN manga m ON m.id = c.manga_id JOIN source s ON s.id = m.source_id
			 WHERE c.manga_id IN ({placeholders}) AND c.trashed_at IS NULL AND s.kind != ?"
		);
		let mut query = sqlx::query_scalar::<_, i64>(&sql);
		for manga_id in chunk {
			query = query.bind(manga_id);
		}
		chapter_ids.extend(query.bind(crate::model::SOURCE_KIND_LOCAL).fetch_all(pool).await?);
	}
	Ok(chapter_ids)
}

pub async fn enqueue_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<EnqueueRequest>,
) -> ApiResult<Json<Value>> {
	let expanded = expand_manga_chapters(&state.pool, &body.manga_ids).await?;
	// the manager deduplicates within the list and against active rows
	let chapter_ids: Vec<i64> = body.chapter_ids.iter().chain(expanded.iter()).copied().collect();
	let queued = state.downloads().enqueue(&chapter_ids, user.id).await;
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

pub(crate) async fn download_entries(state: &AppState, manager: &DownloadManager, user_id: i64) -> ApiResult<Vec<Value>> {
	let rows: Vec<DownloadRow> = sqlx::query_as(
		"SELECT d.chapter_id, d.status, d.format, d.path, d.error, d.retries,
		 d.created_at, d.updated_at, m.id AS manga_id, m.title AS manga_title, c.title AS chapter_title,
		 c.number AS chapter_number, c.volume AS volume_number
		 FROM download d
		 JOIN chapter c ON c.id = d.chapter_id
		 JOIN manga m ON m.id = c.manga_id
		 WHERE d.user_id = ? AND d.dismissed_at IS NULL
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
			"chapterNumber": row.chapter_number.map(super::f32_json),
			"volumeNumber": row.volume_number.map(super::f32_json),
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
	let mut tx = crate::db::begin_write(&state.pool)
	.await?;
	let cleared = sqlx::query(
        "UPDATE download SET dismissed_at = ?, updated_at = ?
		 WHERE user_id = ? AND status IN ('finished', 'error') AND dismissed_at IS NULL",
	).bind(crate::model::unix_now())
    .bind(crate::model::unix_now())
	.bind( user.id).execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
	Ok(Json(json!({ "cleared": cleared })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDownloadsRequest {
    #[serde(default)]
    pub chapter_ids: Vec<i64>,
    #[serde(default)]
	pub manga_ids: Vec<i64>,
    #[serde(default)]
    pub finished_only: bool,
}

pub async fn delete_downloads(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<DeleteDownloadsRequest>,
) -> ApiResult<Json<Value>> {
	if body.manga_ids.is_empty() && body.chapter_ids.is_empty() {
		return Ok(Json(json!({ "deleted": 0 })));
	}
	let mut chapter_ids = Vec::new();
	for chunk in body.chapter_ids.chunks(crate::api::MANGA_ID_QUERY_CHUNK) {
		let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let sql = format!(
			"SELECT chapter_id FROM download WHERE user_id = ? AND chapter_id IN ({placeholders})"
		);
		let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(user.id);
		for chapter_id in chunk {
			query = query.bind(chapter_id);
		}
		chapter_ids.extend(query.fetch_all(&state.pool).await?);
	}
	for chunk in body.manga_ids.chunks(crate::api::MANGA_ID_QUERY_CHUNK) {
		let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
		let sql = format!(
			"SELECT d.chapter_id FROM download d JOIN chapter c ON c.id = d.chapter_id
			 WHERE d.user_id = ? AND c.manga_id IN ({placeholders})"
		);
		let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(user.id);
		for manga_id in chunk {
			query = query.bind(manga_id);
		}
		chapter_ids.extend(query.fetch_all(&state.pool).await?);
	}
    chapter_ids.sort_unstable();
    chapter_ids.dedup();
	let manager = state.downloads();
	let mut deleted = 0i64;
	for chapter_id in chapter_ids {
		if manager.delete_guarded(chapter_id, user.id, body.finished_only).await? {
			deleted += 1;
		}
	}
	Ok(Json(json!({ "deleted": deleted })))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, download::DownloadManager, state::AppState};
    use std::sync::Arc;

    async fn app() -> (Arc<AppState>, i64, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = Config {
            config_dir: tmp.path().to_path_buf(),
            database_url: format!("sqlite://{}/test.db?mode=rwc", tmp.path().display()),
            downloads_dir: tmp.path().join("downloads"),
            covers_dir: tmp.path().join("covers"),
            ..Config::default()
        };
        let pool = crate::db::create_pool(&config.database_url)
            .await
            .expect("pool");
        let source: i64 = sqlx::query_scalar(
            "INSERT INTO source(key,name,wasm_path) VALUES ('source','Source','') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .expect("source");
        let manga: i64 = sqlx::query_scalar(
            "INSERT INTO manga(source_id,key,title) VALUES (?, 'manga','Manga') RETURNING id",
        )
        .bind(source)
        .fetch_one(&pool)
        .await
        .expect("manga");
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapter(manga_id,key,page_count) VALUES (?, 'chapter',1) RETURNING id",
        )
        .bind(manga)
        .fetch_one(&pool)
        .await
        .expect("chapter");
        let (defaults_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (cookie_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState::new(pool, config, defaults_tx, cookie_tx);
        assert!(state.downloads.set(DownloadManager::new(&state)).is_ok());
        (state, chapter, tmp)
    }

    #[tokio::test]
    async fn clearing_hides_finished_download_but_keeps_offline_product_and_explicit_delete() {
        let (state, chapter, _tmp) = app().await;
        let product = state.config.downloads_dir.join("Source/Manga/chapter.cbz");
        std::fs::create_dir_all(product.parent().expect("parent")).expect("mkdir");
        let file = std::fs::File::create(&product).expect("file");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("000.txt", zip::write::SimpleFileOptions::default())
            .expect("entry");
        use std::io::Write;
        zip.write_all(b"offline page").expect("page");
        zip.finish().expect("zip");
        sqlx::query("INSERT INTO download(chapter_id,user_id,status,format,path) VALUES (?,1,'finished','cbz','Source/Manga/chapter.cbz')").bind(chapter).execute(&state.pool).await.expect("download");
        sqlx::query("INSERT INTO chapter_user(chapter_id,user_id,is_downloaded) VALUES (?,1,1)")
            .bind(chapter)
            .execute(&state.pool)
            .await
            .expect("flag");
        let result = clear_downloads(State(state.clone()), Extension(state.auth.admin_user()))
            .await
            .expect("clear")
            .0;
        assert_eq!(result["cleared"], 1);
        let hidden: i64 =
            sqlx::query_scalar("SELECT dismissed_at IS NOT NULL FROM download WHERE chapter_id=?")
                .bind(chapter)
                .fetch_one(&state.pool)
                .await
                .expect("dismissed row");
        assert_eq!(hidden, 1);
        assert!(
            download_entries(&state, &state.downloads(), 1)
                .await
                .expect("entries")
                .is_empty()
        );
        assert!(
            crate::download::pages_from_product(&state, chapter)
                .await
                .expect("offline pages")
                .is_some()
        );
        assert!(product.exists(), "clear must retain the CBZ artifact");
        let exported = get_chapter_file(
            State(state.clone()),
            Extension(state.auth.admin_user()),
            Path(chapter),
        )
        .await
        .expect("CBZ export");
        assert_eq!(exported.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(exported.into_body(), usize::MAX).await.expect("export body");
        assert!(bytes.starts_with(b"PK"), "export remains a readable ZIP artifact");
        let repeated = clear_downloads(State(state.clone()), Extension(state.auth.admin_user()))
            .await.expect("repeat clear").0;
        assert_eq!(repeated["cleared"], 0);
        let flag: i64 = sqlx::query_scalar(
            "SELECT is_downloaded FROM chapter_user WHERE chapter_id=? AND user_id=1",
        )
        .bind(chapter)
        .fetch_one(&state.pool)
        .await
        .expect("flag");
        assert_eq!(flag, 1);
        let deleted = delete_downloads(
            State(state.clone()),
            Extension(state.auth.admin_user()),
            Json(DeleteDownloadsRequest { chapter_ids: vec![chapter], manga_ids: Vec::new(), finished_only: true }),
        ).await.expect("explicit delete").0;
        assert_eq!(deleted["deleted"], 1, "a dismissed finished row remains removable");
        assert!(!product.exists());
    }

    #[tokio::test]
    async fn finished_only_batch_delete_deduplicates_and_skips_changed_states() {
        let (state, finished, _tmp) = app().await;
        let manga: i64 = sqlx::query_scalar("SELECT manga_id FROM chapter WHERE id=?").bind(finished).fetch_one(&state.pool).await.expect("manga");
        let queued: i64 = sqlx::query_scalar("INSERT INTO chapter(manga_id,key) VALUES (?, 'queued') RETURNING id").bind(manga).fetch_one(&state.pool).await.expect("queued");
        let active: i64 = sqlx::query_scalar("INSERT INTO chapter(manga_id,key) VALUES (?, 'active') RETURNING id").bind(manga).fetch_one(&state.pool).await.expect("active");
        let paused: i64 = sqlx::query_scalar("INSERT INTO chapter(manga_id,key) VALUES (?, 'paused') RETURNING id").bind(manga).fetch_one(&state.pool).await.expect("paused");
        let error: i64 = sqlx::query_scalar("INSERT INTO chapter(manga_id,key) VALUES (?, 'error') RETURNING id").bind(manga).fetch_one(&state.pool).await.expect("error");
        for (id, status) in [(finished,"finished"),(queued,"queued"),(active,"downloading"),(paused,"paused"),(error,"error")] {
            sqlx::query("INSERT INTO download(chapter_id,user_id,status,dismissed_at) VALUES (?,1,?,CASE WHEN ?='finished' THEN 1 ELSE NULL END)")
                .bind(id).bind(status).bind(status).execute(&state.pool).await.expect("row");
        }
        let body = Json(DeleteDownloadsRequest { chapter_ids: vec![finished, queued, active, paused, error, finished, i64::MAX], manga_ids: Vec::new(), finished_only: true });
        let deleted = delete_downloads(State(state.clone()), Extension(state.auth.admin_user()), body).await.expect("delete").0;
        assert_eq!(deleted["deleted"], 1);
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM download WHERE chapter_id IN (?,?,?,?)")
            .bind(queued).bind(active).bind(paused).bind(error).fetch_one(&state.pool).await.expect("remaining");
        assert_eq!(remaining, 4, "queued, downloading, paused and error rows survive");
        let repeated = delete_downloads(
            State(state.clone()), Extension(state.auth.admin_user()),
            Json(DeleteDownloadsRequest { chapter_ids: vec![finished], manga_ids: Vec::new(), finished_only: true }),
        ).await.expect("repeat delete").0;
        assert_eq!(repeated["deleted"], 0);
    }
}
