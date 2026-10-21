//! Two-user isolation: A (admin) and B (reader) share the content cache, while user-state data is
//! mutually invisible and non-polluting.
//! The second user is inserted via direct SQL: isolation semantics stay decoupled from the user
//! management endpoints, so each evolves independently

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{base_config, connect_db, fixture_dir, request, request_with_headers, source_by_key};
use serde_json::{Value, json};
use sumidoku_server::build_app;

const ADMIN_PW: &str = "secret";
const BOB_PW: &str = "hunter2";

struct TwoUsers {
	app: Router,
	#[allow(dead_code)]
	tmp: tempfile::TempDir,
	admin: String,
	bob: String,
}

async fn two_user_app() -> TwoUsers {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.auth.password = Some(ADMIN_PW.into());
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let pool = connect_db(&db_path).await;
	let hash = sumidoku_server::auth::hash_password(BOB_PW).expect("hash password");
	sqlx::query("INSERT INTO user (username, password_hash, role) VALUES ('bob', ?, 'reader')")
		.bind(&hash)
		.execute(&pool)
		.await
		.expect("insert bob");
	pool.close().await;
	let admin = login(&app, "admin", ADMIN_PW).await;
	let bob = login(&app, "bob", BOB_PW).await;
	TwoUsers {
		app,
		tmp,
		admin,
		bob,
	}
}

async fn login(app: &Router, username: &str, password: &str) -> String {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": username, "password": password})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	format!("Bearer {}", body["token"].as_str().expect("token"))
}

async fn as_user(app: &Router, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(app, method, path, body, &[("authorization", token)]).await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

/// Installs the source via scan + registers the first search result, returning the internal manga id (with the requesting user throughout)
async fn register_manga_as(app: &Router, token: &str) -> i64 {
	let (status, _) = as_user(app, token, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_user(app, token, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (status, body) = as_user(
		app,
		token,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = as_user(
		app,
		token,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	body["id"].as_i64().expect("manga id")
}

async fn first_chapter_as(app: &Router, token: &str, manga_id: i64) -> i64 {
	let (status, body) = as_user(app, token, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("chapter id")
}

#[tokio::test]
async fn user_state_isolated_between_users() {
	let users = two_user_app().await;
	let app = &users.app;
	let (admin, bob) = (&users.admin, &users.bob);

	// A accumulates user state: library + category + progress + incognito + download row
	let manga_id = register_manga_as(app, admin).await;
	let chapter_id = first_chapter_as(app, admin, manga_id).await;
	let (status, body) = as_user(app, admin, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::OK);
	let admin_cat = body["id"].as_i64().expect("category id");
	let (status, _) = as_user(
		app,
		admin,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [admin_cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		app,
		admin,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 5, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(app, admin, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_user(
		app,
		admin,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"].as_array().expect("queued").len(), 1);

	// B looking at A's user state: empty sets or 404
	let (_, body) = as_user(app, bob, "GET", "/api/v1/library", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/library?categoryId={admin_cat}"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/categories", None).await;
	assert_eq!(body.as_array().expect("categories").len(), 0);
	let (_, body) = as_user(app, bob, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(body["inLibrary"], false);
	assert_eq!(body["incognito"], false);
	assert_eq!(body["categoryIds"].as_array().expect("categoryIds").len(), 0);
	let (_, body) = as_user(app, bob, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["lastPageRead"], 0);
	assert_eq!(body[0]["isRead"], false);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/history", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 0);
	let (status, _) = as_user(app, bob, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/settings", None).await;
	assert!(body.get("incognito.enabled").is_none());
	let (status, _) = as_user(
		app,
		bob,
		"PATCH",
		&format!("/api/v1/categories/{admin_cat}"),
		Some(json!({"name": "hijack"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = as_user(app, bob, "DELETE", &format!("/api/v1/categories/{admin_cat}"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	// to B the manga is merely "not shelved": same 409 shape as single-user semantics, without exposing A's shelved state
	let (status, _) = as_user(
		app,
		bob,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": [admin_cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT);
	// A's library entries are invisible to B's update trigger: B triggering an update by A's category likewise 404s
	let (status, _) = as_user(
		app,
		bob,
		"POST",
		"/api/v1/update/fetch",
		Some(json!({"categoryId": admin_cat})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// B's same-named category and own user state: doesn't pollute A
	let (status, body) = as_user(app, bob, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::OK);
	let bob_cat = body["id"].as_i64().expect("bob category id");
	assert_ne!(bob_cat, admin_cat);
	let (status, _) = as_user(
		app,
		bob,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [bob_cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	// once shelved, B referencing A's category id: treated the same as a nonexistent category, 404
	let (status, _) = as_user(
		app,
		bob,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": [admin_cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = as_user(
		app,
		bob,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 2})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(app, bob, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": false}))).await;
	assert_eq!(status, StatusCode::OK);
	// both users hold a download row for the same chapter (composite primary key)
	let (status, body) = as_user(
		app,
		bob,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"].as_array().expect("queued").len(), 1);

	// re-checking A's view: everything as before
	let (_, body) = as_user(app, admin, "GET", "/api/v1/library", None).await;
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["id"], manga_id);
	let (_, body) = as_user(app, admin, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(body["inLibrary"], true);
	assert_eq!(body["categoryIds"], json!([admin_cat]));
	let (_, body) = as_user(app, admin, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["lastPageRead"], 5);
	assert_eq!(body[0]["isRead"], true);
	let (_, body) = as_user(app, admin, "GET", "/api/v1/downloads", None).await;
	let rows = body.as_array().expect("downloads");
	assert_eq!(rows.len(), 1);
	assert_eq!(rows[0]["chapterId"], chapter_id);
	let (_, body) = as_user(app, admin, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["incognito.enabled"], true);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["incognito.enabled"], false);

	// B's own view has only his own row
	let (_, body) = as_user(app, bob, "GET", "/api/v1/library", None).await;
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["categoryIds"], json!([bob_cat]));
	let (_, body) = as_user(app, bob, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 1);
}

/// B's backup export contains only his own user state; A's categories and progress don't enter B's document
#[tokio::test]
async fn backup_export_scoped_to_requesting_user() {
	let users = two_user_app().await;
	let app = &users.app;
	let (admin, bob) = (&users.admin, &users.bob);

	let manga_id = register_manga_as(app, admin).await;
	let chapter_id = first_chapter_as(app, admin, manga_id).await;
	let (status, _) = as_user(app, admin, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(app, admin, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(
		app,
		admin,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 5, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(app, admin, "PUT", "/api/v1/settings", Some(json!({"incognito.enabled": true}))).await;
	assert_eq!(status, StatusCode::OK);

	let (status, doc) = as_user(app, bob, "GET", "/api/v1/backup", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(doc["library"].as_array().expect("library").len(), 0);
	assert_eq!(doc["categories"].as_array().expect("categories").len(), 0);
	assert_eq!(doc["progress"].as_array().expect("progress").len(), 0);
	assert!(doc["settings"].get("incognito.enabled").is_none());

	let (_, body) = as_user(app, admin, "GET", "/api/v1/backup", None).await;
	assert_eq!(body["library"].as_array().expect("library").len(), 1);
	assert_eq!(body["categories"].as_array().expect("categories").len(), 1);
	assert_eq!(body["progress"].as_array().expect("progress").len(), 1);
	assert_eq!(body["settings"]["incognito.enabled"], true);
}

// ---------- 2b: background workers per user ----------

use common::{PNG, connect_ws, point_page_at_local, read_ws_frame, read_ws_kind, serve_app, serve_bytes};
use std::time::{Duration, Instant};

/// Registers the manga, warms the page cache and points page 0 at the local image host, returning (manga_id, chapter_id)
async fn prepared_chapter(users: &TwoUsers, port: u16) -> (i64, i64) {
	let app = &users.app;
	let manga_id = register_manga_as(app, &users.admin).await;
	let chapter_id = first_chapter_as(app, &users.admin, manga_id).await;
	let (status, _) = as_user(app, &users.admin, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&users.tmp.path().join("test.db")).await;
	point_page_at_local(&pool, chapter_id, port).await;
	pool.close().await;
	(manga_id, chapter_id)
}

async fn wait_download_status_as(app: &Router, token: &str, chapter_id: i64, want: &str) -> Value {
	let deadline = Instant::now() + Duration::from_secs(15);
	loop {
		let (status, body) = as_user(app, token, "GET", "/api/v1/downloads", None).await;
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

/// Delayed image host: keeps the worker in the in-flight state during the page fetch, leaving a deterministic window for in-flight cancel
async fn serve_slow_png(delay: Duration) -> (u16, tokio::task::JoinHandle<()>) {
	let handler = move || async move {
		tokio::time::sleep(delay).await;
		([(axum::http::header::CONTENT_TYPE, "image/png".to_string())], PNG.to_vec())
	};
	let router = Router::new().route("/img.png", axum::routing::get(handler));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, handle)
}

#[tokio::test]
async fn download_shared_artifact_between_users() {
	let users = two_user_app().await;
	let app = &users.app;
	let (admin, bob) = (&users.admin, &users.bob);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let (manga_id, chapter_id) = prepared_chapter(&users, port).await;

	// A finishes via the worker, artifact on disk
	let (status, body) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	let entry = wait_download_status_as(app, admin, chapter_id, "finished").await;
	let path = entry["path"].as_str().expect("download path").to_string();
	let artifact = users.tmp.path().join("downloads").join(&path);
	let bytes_a = std::fs::read(&artifact).expect("artifact bytes");

	// B enqueues the same chapter: the artifact already exists, lands finished directly, no repacking
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
	let entry = wait_download_status_as(app, bob, chapter_id, "finished").await;
	assert_eq!(entry["path"].as_str().expect("download path"), path);
	assert_eq!(std::fs::read(&artifact).expect("artifact bytes"), bytes_a);

	// the download mark belongs to B (the worker's completion mark is per entry user), and B can fetch the same artifact
	let (_, body) = as_user(app, bob, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["isDownloaded"], true);
	let (status, _, file_bytes) =
		request_with_headers(app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None, &[("authorization", bob)]).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(file_bytes, bytes_a);

	// A deletes his own row: B still references the same artifact, the file stays
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);

	// B deletes too: no references left, the artifact is removed
	let (status, _) = as_user(app, bob, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The move_path target is the shared content area: both rows render the same absolute target, and deletion only clears it once nothing references it
#[tokio::test]
async fn download_moved_artifact_shared_between_users() {
	let users = two_user_app().await;
	let app = &users.app;
	let (admin, bob) = (&users.admin, &users.bob);
	let move_root = users.tmp.path().join("library");
	let template = format!("{}/{{manga}}/{{filename}}", move_root.display());
	let (status, _) = as_user(
		app,
		admin,
		"PUT",
		"/api/v1/settings",
		Some(json!({"download.move_path": template})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let (_manga_id, chapter_id) = prepared_chapter(&users, port).await;

	// A finishes and moves to the shared content area, the row stores the absolute path
	let (status, _) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status_as(app, admin, chapter_id, "finished").await;
	let path = entry["path"].as_str().expect("download path").to_string();
	assert!(std::path::Path::new(&path).is_absolute(), "{path}");
	let artifact = std::path::PathBuf::from(&path);
	assert!(artifact.is_file());

	// B enqueues the same chapter: the moved artifact hits the share via the absolute path, both rows point at the same file
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
	let entry = wait_download_status_as(app, bob, chapter_id, "finished").await;
	assert_eq!(entry["path"].as_str().expect("download path"), path);

	// A deletes his own row: B still references the same absolute path, the file stays, B fetches as usual
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);

	// B deletes too: no references left, the shared file is removed
	let (status, _) = as_user(app, bob, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!artifact.exists());
}

#[tokio::test]
async fn download_cancel_only_removes_own_row() {
	let users = two_user_app().await;
	let app = &users.app;
	let (admin, bob) = (&users.admin, &users.bob);
	let (port, _upstream) = serve_slow_png(Duration::from_secs(2)).await;
	let (manga_id, chapter_id) = prepared_chapter(&users, port).await;

	// A's task starts first; with no artifact yet, B queues normally
	let (status, _) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status_as(app, admin, chapter_id, "downloading").await;
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));

	// A cancels in flight: only A's row is withdrawn, B's row stays queued
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = as_user(app, admin, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 0);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 1);

	// B's entry runs the full task to finished; the download mark belongs to B without affecting A's view
	let entry = wait_download_status_as(app, bob, chapter_id, "finished").await;
	assert!(entry["path"].is_string());
	let (_, body) = as_user(app, bob, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["isDownloaded"], true);
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = as_user(app, admin, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["isDownloaded"], false);
}

#[tokio::test]
async fn downloads_ws_events_filtered_per_user() {
	let users = two_user_app().await;
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	let (_manga_id, chapter_id) = prepared_chapter(&users, port).await;
	let addr = serve_app(users.app.clone()).await;
	let (mut admin_ws, mut admin_buf) = connect_ws(addr, "/api/v1/downloads/ws", Some(&users.admin)).await;
	let (mut bob_ws, mut bob_buf) = connect_ws(addr, "/api/v1/downloads/ws", Some(&users.bob)).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut admin_ws, &mut admin_buf).await).expect("snapshot");
	assert_eq!(snapshot["type"], "snapshot");
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut bob_ws, &mut bob_buf).await).expect("snapshot");
	assert_eq!(snapshot["type"], "snapshot");

	// A enqueues: row events go only into A's connection
	let (status, _) = as_user(
		&users.app,
		&users.admin,
		"POST",
		"/api/v1/downloads",
		Some(json!({"chapterIds": [chapter_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut admin_ws, &mut admin_buf, "queued").await;
	assert_eq!(event["chapterId"], chapter_id);
	let leaked = tokio::time::timeout(Duration::from_millis(300), read_ws_frame(&mut bob_ws, &mut bob_buf)).await;
	assert!(leaked.is_err(), "bob must not see admin's download events");

	// global start/stop is manager state: both connections receive running events
	let (status, _) = as_user(&users.app, &users.admin, "POST", "/api/v1/downloads/stop", None).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut admin_ws, &mut admin_buf, "running").await;
	assert_eq!(event["running"], false);
	let event = read_ws_kind(&mut bob_ws, &mut bob_buf, "running").await;
	assert_eq!(event["running"], false);
}

#[tokio::test]
async fn updates_ws_events_filtered_per_user() {
	let users = two_user_app().await;
	let manga_id = register_manga_as(&users.app, &users.admin).await;
	let (status, _) = as_user(
		&users.app,
		&users.admin,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let addr = serve_app(users.app.clone()).await;
	let (mut admin_ws, mut admin_buf) = connect_ws(addr, "/api/v1/update/ws", Some(&users.admin)).await;
	let (mut bob_ws, mut bob_buf) = connect_ws(addr, "/api/v1/update/ws", Some(&users.bob)).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut admin_ws, &mut admin_buf).await).expect("snapshot");
	assert_eq!(snapshot["type"], "summary");
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut bob_ws, &mut bob_buf).await).expect("snapshot");
	assert_eq!(snapshot["type"], "summary");

	// admin triggers fetch manually: job events go only into admin's connection
	let (status, _) = as_user(
		&users.app,
		&users.admin,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.skip_titles": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&users.app, &users.admin, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut admin_ws, &mut admin_buf, "job").await;
	assert_eq!(event["mangaId"], manga_id);
	let leaked = tokio::time::timeout(Duration::from_millis(500), read_ws_frame(&mut bob_ws, &mut bob_buf)).await;
	assert!(leaked.is_err(), "bob must not see admin's update events");

	// wait for the fetch to settle
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (_, body) = as_user(&users.app, &users.admin, "GET", "/api/v1/update/summary", None).await;
		if body["running"] == false {
			break;
		}
		assert!(Instant::now() < deadline, "update did not finish: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}

	// the fetch belongs to admin: bob's fresh-connection snapshot renders as idle, admin's carries real counts
	let (mut bob_ws2, mut bob_buf2) = connect_ws(addr, "/api/v1/update/ws", Some(&users.bob)).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut bob_ws2, &mut bob_buf2).await).expect("snapshot");
	assert_eq!(snapshot["running"], false);
	assert_eq!(snapshot["total"], 0);
	let (mut admin_ws2, mut admin_buf2) = connect_ws(addr, "/api/v1/update/ws", Some(&users.admin)).await;
	let snapshot: Value = serde_json::from_slice(&read_ws_frame(&mut admin_ws2, &mut admin_buf2).await).expect("snapshot");
	assert_eq!(snapshot["total"], 1);
	assert_eq!(snapshot["complete"], 1);
}

#[tokio::test]
async fn update_summary_rest_scoped_per_user() {
	let users = two_user_app().await;
	let manga_id = register_manga_as(&users.app, &users.admin).await;
	let (status, _) = as_user(
		&users.app,
		&users.admin,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// admin triggers fetch manually and waits for it to settle: the counts belonging to admin are invisible to bob.
	// the fetch is spawned, so polling before running flips true reads the previous snapshot; just wait until the counts land
	let (status, _) = as_user(
		&users.app,
		&users.admin,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.skip_titles": []})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = as_user(&users.app, &users.admin, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let deadline = Instant::now() + Duration::from_secs(10);
	let summary = loop {
		let (_, body) = as_user(&users.app, &users.admin, "GET", "/api/v1/update/summary", None).await;
		if body["running"] == false && body["total"] == 1 {
			break body;
		}
		assert!(Instant::now() < deadline, "update did not finish: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	};
	assert_eq!(summary["complete"], 1);

	let (_, body) = as_user(&users.app, &users.bob, "GET", "/api/v1/update/summary", None).await;
	assert_eq!(body["running"], false);
	assert_eq!(body["total"], 0);
	assert_eq!(body["lastRunAt"], Value::Null);
}
