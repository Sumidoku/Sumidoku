//! 双用户隔离：A（admin）与 B（reader）共享内容缓存，用户态数据互不可见、互不污染。
//! 第二个用户直接 SQL 落库——用户管理端点属于后续切片，测试不依赖它

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
