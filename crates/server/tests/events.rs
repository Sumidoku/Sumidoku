mod common;

use axum::http::StatusCode;
use common::{
	connect_db, connect_sse, point_page_at_local, read_sse_kind, register_example_manga, request, serve_app, serve_bytes,
	test_app,
};
use common::PNG;
use serde_json::json;

#[tokio::test]
async fn event_replay_is_bounded_and_deduplicated() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&app.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let addr = serve_app(app.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_kind(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["data"]["type"], "snapshot");

	let (status, _) = request(&app.app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let queued = read_sse_kind(&mut stream, &mut rest, "download").await;
	assert_eq!(queued["data"]["type"], "queued");
	let started = read_sse_kind(&mut stream, &mut rest, "download").await;
	assert_eq!(started["data"]["type"], "started");
	let first = read_sse_kind(&mut stream, &mut rest, "download").await;
	assert_eq!(first["data"]["type"], "progress");
	let second = read_sse_kind(&mut stream, &mut rest, "download").await;
	assert_eq!(second["data"]["type"], "progress");
	let finished = read_sse_kind(&mut stream, &mut rest, "download").await;
	assert_eq!(finished["data"]["type"], "finished");
	let ids = [queued["id"].as_i64().unwrap(), started["id"].as_i64().unwrap(), first["id"].as_i64().unwrap(), second["id"].as_i64().unwrap(), finished["id"].as_i64().unwrap()];
	assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");

	let after = started["id"].as_i64().unwrap();
	let (mut replay, mut replay_rest) = connect_sse(addr, "/api/v1/events", Some(after), &[]).await;
	let first_replay = read_sse_kind(&mut replay, &mut replay_rest, "download").await;
	assert_eq!(first_replay["id"], first["id"]);
	let second_replay = read_sse_kind(&mut replay, &mut replay_rest, "download").await;
	assert_eq!(second_replay["id"], second["id"]);
	let finished_replay = read_sse_kind(&mut replay, &mut replay_rest, "download").await;
	assert_eq!(finished_replay["id"], finished["id"]);
	let replay_snapshot = read_sse_kind(&mut replay, &mut replay_rest, "snapshot").await;
	assert_eq!(replay_snapshot["data"]["type"], "snapshot");
	let (status, _) = request(&app.app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let running = common::read_sse_event(&mut replay, &mut replay_rest).await;
	assert_eq!(running["type"], "running");
	assert_eq!(running["data"]["running"], false);
	assert!(running["id"].as_i64().unwrap() > finished["id"].as_i64().unwrap());

	let pool = connect_db(&app.db_path).await;
	let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_event").fetch_one(&pool).await.expect("event count");
	assert!(count >= 5, "{count}");
	sqlx::query("UPDATE app_event SET created_at = 0").execute(&pool).await.expect("age events");
	let (mut old, mut old_rest) = connect_sse(addr, "/api/v1/events", Some(0), &[]).await;
	let old_snapshot = read_sse_kind(&mut old, &mut old_rest, "snapshot").await;
	assert_eq!(old_snapshot["data"]["type"], "snapshot");
}

#[tokio::test]
async fn update_complete_carries_final_summary() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let (status, _) = request(&app.app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let addr = serve_app(app.app.clone()).await;
	let (mut stream, mut rest) = connect_sse(addr, "/api/v1/events", None, &[]).await;
	let snapshot = read_sse_kind(&mut stream, &mut rest, "snapshot").await;
	assert_eq!(snapshot["data"]["type"], "snapshot");
	let (status, _) = request(&app.app, "PUT", "/api/v1/settings", Some(json!({"library.skip_titles": []}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&app.app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let started = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(started["type"], "updateSummary");
	assert_eq!(started["data"]["running"], true);
	assert_eq!(started["data"]["pending"], 1);
	let pending = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(pending["type"], "update");
	assert_eq!(pending["data"]["status"], "pending");
	let running = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(running["data"]["status"], "running");
	let progress = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(progress["type"], "updateSummary");
	assert_eq!(progress["data"]["running"], true);
	assert_eq!(progress["data"]["complete"], 1);
	assert_eq!(progress["data"]["pending"], 0);
	let finished_job = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(finished_job["type"], "update");
	assert_eq!(finished_job["data"]["status"], "complete");
	let complete = common::read_sse_event(&mut stream, &mut rest).await;
	assert_eq!(complete["type"], "updateComplete");
	assert_eq!(complete["data"]["running"], false);
	assert_eq!(complete["data"]["total"], 1);
	assert_eq!(complete["data"]["complete"], 1);
	let frames = [started, pending, running, progress, finished_job, complete];
	let ids = frames.map(|frame| frame["id"].as_i64().unwrap());
	assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");
}

#[tokio::test]
async fn future_cursor_resets_to_snapshot_and_live_events() {
	let app = test_app().await;
	let addr = serve_app(app.app.clone()).await;
	let (mut stream, mut rest) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		connect_sse(addr, "/api/v1/events", Some(i64::MAX - 1), &[]),
	)
	.await
	.expect("connect sse");
	let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), read_sse_kind(&mut stream, &mut rest, "snapshot"))
		.await
		.expect("initial snapshot");
	assert_eq!(snapshot["data"]["type"], "snapshot");
	let (status, _) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(&app.app, "POST", "/api/v1/downloads/stop", None),
	)
	.await
	.expect("stop downloads");
	assert_eq!(status, StatusCode::OK);
	let running = tokio::time::timeout(std::time::Duration::from_secs(5), common::read_sse_event(&mut stream, &mut rest))
		.await
		.expect("running event");
	assert_eq!(running["type"], "running");
	assert_eq!(running["data"]["running"], false);
}
