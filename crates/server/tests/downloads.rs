mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{
	TestApp, base_config, connect_db, connect_ws, fixture_dir, point_page_at_local, read_ws_frame,
	register_example_manga, request, request_full, serve_app, serve_bytes,
};
use common::PNG;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;

/// 下载测试需要 downloads_dir 落在 tmp（base_config 不管它，默认 ./downloads 会污染仓库）
async fn downloads_app(configure: impl FnOnce(&mut Config)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	configure(&mut config);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn first_chapter_id(app: &Router, manga_id: i64) -> i64 {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("chapter id")
}

async fn wait_download_status(app: &Router, chapter_id: i64, want: &str) -> Value {
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (status, body) = request(app, "GET", "/api/v1/downloads", None).await;
		assert_eq!(status, StatusCode::OK);
		if let Some(entry) = body
			.as_array()
			.and_then(|rows| rows.iter().find(|row| row["chapterId"] == chapter_id))
			.filter(|entry| entry["status"] == want)
		{
			return entry.clone();
		}
		assert!(Instant::now() < deadline, "download status did not become {want}: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

fn assert_cbz(bytes: &[u8], source_key: &str, manga_key: &str, chapter_key: &str, pages: usize) {
	use std::io::Read;
	let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("open cbz");
	assert_eq!(archive.len(), pages + 1);
	{
		let mut info = archive.by_index(0).expect("first entry");
		assert_eq!(info.name().expect("entry name"), "ComicInfo.xml");
		let mut text = String::new();
		info.read_to_string(&mut text).expect("read ComicInfo");
		assert!(text.contains("!SUMIDOKUDATA"), "{text}");
		assert!(text.contains(&format!("\"sourceKey\":\"{source_key}\"")), "{text}");
		assert!(text.contains(&format!("\"mangaKey\":\"{manga_key}\"")), "{text}");
		assert!(text.contains(&format!("\"chapterKey\":\"{chapter_key}\"")), "{text}");
		assert!(text.contains(&format!("<PageCount>{pages}</PageCount>")), "{text}");
	}
	let names: Vec<String> = (1..archive.len())
		.map(|i| archive.by_index(i).expect("entry").name().expect("entry name").into_owned())
		.collect();
	assert!(names.contains(&"000.png".to_string()), "{names:?}");
	assert!(names.contains(&"001.txt".to_string()), "{names:?}");
	{
		let mut image = archive.by_name("000.png").expect("image entry");
		let mut data = Vec::new();
		image.read_to_end(&mut data).expect("read image entry");
		assert_eq!(data, PNG);
	}
	for i in 0..archive.len() {
		let entry = archive.by_index(i).expect("entry");
		let modified = entry.last_modified().expect("entry timestamp");
		assert_eq!(
			(
				modified.year(),
				modified.month(),
				modified.day(),
				modified.hour(),
				modified.minute(),
				modified.second()
			),
			(1980, 1, 1, 0, 0, 0),
			"entry {} timestamp",
			entry.name().expect("entry name")
		);
	}
}

#[tokio::test]
async fn download_full_flow() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let downloads_dir = t.tmp.path().join("downloads");
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");

	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;
	let (manga_key,): (String,) = sqlx::query_as("SELECT key FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("manga key");
	let (chapter_key,): (String,) = sqlx::query_as("SELECT key FROM chapter WHERE id = ?")
		.bind(chapter_id)
		.fetch_one(&pool)
		.await
		.expect("chapter key");

	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	// 重复入队被跳过
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));

	let entry = wait_download_status(&app, chapter_id, "finished").await;
	let rel_path = entry["path"].as_str().expect("download path").to_string();
	let cbz_path = downloads_dir.join(&rel_path);
	assert!(cbz_path.is_file(), "cbz should exist at {}", cbz_path.display());
	assert_eq!(entry["downloaded"], Value::Null);
	assert_eq!(entry["total"], Value::Null);

	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(body[0]["isDownloaded"], true);
	assert_eq!(body[1]["isDownloaded"], false);

	let bytes = std::fs::read(&cbz_path).expect("read cbz");
	assert_cbz(&bytes, "en.example-source", &manga_key, &chapter_key, 2);

	// 删了重下，产物字节完全一致
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!cbz_path.exists());
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(body[0]["isDownloaded"], false);

	let (status, _) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status(&app, chapter_id, "finished").await;
	let bytes_again = std::fs::read(&cbz_path).expect("read cbz again");
	assert_eq!(bytes, bytes_again, "re-download should be byte-identical");

	let (status, headers, file_bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "application/zip");
	assert_eq!(file_bytes, bytes_again);

	let (status, _) = request(&app, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!cbz_path.exists());
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_pause_resume() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let chapter_id = first_chapter_id(&app, manga_id).await;
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let pool = connect_db(&t.db_path).await;
	point_page_at_local(&pool, chapter_id, port).await;

	// 先停再入队，不与 worker 抢跑；再 stop 一次把新入队的 queued 落成 paused
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	let (status, _) = request(&app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body[0]["chapterId"], chapter_id);
	assert_eq!(body[0]["status"], "paused");

	let (status, _) = request(&app, "POST", "/api/v1/downloads/start", None).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status(&app, chapter_id, "finished").await;
	assert!(entry["path"].is_string());
}

async fn wait_update_complete(app: &Router, want_total: i64) {
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (status, body) = request(app, "GET", "/api/v1/update/summary", None).await;
		assert_eq!(status, StatusCode::OK);
		if body["running"] == false
			&& body["total"].as_i64() == Some(want_total)
			&& body["complete"].as_i64() == Some(want_total)
		{
			return;
		}
		assert!(Instant::now() < deadline, "update did not complete: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

#[tokio::test]
async fn update_fetch_reconciles_and_recent() {
	let t = downloads_app(|_| {}).await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let deleted = sqlx::query("DELETE FROM chapter WHERE key = '8'")
		.execute(&pool)
		.await
		.expect("delete chapter")
		.rows_affected();
	assert_eq!(deleted, 1);

	let (status, body) = request(&app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["started"], true);
	wait_update_complete(&app, 1).await;

	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chapter WHERE manga_id = ? AND key = '8'")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("count chapter");
	assert_eq!(count, 1);

	let (status, body) = request(&app, "GET", "/api/v1/update/recent", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["hasNextPage"], false);
	assert_eq!(body["entries"][0]["chapter"]["key"], "8");
	assert_eq!(body["entries"][0]["manga"]["id"], manga_id);

	// 未开 auto_download：download 表为空
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM download")
		.fetch_one(&pool)
		.await
		.expect("count download");
	assert_eq!(count, 0);
}

#[tokio::test]
async fn update_auto_download_enqueues_new_chapters() {
	let t = downloads_app(|config| {
		config.auto_download_new_chapters = true;
	})
	.await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&t.db_path).await;
	let deleted = sqlx::query("DELETE FROM chapter WHERE key = '8'")
		.execute(&pool)
		.await
		.expect("delete chapter")
		.rows_affected();
	assert_eq!(deleted, 1);

	let (status, body) = request(&app, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["started"], true);
	wait_update_complete(&app, 1).await;

	// 新章自动入下载队（worker 后续成败不影响入队事实）
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let row: Option<(String,)> = sqlx::query_as(
			"SELECT d.status FROM download d JOIN chapter c ON c.id = d.chapter_id WHERE c.key = '8'",
		)
		.fetch_optional(&pool)
		.await
		.expect("query download");
		if row.is_some() {
			break;
		}
		assert!(Instant::now() < deadline, "auto download did not enqueue chapter 8");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
}

#[tokio::test]
async fn downloads_ws_pushes_snapshot() {
	let t = downloads_app(|_| {}).await;
	let addr = serve_app(t.app.clone()).await;
	let (mut stream, mut rest) = connect_ws(addr, "/api/v1/downloads/ws", None).await;
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let value: Value = serde_json::from_slice(&payload).expect("snapshot json");
	assert!(value["type"].is_string(), "{value}");
	assert_eq!(value["type"], "snapshot");
	assert!(value["downloads"].is_array(), "{value}");
	assert_eq!(value["running"], true, "{value}");

	// stop → running 事件帧（chapterId=0 占位，running=false）
	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let event: Value = serde_json::from_slice(&payload).expect("running event json");
	assert_eq!(event["type"], "running", "{event}");
	assert_eq!(event["chapterId"], 0);
	assert_eq!(event["downloaded"], 0);
	assert_eq!(event["total"], 0);
	assert_eq!(event["running"], false);

	// start → running=true
	let (status, _) = request(&t.app, "POST", "/api/v1/downloads/start", None).await;
	assert_eq!(status, StatusCode::OK);
	let payload = read_ws_frame(&mut stream, &mut rest).await;
	let event: Value = serde_json::from_slice(&payload).expect("running event json");
	assert_eq!(event["type"], "running", "{event}");
	assert_eq!(event["running"], true);
}
