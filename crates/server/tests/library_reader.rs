mod common;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use common::{
	TestApp, base_config, connect_db, fixture_dir, install_fixture_source, register_example_manga,
	request, request_full, source_by_key, test_app,
};
use serde_json::{Value, json};
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
	let TestApp { app, tmp, db_path } = test_app().await;
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
	// example fixture 带 process_cover_image 导出，封面恒被处理成 200x300 PNG
	let cover = image::ImageReader::new(std::io::Cursor::new(&bytes))
		.with_guessed_format()
		.expect("cover format")
		.decode()
		.expect("cover decode");
	assert_eq!((cover.width(), cover.height()), (200, 300));

	// 上游消失后仍 200：走缓存
	upstream.abort();
	let (status, _, cached) = request_full(&app, "GET", &cover_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(cached, bytes);

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

	// Image 页：物化 blob 存在时正常出图；引用不存在的 blob 报 404
	update_page_content(&pool, chapter_id, 0, json!({"Image": "0.png"})).await;
	let blob_dir = tmp.path().join("page-blobs").join(chapter_id.to_string());
	std::fs::create_dir_all(&blob_dir).expect("mkdir blob dir");
	std::fs::write(blob_dir.join("0.png"), PNG).expect("write blob");
	let (status, headers, bytes) = request_full(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, PNG);

	update_page_content(&pool, chapter_id, 0, json!({"Image": "9.png"})).await;
	let (status, body) = request(&app, "GET", &page_path, None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

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

	// API 不被 SPA 吞掉；内置本地源随迁移落库，空库也有一条
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("sources array").len(), 1);
	assert_eq!(body[0]["key"], "local.local");
	assert_eq!(body[0]["kind"], "local");
	let (status, body) = request(&app, "GET", "/api/v1/nope", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");

	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn category_include_flags_patch() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Flags"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat = body["id"].as_i64().expect("category id");
	assert_eq!(body["includeInUpdate"], 0);
	assert_eq!(body["includeInDownload"], 0);
	let cat_path = format!("/api/v1/categories/{cat}");

	let (status, body) = request(&app, "PATCH", &cat_path, Some(json!({"includeInUpdate": 1}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["includeInUpdate"], 1);
	assert_eq!(body["includeInDownload"], 0);
	assert_eq!(body["name"], "Flags");

	let (status, body) = request(
		&app,
		"PATCH",
		&cat_path,
		Some(json!({"includeInDownload": 2, "name": "Flags2"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["includeInUpdate"], 1);
	assert_eq!(body["includeInDownload"], 2);
	assert_eq!(body["name"], "Flags2");

	// 回读确认落库
	let (_, body) = request(&app, "GET", "/api/v1/categories", None).await;
	let cats = body.as_array().expect("categories");
	let row = cats
		.iter()
		.find(|c| c["id"].as_i64() == Some(cat))
		.expect("category");
	assert_eq!(row["includeInUpdate"], 1);
	assert_eq!(row["includeInDownload"], 2);

	// {} 与空 body 都是无操作
	let (status, body) = request(&app, "PATCH", &cat_path, Some(json!({}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["includeInUpdate"], 1);
	let (status, body) = request(&app, "PATCH", &cat_path, None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["includeInDownload"], 2);

	// 非法值 400，且不落库
	for bad in [
		json!({"includeInUpdate": 3}),
		json!({"includeInDownload": -1}),
		json!({"includeInUpdate": "x"}),
	] {
		let (status, body) = request(&app, "PATCH", &cat_path, Some(bad.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
		assert_eq!(body["error"]["code"], "bad_request");
	}
	let (_, body) = request(&app, "GET", "/api/v1/categories", None).await;
	let row = body
		.as_array()
		.expect("categories")
		.iter()
		.find(|c| c["id"].as_i64() == Some(cat))
		.expect("category")
		.clone();
	assert_eq!(row["includeInUpdate"], 1);
	assert_eq!(row["includeInDownload"], 2);

	let (status, _) = request(&app, "PATCH", "/api/v1/categories/999", Some(json!({"includeInUpdate": 1}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 注册前三条搜索结果（带完整条目，标题随注册落库），返回 (id, 标题)
async fn register_three_manga(app: &Router) -> Vec<(i64, String)> {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entries = body["entries"].as_array().expect("entries");
	let mut ids = Vec::new();
	for entry in entries.iter().take(3) {
		let title = entry["title"].as_str().expect("manga title").to_string();
		let (status, body) = request(
			app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": entry})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		ids.push((body["id"].as_i64().expect("manga id"), title));
	}
	ids
}

async fn first_chapters(app: &Router, manga_id: i64, count: usize) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters")
		.iter()
		.take(count)
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect()
}

async fn put_chapter_user(
	pool: &sqlx::SqlitePool,
	chapter_id: i64,
	is_read: bool,
	is_bookmarked: bool,
	is_downloaded: bool,
	last_read_at: Option<i64>,
) {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_read, is_bookmarked, is_downloaded, last_read_at)
		 VALUES (?, 1, ?, ?, ?, ?)",
	)
	.bind(chapter_id)
	.bind(is_read)
	.bind(is_bookmarked)
	.bind(is_downloaded)
	.bind(last_read_at)
	.execute(pool)
	.await
	.expect("insert chapter_user");
}

fn entry_ids(body: &Value) -> Vec<i64> {
	body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["id"].as_i64().expect("id"))
		.collect()
}

#[tokio::test]
async fn library_search_sort_filter() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let ids = register_three_manga(&app).await;
	let (a, b, c) = (ids[0].0, ids[1].0, ids[2].0);
	let (ta, tb, tc) = (&ids[0].1, &ids[1].1, &ids[2].1);
	// 搜索/排序断言都建立在注册落库的标题上，先回读对账
	for (id, title) in [(a, ta), (b, tb), (c, tc)] {
		let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{id}"), None).await;
		assert_eq!(&body["title"], title);
	}
	for id in [a, b, c] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let pool = connect_db(&db_path).await;
	// added 排序锚点：入库时间显式拉开，避免同秒并列
	for (id, at) in [(a, 1000), (b, 1001), (c, 1002)] {
		sqlx::query("UPDATE manga_user SET in_library_at = ? WHERE manga_id = ?")
			.bind(at)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set in_library_at");
	}
	let a_ch = first_chapters(&app, a, 2).await;
	let b_ch = first_chapters(&app, b, 1).await;
	let c_ch = first_chapters(&app, c, 1).await;
	// A：2 未读（其一加书签、读过 @100）；B：1 未读已下载、读过 @300；C：1 已读、无阅读时间
	put_chapter_user(&pool, a_ch[0], false, true, false, Some(100)).await;
	put_chapter_user(&pool, a_ch[1], false, false, false, None).await;
	put_chapter_user(&pool, b_ch[0], false, false, true, Some(300)).await;
	put_chapter_user(&pool, c_ch[0], true, false, false, None).await;

	// 默认 added 排序 + unreadCount
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![c, b, a]);
	assert_eq!(body["hasNextPage"], false);
	let counts: std::collections::HashMap<i64, i64> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| {
			(
				e["id"].as_i64().expect("id"),
				e["unreadCount"].as_i64().expect("unreadCount"),
			)
		})
		.collect();
	assert_eq!(counts, std::collections::HashMap::from([(a, 2), (b, 1), (c, 0)]));

	// 搜索：B 的完整标题只命中 B
	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(tb)),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_ids(&body), vec![b]);
	// LIKE 通配符按字面量处理：把标题第 2 个字符换成 _ 或 % 后若被当通配符本应命中
	let mut wildcard: String = tb.chars().enumerate().map(|(i, ch)| if i == 1 { '_' } else { ch }).collect();
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(&wildcard)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	wildcard.insert(1, '%');
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}", urlencoding(&wildcard)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	// 空 search 视为未传
	let (_, body) = request(&app, "GET", "/api/v1/library?search=", None).await;
	assert_eq!(entry_ids(&body).len(), 3);

	// 排序
	let mut by_title = [(a, ta), (b, tb), (c, tc)];
	by_title.sort_by_key(|x| x.1.to_lowercase());
	let title_order: Vec<i64> = by_title.iter().map(|(id, _)| *id).collect();
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=title", None).await;
	assert_eq!(entry_ids(&body), title_order);
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=lastRead", None).await;
	assert_eq!(entry_ids(&body), vec![b, a, c]);
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=unread", None).await;
	assert_eq!(entry_ids(&body), vec![a, b, c]);
	let (status, _) = request(&app, "GET", "/api/v1/library?sort=bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// 过滤（多个为 AND）
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(entry_ids(&body), vec![b, a]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=downloaded", None).await;
	assert_eq!(entry_ids(&body), vec![b]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=bookmarked", None).await;
	assert_eq!(entry_ids(&body), vec![a]);
	let (_, body) = request(&app, "GET", "/api/v1/library?filter=unread,downloaded", None).await;
	assert_eq!(entry_ids(&body), vec![b]);
	let (status, _) = request(&app, "GET", "/api/v1/library?filter=unread,bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// 排除语义：unread 排除=全部已读；与包含组合取交集（b 已下载但有未读，被排除）
	let (_, body) = request(&app, "GET", "/api/v1/library?filterExclude=unread", None).await;
	assert_eq!(entry_ids(&body), vec![c]);
	let (_, body) = request(
		&app,
		"GET",
		"/api/v1/library?filter=downloaded&filterExclude=unread",
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), Vec::<i64>::new());
	let (status, _) = request(&app, "GET", "/api/v1/library?filterExclude=bogus", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// 组合：搜索 + title 排序
	let (_, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?search={}&sort=title", urlencoding(ta)),
		None,
	)
	.await;
	assert_eq!(entry_ids(&body), vec![a]);
}

#[tokio::test]
async fn library_source_counts() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path: _,
	} = test_app().await;
	let (_, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(body.as_array().expect("array").len(), 0, "{body}");

	let ids = register_three_manga(&app).await;
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	for (id, _) in ids.iter().take(2) {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}

	let (status, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!([{ "sourceId": source_id, "count": 2 }]), "{body}");

	// 移出一本后计数跟随
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/library/{}", ids[0].0), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(body, json!([{ "sourceId": source_id, "count": 1 }]), "{body}");
}

/// 库列表 id 集（排序无关断言，先排序再比）
async fn library_ids(app: &Router, qs: &str) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/library?{qs}"), None).await;
	assert_eq!(status, StatusCode::OK, "{qs}");
	let mut ids = entry_ids(&body);
	ids.sort_unstable();
	ids
}

#[tokio::test]
async fn library_rating_source_filter() {
	let TestApp { app, tmp, db_path } = test_app().await;
	// 第二个源用同一 fixture 载荷，key 不同即是另一个源
	install_fixture_source(&tmp, "en.example-two", "Example Two", &fixture_dir().join("main.wasm"));
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(&app, "GET", "/api/v1/sources", None).await;
	let sources = body.as_array().expect("sources").clone();
	let source_id_of = |key: &str| {
		sources
			.iter()
			.find(|s| s["key"].as_str() == Some(key))
			.unwrap_or_else(|| panic!("source id for {key}"))["id"]
			.as_i64()
			.expect("source id")
	};
	let one = source_id_of("en.example-source");
	let two = source_id_of("en.example-two");

	// 源一注册 3 本、源二注册 1 本，全部入库；分级直接落库控制（fixture 不产出）
	let mut registered: Vec<(i64, String)> = Vec::new();
	for (source, take) in [(one, 3), (two, 1)] {
		let (_, body) = request(
			&app,
			"POST",
			&format!("/api/v1/sources/{source}/search"),
			Some(json!({"page": 1})),
		)
		.await;
		for entry in body["entries"].as_array().expect("entries").iter().take(take) {
			let title = entry["title"].as_str().expect("manga title").to_string();
			let (status, body) = request(
				&app,
				"POST",
				"/api/v1/manga",
				Some(json!({"sourceId": source, "manga": entry})),
			)
			.await;
			assert_eq!(status, StatusCode::OK);
			let id = body["id"].as_i64().expect("manga id");
			let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
			assert_eq!(status, StatusCode::OK);
			registered.push((id, title));
		}
	}
	let (a, b, c, d) = (
		registered[0].0,
		registered[1].0,
		registered[2].0,
		registered[3].0,
	);
	let tb = registered[1].1.clone();
	let pool = connect_db(&db_path).await;
	for (id, rating) in [(a, 1), (b, 2), (c, 3), (d, 3)] {
		sqlx::query("UPDATE manga SET content_rating = ? WHERE id = ?")
			.bind(rating)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set content_rating");
	}

	// 分级：包含集内 OR、排除集 NOT-OR
	assert_eq!(library_ids(&app, "contentRating=1,2").await, vec![a, b]);
	assert_eq!(library_ids(&app, "contentRating=3").await, vec![c, d]);
	assert_eq!(library_ids(&app, "contentRatingExclude=3").await, vec![a, b]);
	assert_eq!(
		library_ids(&app, "contentRating=1,2,3&contentRatingExclude=2").await,
		vec![a, c, d]
	);
	// 源：包含集内 OR、排除集 NOT-OR
	assert_eq!(library_ids(&app, &format!("source={one}")).await, vec![a, b, c]);
	assert_eq!(library_ids(&app, &format!("sourceExclude={one}")).await, vec![d]);
	assert_eq!(
		library_ids(&app, &format!("source={one},{two}&sourceExclude={two}")).await,
		vec![a, b, c]
	);
	// 维度间 AND
	assert_eq!(
		library_ids(&app, &format!("contentRating=3&source={one}")).await,
		vec![c]
	);
	assert_eq!(
		library_ids(&app, &format!("contentRatingExclude=3&sourceExclude={two}")).await,
		vec![a, b]
	);
	// 空 / 纯空白视为缺席
	let mut all = vec![a, b, c, d];
	all.sort_unstable();
	assert_eq!(library_ids(&app, "contentRating=").await, all);
	assert_eq!(library_ids(&app, "contentRating=%20").await, all);
	assert_eq!(library_ids(&app, "source=%20%20").await, all.clone());
	assert_eq!(library_ids(&app, "sourceExclude=").await, all);
	// 非法 token 400：越界分级、非整数
	for qs in [
		"contentRating=0",
		"contentRating=4",
		"contentRating=x",
		"contentRatingExclude=9",
		"source=abc",
		"sourceExclude=1.5",
	] {
		let (status, body) = request(&app, "GET", &format!("/api/v1/library?{qs}"), None).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{qs}");
		assert_eq!(body["error"]["code"], "bad_request", "{qs}");
	}

	// bind 位序对账：search + categoryId + 四个新参数同发，换位即错
	let (status, body) = request(&app, "POST", "/api/v1/categories", Some(json!({"name": "Fav"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat = body["id"].as_i64().expect("category id");
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{b}/categories"),
		Some(json!({"categoryIds": [cat]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let qs = format!(
		"search={}&categoryId={cat}&contentRating=2&contentRatingExclude=3&source={one},{two}&sourceExclude={two}",
		urlencoding(&tb)
	);
	assert_eq!(library_ids(&app, &qs).await, vec![b]);
}

/// query 值里的保留字符 percent-encode（测试标题是 ASCII，按字节处理即可）
fn urlencoding(raw: &str) -> String {
	let mut out = String::new();
	for byte in raw.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

#[tokio::test]
async fn embedded_webui() {
	let TestApp { app, tmp, .. } = test_app().await;
	// test_app 的 webui_dir 在 tmp 下不存在 → 不走磁盘分支
	assert!(!tmp.path().join("webui").is_dir());
	if sumidoku_server::webui_assets::has_index() {
		let index = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../webui/build/index.html"))
			.expect("repo webui index");
		let (status, headers, bytes) = request_full(&app, "GET", "/", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "no-cache");
		assert_eq!(bytes, index);

		// SPA 路由回退 index.html
		let (status, headers, bytes) = request_full(&app, "GET", "/some/spa/route", None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["cache-control"], "no-cache");
		assert_eq!(bytes, index);

		// 路径穿越被拒，保持 JSON 404
		let (status, body) = request(&app, "GET", "/../../secret", None).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
		assert_eq!(body["error"]["code"], "not_found");

		// API 不被 SPA 吞掉
		let (status, body) = request(&app, "GET", "/api/v1/nope", None).await;
		assert_eq!(status, StatusCode::NOT_FOUND);
		assert_eq!(body["error"]["code"], "not_found");
	} else {
		// 无 webui 产物的环境（CI 干净 checkout）：回「webui not built」文本
		let (status, _, bytes) = request_full(&app, "GET", "/", None).await;
		assert_eq!(status, StatusCode::OK);
		assert!(String::from_utf8(bytes).expect("utf8").contains("webui not built"));
	}
}
