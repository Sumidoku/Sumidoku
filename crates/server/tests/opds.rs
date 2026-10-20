mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::routing::get;
use common::{TestApp, base_config, connect_db, fixture_dir, register_example_manga, request, test_app};
use serde_json::{Value, json};
use sumidoku_server::build_app;
use tower::ServiceExt;

// 1x1 transparent PNG
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// OPDS assertions inspect absolute URLs, so requests always carry Host + X-Forwarded-Proto
async fn opds_get(app: &Router, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
	opds_get_auth(app, path, None).await
}

async fn opds_get_auth(app: &Router, path: &str, auth: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
	let mut builder = Request::builder()
		.method("GET")
		.uri(path)
		.header(header::HOST, "opds.test")
		.header("x-forwarded-proto", "https");
	if let Some(auth) = auth {
		builder = builder.header(header::AUTHORIZATION, auth);
	}
	let request = builder.body(Body::empty()).expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	let status = response.status();
	let headers = response.headers().clone();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	(status, headers, bytes.to_vec())
}

/// Boots the app and registers + shelves one manga, with downloads_dir pointing into tmp (the file endpoint reads real artifacts)
async fn opds_app() -> (TestApp, i64) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	(TestApp { app, tmp, db_path }, manga_id)
}

async fn first_chapters(app: &Router, manga_id: i64) -> Vec<Value> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array().expect("chapters").clone()
}

#[tokio::test]
async fn feeds_files_and_escaping() {
	let (test_app, manga_id) = opds_app().await;
	let TestApp { app, tmp, db_path } = &test_app;

	// root: navigation + osd links
	let (status, headers, body) = opds_get(app, "/opds/v1.2/root").await;
	assert_eq!(status, StatusCode::OK);
	let content_type = headers[header::CONTENT_TYPE].to_str().expect("content-type");
	assert!(content_type.starts_with("application/atom+xml;profile=opds-catalog"));
	let body = String::from_utf8(body).expect("utf8");
	for link in [
		"/opds/v1.2/library",
		"/opds/v1.2/categories",
		"/opds/v1.2/sources",
		"/opds/v1.2/history",
		"/opds/v1.2/updates",
		"/opds/v1.2/osd",
	] {
		assert!(body.contains(link), "root feed should link {link}");
	}

	// osd: the OpenSearchDescription template
	let (status, headers, body) = opds_get(app, "/opds/v1.2/osd").await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers[header::CONTENT_TYPE], "application/opensearchdescription+xml");
	let body = String::from_utf8(body).expect("utf8");
	assert!(body.contains("https://opds.test/opds/v1.2/search?q={searchTerms}"));

	// library: title + absolute cover link
	let (_, manga) = request(app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	let title = manga["title"].as_str().expect("title").to_string();
	let (status, _, body) = opds_get(app, "/opds/v1.2/library").await;
	assert_eq!(status, StatusCode::OK);
	let body = String::from_utf8(body).expect("utf8");
	assert!(body.contains(&format!("<title>{title}</title>")));
	assert!(body.contains(&format!("https://opds.test/opds/v1.2/cover/{manga_id}")));
	assert!(body.contains(&format!("urn:sumidoku:manga:{manga_id}")));

	// series: populate the page cache first to get page_count=2, then record progress
	let chapters = first_chapters(app, manga_id).await;
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");
	let other_chapter_id = chapters[1]["id"].as_i64().expect("chapter id");
	let (status, _) = request(app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// the download artifact does not depend on background tasks: place the CBZ and a finished row by hand
	std::fs::create_dir_all(tmp.path().join("downloads")).expect("downloads dir");
	let cbz = make_zip("page/1.jpg", PNG);
	std::fs::write(tmp.path().join("downloads/ch1.cbz"), &cbz).expect("write cbz");
	let pool = connect_db(db_path).await;
	sqlx::query(
		"INSERT INTO download (chapter_id, user_id, status, format, path, created_at, updated_at)
		 VALUES (?, 1, 'finished', 'cbz', 'ch1.cbz', 0, 0)",
	)
	.bind(chapter_id)
	.execute(&pool)
	.await
	.expect("insert download");

	let (status, _, body) = opds_get(app, &format!("/opds/v1.2/series/{manga_id}")).await;
	assert_eq!(status, StatusCode::OK);
	let body = String::from_utf8(body).expect("utf8");
	assert!(body.contains("xmlns:pse=\"http://vaemendis.net/opds-pse\""));
	assert!(body.contains("pse:count=\"2\""), "series feed: {body}");
	// lastPageRead is a 0-based index, PSE lastRead counts from 1
	assert!(body.contains("pse:lastRead=\"2\""), "series feed: {body}");
	assert!(body.contains("pse:lastReadDate="), "series feed: {body}");
	// href must contain the literal {pageNumber} for the client to substitute; chapters without progress get no lastRead
	assert!(
		body.contains(&format!("https://opds.test/opds/v1.2/stream/{chapter_id}/{{pageNumber}}")),
		"series feed: {body}"
	);
	assert_eq!(body.matches("pse:lastRead=").count(), 1, "series feed: {body}");
	// only the downloaded chapter carries a zip acquisition
	assert_eq!(body.matches("http://opds-spec.org/acquisition").count(), 1);
	assert!(body.contains(&format!("https://opds.test/opds/v1.2/file/{chapter_id}")));
	assert!(!body.contains(&format!("/opds/v1.2/file/{other_chapter_id}")));

	// file: 200 zip for the finished one, 404 for the not-downloaded one
	let (status, headers, bytes) = opds_get(app, &format!("/opds/v1.2/file/{chapter_id}")).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers[header::CONTENT_TYPE], "application/zip");
	assert_eq!(bytes, cbz);
	let (status, _, _) = opds_get(app, &format!("/opds/v1.2/file/{other_chapter_id}")).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// escaping: &<>" in the title must be entity-escaped
	sqlx::query("UPDATE manga SET title = ? WHERE id = ?")
		.bind("A & B <C> \"D\"")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update title");
	let (status, _, body) = opds_get(app, "/opds/v1.2/library").await;
	assert_eq!(status, StatusCode::OK);
	let body = String::from_utf8(body).expect("utf8");
	assert!(body.contains("A &amp; B &lt;C&gt; &quot;D&quot;"), "library feed: {body}");
}

#[tokio::test]
async fn stream_page_proxy() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let manga_id = register_example_manga(&app).await;
	let pool = connect_db(&db_path).await;
	let chapters = first_chapters(&app, manga_id).await;
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);

	// point the page content at the local image host
	let (port, _upstream) = serve_bytes(vec![("/img.png", "image/png", PNG.to_vec())]).await;
	update_page_content(
		&pool,
		chapter_id,
		0,
		json!({"Url": [format!("http://127.0.0.1:{port}/img.png"), null]}),
	)
	.await;
	let (status, headers, bytes) = opds_get(&app, &format!("/opds/v1.2/stream/{chapter_id}/0")).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers[header::CONTENT_TYPE], "image/png");
	assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=3600");
	assert_eq!(bytes, PNG);
}

#[tokio::test]
async fn basic_auth_guard() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.basic_password = Some("secret".into());
	let app = build_app(config).await.expect("build app");

	// no credentials: 401 + WWW-Authenticate
	let (status, headers, _) = opds_get(&app, "/opds/v1.2/root").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(
		headers[header::WWW_AUTHENTICATE],
		"Basic realm=\"Sumidoku OPDS\""
	);

	// wrong password: 401 (admin:wrong)
	let (status, _, _) = opds_get_auth(&app, "/opds/v1.2/root", Some("Basic YWRtaW46d3Jvbmc=")).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);

	// real username+password: 200 (admin:secret)
	let (status, _, _) = opds_get_auth(&app, "/opds/v1.2/root", Some("Basic YWRtaW46c2VjcmV0")).await;
	assert_eq!(status, StatusCode::OK);

	// API routes are unaffected by OPDS Basic
	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");
}

/// Starts a minimal static server returning bytes; aborting the handle shuts it down
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

async fn update_page_content(pool: &sqlx::SqlitePool, chapter_id: i64, idx: i64, content: Value) {
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
