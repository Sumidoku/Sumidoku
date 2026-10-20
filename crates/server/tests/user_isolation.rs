//! 双用户隔离：A（admin）与 B（reader）共享内容缓存，用户态数据互不可见、互不污染。
//! 第二个用户直接 SQL 落库：隔离语义与用户管理端点解耦，各自演化互不影响

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

/// scan 装源 + 搜索注册第一条结果，返回内部 manga id（全程带请求用户）
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

	// A 攒下用户态：库 + 分类 + 进度 + 隐身 + 下载行
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

	// B 看 A 的用户态：空集或 404
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
	// 漫画对 B 只是「未入库」：与单用户语义同形 409，不暴露 A 的在库状态
	let (status, _) = as_user(
		app,
		bob,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/categories"),
		Some(json!({"categoryIds": [admin_cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT);
	// A 的库内条目对 B 的更新触发不可见：B 按 A 的分类触发更新同样 404
	let (status, _) = as_user(
		app,
		bob,
		"POST",
		"/api/v1/update/fetch",
		Some(json!({"categoryId": admin_cat})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// B 的同名分类与自留用户态：不污染 A
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
	// 入库后的 B 引用 A 的分类 id：与分类不存在同按 404
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
	// 同一章节两人各持下载行（复合主键）
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

	// 回头核对 A 的视图：一切如旧
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

	// B 自己的视图里只有自己的一行
	let (_, body) = as_user(app, bob, "GET", "/api/v1/library", None).await;
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["categoryIds"], json!([bob_cat]));
	let (_, body) = as_user(app, bob, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 1);
}

/// B 导出备份只含自己的用户态；A 的分类与进度不进 B 的文档
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

// ---------- 2b：后台 worker 按用户 ----------

use common::{PNG, connect_ws, point_page_at_local, read_ws_frame, read_ws_kind, serve_app, serve_bytes};
use std::time::{Duration, Instant};

/// 注册漫画、预热页缓存并把第 0 页指向本地图床，返回 (manga_id, chapter_id)
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

/// 延时图床：worker 在页抓取期间停在在途态，给在途取消留确定性窗口
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

	// A 走 worker 下完，产物落盘
	let (status, body) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));
	let entry = wait_download_status_as(app, admin, chapter_id, "finished").await;
	let path = entry["path"].as_str().expect("download path").to_string();
	let artifact = users.tmp.path().join("downloads").join(&path);
	let bytes_a = std::fs::read(&artifact).expect("artifact bytes");

	// B 入队同一章：产物已在，直接落 finished，不重复打包
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
	let entry = wait_download_status_as(app, bob, chapter_id, "finished").await;
	assert_eq!(entry["path"].as_str().expect("download path"), path);
	assert_eq!(std::fs::read(&artifact).expect("artifact bytes"), bytes_a);

	// 下载标记归 B（worker 的完成标记按条目用户），B 能取到同一份产物
	let (_, body) = as_user(app, bob, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(body[0]["isDownloaded"], true);
	let (status, _, file_bytes) =
		request_with_headers(app, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None, &[("authorization", bob)]).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(file_bytes, bytes_a);

	// A 删自己的行：B 还引用同一产物，文件留着
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);

	// B 也删：没人引用了，产物清掉
	let (status, _) = as_user(app, bob, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(!artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// move_path 目标是共享内容区：两行渲染同一绝对目标，删除只清无人引用的一刻
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

	// A 下完并移动到共享内容区，行存绝对路径
	let (status, _) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	let entry = wait_download_status_as(app, admin, chapter_id, "finished").await;
	let path = entry["path"].as_str().expect("download path").to_string();
	assert!(std::path::Path::new(&path).is_absolute(), "{path}");
	let artifact = std::path::PathBuf::from(&path);
	assert!(artifact.is_file());

	// B 入队同一章：移动后的产物经绝对路径命中共享，两行指向同一文件
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([]));
	let entry = wait_download_status_as(app, bob, chapter_id, "finished").await;
	assert_eq!(entry["path"].as_str().expect("download path"), path);

	// A 删自己的行：B 还引用同一绝对路径，文件留着，B 照常能取
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(artifact.exists());
	let (status, _) = as_user(app, bob, "GET", &format!("/api/v1/chapter/{chapter_id}/file"), None).await;
	assert_eq!(status, StatusCode::OK);

	// B 也删：没人引用了，共享文件清掉
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

	// A 的任务先跑起来；产物未出，B 正常排队
	let (status, _) = as_user(app, admin, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	wait_download_status_as(app, admin, chapter_id, "downloading").await;
	let (status, body) = as_user(app, bob, "POST", "/api/v1/downloads", Some(json!({"chapterIds": [chapter_id]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["queued"], json!([chapter_id]));

	// A 在途取消：只退 A 的行，B 的行原样在队
	let (status, _) = as_user(app, admin, "DELETE", &format!("/api/v1/downloads/{chapter_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = as_user(app, admin, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 0);
	let (_, body) = as_user(app, bob, "GET", "/api/v1/downloads", None).await;
	assert_eq!(body.as_array().expect("downloads").len(), 1);

	// B 的条目走完整任务落 finished，下载标记归 B 不殃及 A 的视图
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

	// A 入队：行事件只进 A 的连接
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

	// 全局 start/stop 是管理器状态：两条连接都收 running 事件
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

	// admin 手动触发 fetch：job 事件只进 admin 的连接
	let (status, _) = as_user(&users.app, &users.admin, "POST", "/api/v1/update/fetch", Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	let event = read_ws_kind(&mut admin_ws, &mut admin_buf, "job").await;
	assert_eq!(event["mangaId"], manga_id);
	let leaked = tokio::time::timeout(Duration::from_millis(500), read_ws_frame(&mut bob_ws, &mut bob_buf)).await;
	assert!(leaked.is_err(), "bob must not see admin's update events");

	// 等 fetch 收尾
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let (_, body) = as_user(&users.app, &users.admin, "GET", "/api/v1/update/summary", None).await;
		if body["running"] == false {
			break;
		}
		assert!(Instant::now() < deadline, "update did not finish: {body}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}

	// fetch 归属 admin：bob 新连进来的快照按闲置呈现，admin 的带真实计数
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

	// admin 手动触发 fetch 并等它收尾：归属 admin 的计数对 bob 不可见。
	// fetch 是 spawn 出去的，running 翻真之前轮询会读到上一次的快照，直接等到计数落地
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
