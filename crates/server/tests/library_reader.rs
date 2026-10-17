mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, base_config, connect_db, register_example_manga, request, request_full, test_app};
use serde_json::json;
use sumidoku_server::build_app;

// 1x1 透明 PNG
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[tokio::test]
async fn library_category_progress_history() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let manga_id = register_example_manga(&app).await;

	// 入库
	let (status, body) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["id"], manga_id);
	assert_eq!(body["inLibrary"], true);

	// 库列表
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("library entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["id"], manga_id);
	assert_eq!(entries[0]["inLibrary"], true);
	assert!(entries[0]["inLibraryAt"].is_i64());
	assert_eq!(body["hasNextPage"], false);

	// 未入库即未分类
	let (status, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);

	// 分类
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_a = body["id"].as_i64().expect("category id");
	assert_eq!(body["sortOrder"], 0);
	assert_eq!(body["mangaCount"], 0);
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Action"}))).await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "duplicate");
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Drama"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_b = body["id"].as_i64().expect("category id");
	assert_eq!(body["sortOrder"], 1);

	// 挂进分类
	let manga_categories = format!("/api/v1/manga/{manga_id}/categories");
	let (status, _) = request(&app, "PUT", &manga_categories, Some(json!({"categoryIds": [cat_a]}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", &format!("/api/v1/library?categoryId={cat_a}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);
	let (_, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);

	// 分类列表带 mangaCount
	let (status, body) = request(&app, "GET", "/api/v1/categories", None).await;
	assert_eq!(status, StatusCode::OK);
	let cats = body.as_array().expect("categories");
	assert_eq!(cats.len(), 2);
	assert_eq!(cats[0]["name"], "Action");
	assert_eq!(cats[0]["mangaCount"], 1);
	assert_eq!(cats[1]["mangaCount"], 0);

	// 重排
	let (status, body) = request(
		&app,
		"PATCH",
		"/api/v1/categories/reorder",
		Some(json!({"ids": [cat_b, cat_a]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let cats = body.as_array().expect("reordered categories");
	assert_eq!(cats[0]["id"], cat_b);
	assert_eq!(cats[0]["sortOrder"], 0);
	assert_eq!(cats[1]["id"], cat_a);
	assert_eq!(cats[1]["sortOrder"], 1);

	// 改名
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/categories/{cat_b}"),
		Some(json!({"name": "Romance"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["name"], "Romance");

	// 章节带默认用户字段
	let chapters_path = format!("/api/v1/manga/{manga_id}/chapters");
	let (status, body) = request(&app, "GET", &chapters_path, None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters");
	assert_eq!(chapters.len(), 8);
	assert_eq!(chapters[0]["lastPageRead"], 0);
	assert_eq!(chapters[0]["isRead"], false);
	assert_eq!(chapters[0]["isBookmarked"], false);
	assert!(chapters[0]["lastReadAt"].is_null());
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");

	// 进度
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 3})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["chapterId"], chapter_id);
	assert_eq!(body["lastPageRead"], 3);
	assert_eq!(body["isRead"], false);
	assert_eq!(body["isBookmarked"], false);
	assert!(body["lastReadAt"].is_i64());

	// 章节回读用户字段：读过的有值，没读过的默认
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	let chapters = body.as_array().expect("chapters");
	assert_eq!(chapters[0]["lastPageRead"], 3);
	assert!(chapters[0]["lastReadAt"].is_i64());
	assert_eq!(chapters[1]["lastPageRead"], 0);
	assert!(chapters[1]["lastReadAt"].is_null());

	// 历史
	let (status, body) = request(&app, "GET", "/api/v1/history", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("history entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["chapter"]["id"], chapter_id);
	assert_eq!(entries[0]["chapter"]["lastPageRead"], 3);
	assert_eq!(entries[0]["manga"]["id"], manga_id);
	assert_eq!(entries[0]["lastPageRead"], 3);
	assert_eq!(entries[0]["isRead"], false);
	assert!(entries[0]["lastReadAt"].is_i64());
	assert_eq!(body["hasNextPage"], false);

	// 批量已读
	let ids: Vec<i64> = chapters.iter().map(|c| c["id"].as_i64().expect("id")).collect();
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/chapter/batch",
		Some(json!({"chapterIds": ids, "isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["updated"], 8);
	let (_, body) = request(&app, "GET", &chapters_path, None).await;
	assert!(
		body.as_array()
			.expect("chapters")
			.iter()
			.all(|c| c["isRead"] == true)
	);

	// 删分类：category_manga 级联清掉，漫画回落未分类
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/categories/{cat_a}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library?categoryId=0", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 1);

	// 出库
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(body["entries"].as_array().expect("entries").len(), 0);
	let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(body["inLibrary"], false);
	let (status, body) = request(&app, "PUT", &manga_categories, Some(json!({"categoryIds": []}))).await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "not_in_library");

	// category_manga 行也清掉
	let pool = connect_db(&db_path).await;
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM category_manga WHERE manga_id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("count category_manga");
	assert_eq!(count, 0);
}

/// 起个微型静态服务返字节；返回端口与句柄（abort 即关站，模拟上游消失）
async fn serve_bytes(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> (u16, tokio::task::JoinHandle<()>) {
	let mut router = Router::new();
	for (path, mime, bytes) in routes {
		let handler = move || {
			let mime = mime.to_string();
			let bytes = bytes.clone();
			async move { ([(axum::http::header::CONTENT_TYPE, mime)], bytes) }
		};
		router = router.route(path, get(handler));
	}
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, handle)
}

async fn update_page_content(pool: &sqlx::SqlitePool, chapter_id: i64, idx: i64, content: serde_json::Value) {
	let stored = json!({
		"content": content,
		"thumbnail": null,
		"hasDescription": false,
		"description": null,
	});
	sqlx::query("UPDATE page SET content = ? WHERE chapter_id = ? AND idx = ?")
		.bind(stored.to_string())
		.bind(chapter_id)
		.bind(idx)
		.execute(pool)
		.await
		.expect("update page content");
}

#[tokio::test]
async fn cover_and_page_proxy() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let pool = connect_db(&db_path).await;

	// 封面：上游 PNG → 200 落缓存
	let (port, upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	sqlx::query("UPDATE manga SET cover_url = ? WHERE id = ?")
		.bind(format!("http://127.0.0.1:{port}/img.png"))
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update cover url");
	let cover_path = format!("/api/v1/manga/{manga_id}/cover");
	let (status, headers, bytes) = request_full(&app, "GET", &cover_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=86400");
	assert_eq!(bytes, PNG);

	// 上游消失后仍 200：走缓存
	upstream.abort();
	let (status, _, bytes) = request_full(&app, "GET", &cover_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, PNG);

	// ?refresh 绕过缓存 → 上游已死 502
	let (status, body) = request(&app, "GET", &format!("{cover_path}?refresh"), None).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);
	assert_eq!(body["error"]["code"], "bad_gateway");

	// 页图代理：先把 pages 落缓存
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);

	// Url 页
	let (port, _pages_upstream) = serve_bytes(vec![
		("/img.png", "image/png", PNG.to_vec()),
		("/img.zip", "application/zip", make_zip("page/1.jpg", PNG)),
	])
	.await;
	update_page_content(
		&pool,
		chapter_id,
		0,
		json!({"Url": [format!("http://127.0.0.1:{port}/img.png"), null]}),
	)
	.await;
	let page_path = format!("/api/v1/chapter/{chapter_id}/page/0");
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(headers["cache-control"], "private, max-age=3600");
	assert_eq!(bytes, PNG);

	// Text 页
	update_page_content(&pool, chapter_id, 0, json!({"Text": "# Hi"})).await;
	let (status, body) = request(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!({"type": "text", "text": "# Hi"}));

	// Zip 页：内存 zip 解出条目
	update_page_content(
		&pool,
		chapter_id,
		0,
		json!({"Zip": [format!("http://127.0.0.1:{port}/img.zip"), "page/1.jpg"]}),
	)
	.await;
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/jpeg");
	assert_eq!(bytes, PNG);

	// Image 页：M4 前 501
	update_page_content(&pool, chapter_id, 0, json!({"Image": {}})).await;
	let (status, body) = request(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], "unsupported");

	// 越界 404
	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/99"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

fn make_zip(entry_path: &str, data: &[u8]) -> Vec<u8> {
	use std::io::Write;
	let mut cursor = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut cursor);
		writer
			.start_file(entry_path, zip::write::SimpleFileOptions::default())
			.expect("start file");
		writer.write_all(data).expect("write entry");
		writer.finish().expect("finish zip");
	}
	cursor.into_inner()
}

#[tokio::test]
async fn static_webui() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let webui = tmp.path().join("webui");
	std::fs::create_dir_all(webui.join("assets")).expect("mkdir assets");
	std::fs::write(webui.join("index.html"), "<html><body>sumidoku-webui</body></html>").expect("index");
	std::fs::write(webui.join("assets/app.js"), "console.log(1)").expect("asset");
	let mut config = base_config(&tmp);
	config.webui_dir = webui;
	let app = build_app(config).await.expect("build app");

	let (status, _, bytes) = request_full(&app, "GET", "/", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(String::from_utf8(bytes).expect("utf8").contains("sumidoku-webui"));

	let (status, _, bytes) = request_full(&app, "GET", "/any/spa/route", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(String::from_utf8(bytes).expect("utf8").contains("sumidoku-webui"));

	let (status, _, bytes) = request_full(&app, "GET", "/assets/app.js", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"console.log(1)");

	// API 不被 SPA 吞掉
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!([]));
	let (status, body) = request(&app, "GET", "/api/v1/nope", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");
}
