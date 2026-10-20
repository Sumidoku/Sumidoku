//! 内置 OPDS 源契约：多实例注册（slug key 撞名计数、basic 凭据进 source_setting、
//! admin 面）/ OpenSearch 搜索与遍历兜底（degraded 标记）/ 详情章节映射 /
//! PSE 逐页直链与 CBZ 兜底两种页路径 / 封面缓存 / 401 与凭据通过 / migrate 落库
mod common;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use common::{TestApp, base_config, request, request_full, source_by_key};
use serde_json::{Value, json};
use std::collections::HashMap;
use sumidoku_server::build_app;

const PSE_PAGE_0: &[u8] = b"pse-page-0";
const PSE_PAGE_1: &[u8] = b"pse-page-1";
const ALPHA_COVER: &[u8] = b"alpha-cover";
const CBZ_PAGE_1: &[u8] = b"cbz-first";
const CBZ_PAGE_2: &[u8] = b"cbz-second";
const AUTH_HEADER: &str = "Basic dXNlcjpwYXNz"; // user:pass

#[derive(Clone)]
struct MockState {
	base: String,
	opensearch: bool,
}

fn root_feed(state: &MockState) -> String {
	let search_link = if state.opensearch {
		format!(
			r#"  <link rel="search" type="application/opensearchdescription+xml" href="{}/opds/search"/>"#,
			state.base
		)
	} else {
		String::new()
	};
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Komga OPDS catalog</title>
  <id>root</id>
{search_link}
  <entry>
    <title>All series</title>
    <id>allSeries</id>
    <updated>2024-01-01T00:00:00Z</updated>
    <content>Browse by series</content>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog;kind=navigation" href="{}/opds/series"/>
  </entry>
</feed>"#,
		state.base
	)
}

fn search_descriptor(base: &str) -> String {
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Search</ShortName>
  <Description>Search for series</Description>
  <Url type="application/atom+xml;profile=opds-catalog;kind=acquisition" template="{base}/opds/series?search={{searchTerms}}"/>
</OpenSearchDescription>"#
	)
}

fn series_entry(id: &str, title: &str, base: &str) -> String {
	format!(
		r#"  <entry>
    <title>{title}</title>
    <id>{id}</id>
    <updated>2024-02-01T00:00:00Z</updated>
    <content></content>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog;kind=navigation" href="{base}/opds/series/{id}"/>
  </entry>"#
	)
}

fn series_feed(query: Option<&str>, base: &str) -> String {
	let needle = query.map(str::to_lowercase);
	let mut entries = String::new();
	for (id, title) in [("series-alpha", "Alpha Series"), ("series-beta", "Beta Series")] {
		if needle.as_ref().is_some_and(|n| !title.to_lowercase().contains(n)) {
			continue;
		}
		entries.push_str(&series_entry(id, title, base));
		entries.push('\n');
	}
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>All series</title>
  <id>allSeries</id>
{entries}
</feed>"#
	)
}

fn alpha_books_feed(base: &str) -> String {
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns">
  <title>Alpha Series</title>
  <id>series-alpha</id>
  <entry>
    <title>Vol. 1</title>
    <id>book-1</id>
    <updated>2024-03-01T10:00:00Z</updated>
    <content>cbz - 12 MB</content>
    <author><name>Author One</name></author>
    <link rel="http://opds-spec.org/image/thumbnail" type="image/jpeg" href="{base}/opds/books/book-1/thumbnail/small"/>
    <link rel="http://opds-spec.org/image" type="image/jpeg" href="{base}/opds/books/book-1/thumbnail"/>
    <link rel="http://opds-spec.org/acquisition" type="application/zip" href="{base}/opds/books/book-1/file/vol1.cbz"/>
    <link rel="http://vaemendis.net/opds-pse/stream" type="image/jpeg" href="{base}/opds/books/book-1/pages/{{pageNumber}}" pse:count="2"/>
  </entry>
  <entry>
    <title>Vol. 2</title>
    <id>book-2</id>
    <updated>2024-03-02T10:00:00Z</updated>
    <content>cbz - 10 MB</content>
    <author><name>Author One</name></author>
    <link rel="http://opds-spec.org/image/thumbnail" type="image/jpeg" href="{base}/opds/books/book-2/thumbnail/small"/>
    <link rel="http://opds-spec.org/image" type="image/jpeg" href="{base}/opds/books/book-2/thumbnail"/>
    <link rel="http://opds-spec.org/acquisition" type="application/zip" href="/opds/books/book-2/file/vol2.cbz"/>
  </entry>
</feed>"#
	)
}

fn cbz_bytes() -> Vec<u8> {
	let mut cursor = std::io::Cursor::new(Vec::new());
	let mut writer = zip::ZipWriter::new(&mut cursor);
	for (name, bytes) in [("p1.png", CBZ_PAGE_1), ("p2.png", CBZ_PAGE_2)] {
		writer
			.start_file(name, zip::write::SimpleFileOptions::default())
			.expect("start entry");
		std::io::Write::write_all(&mut writer, bytes).expect("write entry");
	}
	writer.finish().expect("finish cbz");
	cursor.into_inner()
}

fn xml(body: String) -> Response {
	(
		[(header::CONTENT_TYPE, "application/atom+xml;profile=opds-catalog")],
		body,
	)
		.into_response()
}

fn binary(bytes: &'static [u8], mime: &'static str) -> Response {
	([(header::CONTENT_TYPE, mime)], bytes).into_response()
}

/// 裸 basic 校验：凭据恰为 user:pass 才放行，其余 401（WWW-Authenticate 按惯例带上）
async fn basic_gate(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
	let ok = req
		.headers()
		.get(header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		== Some(AUTH_HEADER);
	if ok {
		next.run(req).await
	} else {
		(StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic")]).into_response()
	}
}

/// 起 mock OPDS 目录：auth = 要 basic 凭据，opensearch = 根 feed 带 search 链接
async fn mock_catalog(auth: bool, opensearch: bool) -> (u16, tokio::task::JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let state = MockState {
		base: format!("http://127.0.0.1:{port}"),
		opensearch,
	};
	let cbz = std::sync::Arc::new(cbz_bytes());
	let app = Router::new()
		.route(
			"/opds/catalog",
			get(|State(state): State<MockState>| async move { xml(root_feed(&state)) }),
		)
		.route(
			"/opds/search",
			get(|State(state): State<MockState>| async move { xml(search_descriptor(&state.base)) }),
		)
		.route(
			"/opds/series",
			get(
				|State(state): State<MockState>, Query(query): Query<HashMap<String, String>>| async move {
					xml(series_feed(query.get("search").map(String::as_str), &state.base))
				},
			),
		)
		.route(
			"/opds/series/series-alpha",
			get(|State(state): State<MockState>| async move { xml(alpha_books_feed(&state.base)) }),
		)
		.route(
			"/opds/books/book-1/pages/{n}",
			get(|Path(n): Path<u32>| async move {
				match n {
					0 => binary(PSE_PAGE_0, "image/png"),
					_ => binary(PSE_PAGE_1, "image/png"),
				}
			}),
		)
		.route(
			"/opds/books/book-1/thumbnail",
		 get(|| async move { binary(ALPHA_COVER, "image/jpeg") }),
		)
		.route(
			"/opds/books/book-2/file/vol2.cbz",
			get(move || {
				let cbz = cbz.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.with_state(state);
	let app = if auth {
		app.layer(axum::middleware::from_fn(basic_gate))
	} else {
		app
	};
	let handle = tokio::spawn(async move {
		axum::serve(listener, app).await.expect("serve");
	});
	(port, handle)
}

async fn opds_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn register_instance(app: &Router, name: &str, url: &str, auth: Option<(&str, &str)>) -> Value {
	let body = match auth {
		Some((username, password)) => json!({"name": name, "url": url, "username": username, "password": password}),
		None => json!({"name": name, "url": url}),
	};
	let (status, body) = request(app, "POST", "/api/v1/sources/opds", Some(body)).await;
	assert_eq!(status, StatusCode::OK, "register opds instance: {body}");
	body
}

async fn register_alpha_manga(app: &Router, source_id: i64) -> (i64, Value) {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entry = body["entries"][0].clone();
	assert_eq!(entry["key"], "series-alpha");
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": entry})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "register manga: {body}");
	(body["id"].as_i64().expect("manga id"), entry)
}

async fn chapter_ids(app: &Router, manga_id: i64) -> Vec<(i64, String)> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters array")
		.iter()
		.map(|c| (c["id"].as_i64().expect("chapter id"), c["title"].as_str().unwrap_or("").to_string()))
		.collect()
}

#[tokio::test]
async fn register_two_instances_and_search_and_browse() {
	let t = opds_app().await;
	let (port, _server) = mock_catalog(false, true).await;
	let url = format!("http://127.0.0.1:{port}/opds/catalog");

	let first = register_instance(&t.app, "Demo Komga", &url, None).await;
	assert_eq!(first["key"], "opds.demo-komga");
	assert_eq!(first["kind"], "opds");
	assert_eq!(first["baseUrl"], url);
	// 撞名计数：同名第二个实例落 opds.demo-komga-2，两个实例同时在列表里
	let second = register_instance(&t.app, "Demo Komga", &url, None).await;
	assert_eq!(second["key"], "opds.demo-komga-2");
	let (status, body) = request(&t.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(source_by_key(&body, "opds.demo-komga")["name"], "Demo Komga");
	assert_eq!(source_by_key(&body, "opds.demo-komga-2")["kind"], "opds");

	let source_id = first["id"].as_i64().expect("source id");
	// OpenSearch：mock 只按 search 词过滤，beta 被滤掉
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["degraded"], false);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["key"], "series-alpha");
	assert_eq!(entries[0]["title"], "Alpha Series");
	assert_eq!(
		entries[0]["url"].as_str().expect("entry url"),
		format!("http://127.0.0.1:{port}/opds/series/series-alpha")
	);
	// 浏览（空 query）= 遍历根目录下钻，两本都出来且不标 degraded
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["degraded"], false);
	let keys: Vec<&str> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.filter_map(|e| e["key"].as_str())
		.collect();
	assert!(keys.contains(&"series-alpha"), "browse keys: {keys:?}");
	assert!(keys.contains(&"series-beta"), "browse keys: {keys:?}");

	// 注册参数校验：空名 / 坏 URL / 非 http 方案一律 400
	for body in [
		json!({"name": "", "url": &url}),
		json!({"name": "x", "url": "not-a-url"}),
		json!({"name": "x", "url": "ftp://host/catalog"}),
	] {
		let (status, _) = request(&t.app, "POST", "/api/v1/sources/opds", Some(body)).await;
		assert_eq!(status, StatusCode::BAD_REQUEST);
	}
}

#[tokio::test]
async fn read_pages_via_pse_links_and_cbz_fallback() {
	let t = opds_app().await;
	let (port, _server) = mock_catalog(false, true).await;
	let url = format!("http://127.0.0.1:{port}/opds/catalog");
	let source = register_instance(&t.app, "Demo", &url, None).await;
	let source_id = source["id"].as_i64().expect("source id");
	let (manga_id, _) = register_alpha_manga(&t.app, source_id).await;

	let chapters = chapter_ids(&t.app, manga_id).await;
	assert_eq!(chapters.len(), 2);
	assert_eq!(chapters[0].1, "Vol. 1");
	assert_eq!(chapters[1].1, "Vol. 2");

	// PSE 直链：pse:count 展开成逐页 Url，页码 0 基
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{}/pages", chapters[0].0), None).await;
	assert_eq!(status, StatusCode::OK, "pse pages: {body}");
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	assert_eq!(
		pages[0]["content"]["Url"][0].as_str().expect("page url"),
		format!("http://127.0.0.1:{port}/opds/books/book-1/pages/0")
	);
	for (index, want) in [PSE_PAGE_0, PSE_PAGE_1].iter().enumerate() {
		let (status, headers, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/chapter/{}/page/{index}", chapters[0].0), None).await;
		assert_eq!(status, StatusCode::OK, "pse page {index}: {}", String::from_utf8_lossy(&bytes));
		assert_eq!(headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()), Some("image/png"));
		assert_eq!(bytes.as_slice(), *want, "page {index} bytes");
	}

	// CBZ 兜底：无 PSE 链接的卷下载一次压缩包拆页（OpdsZip 变体，根相对 href 按 feed URL 解析）
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{}/pages", chapters[1].0), None).await;
	assert_eq!(status, StatusCode::OK);
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	assert_eq!(pages[0]["content"]["OpdsZip"].as_str(), Some("p1.png"));
	for (index, want) in [CBZ_PAGE_1, CBZ_PAGE_2].iter().enumerate() {
		let (status, _, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/chapter/{}/page/{index}", chapters[1].0), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(bytes.as_slice(), *want, "cbz page {index} bytes");
	}

	// 封面 = 首册 image 链接，走磁盘缓存；refresh 重拉仍是同一 URL 的字节
	for suffix in ["", "?refresh=1"] {
		let (status, headers, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover{suffix}"), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()), Some("image/jpeg"));
		assert_eq!(bytes.as_slice(), ALPHA_COVER);
	}
}

#[tokio::test]
async fn basic_auth_credentials_gate_the_catalog() {
	let t = opds_app().await;
	let (port, _server) = mock_catalog(true, true).await;
	let url = format!("http://127.0.0.1:{port}/opds/catalog");

	// 不种凭据：目录 401 以 bad_gateway 透出（状态码在 message 里可诊断）
	let anonymous = register_instance(&t.app, "Anon", &url, None).await;
	let anon_id = anonymous["id"].as_i64().expect("source id");
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{anon_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);
	assert_eq!(body["error"]["code"], "bad_gateway");
	assert!(body["error"]["message"].as_str().expect("message").contains("401"));

	// 错密码同样 401
	let wrong = register_instance(&t.app, "Wrong", &url, Some(("user", "nope"))).await;
	let wrong_id = wrong["id"].as_i64().expect("source id");
	let (status, _) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{wrong_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_GATEWAY);

	// 正确凭据：搜索、登记、页字节全通（凭据从 source_setting 读）
	let authed = register_instance(&t.app, "Authed", &url, Some(("user", "pass"))).await;
	let authed_id = authed["id"].as_i64().expect("source id");
	let (manga_id, _) = register_alpha_manga(&t.app, authed_id).await;
	let chapters = chapter_ids(&t.app, manga_id).await;
	assert_eq!(chapters.len(), 2);
	let (status, _, bytes) = request_full(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{}/page/0", chapters[0].0),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes.as_slice(), PSE_PAGE_0);
}

#[tokio::test]
async fn missing_opensearch_falls_back_to_degraded_traversal() {
	let t = opds_app().await;
	let (port, _server) = mock_catalog(false, false).await;
	let url = format!("http://127.0.0.1:{port}/opds/catalog");
	let source = register_instance(&t.app, "NoSearch", &url, None).await;
	let source_id = source["id"].as_i64().expect("source id");

	// 无 OpenSearch 描述：退遍历 + degraded 标记，结果按标题子串过滤
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["degraded"], true);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["key"], "series-alpha");

	// 聚合搜索同形：degraded 随源结果透出
	let (status, body) = request(
		&t.app,
		"POST",
		"/api/v1/search",
		Some(json!({"query": "beta", "sourceIds": [source_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["sources"][0]["degraded"], true);
	let entries = body["sources"][0]["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["key"], "series-beta");
}

#[tokio::test]
async fn migrate_from_local_to_opds_registers_and_shelves() {
	let t = opds_app().await;
	// 本地源漫画入库：local_dir 默认派生 <config_dir>/local
	let manga_dir = t.tmp.path().join("local").join("MangaX").join("ch 1");
	std::fs::create_dir_all(&manga_dir).expect("mkdir local chapter");
	std::fs::write(manga_dir.join("p1.png"), common::PNG).expect("write local page");
	let (status, body) = request(&t.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local_id = source_by_key(&body, "local.local")["id"].as_i64().expect("local id");
	let (status, body) = request(
		&t.app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": local_id, "mangaKey": "MangaX"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let from_manga_id = body["id"].as_i64().expect("local manga id");
	let (status, _) = request(&t.app, "POST", "/api/v1/library", Some(json!({"mangaId": from_manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	let (port, _server) = mock_catalog(false, true).await;
	let url = format!("http://127.0.0.1:{port}/opds/catalog");
	let source = register_instance(&t.app, "Demo", &url, None).await;
	let opds_id = source["id"].as_i64().expect("source id");

	// lookup 走目录搜索臂，候选带条目 url（migrate 回源凭据）
	let (status, body) = request(
		&t.app,
		"GET",
		&format!("/api/v1/migrate/lookup?mangaId={from_manga_id}&targetSourceId={opds_id}&query=alpha"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let candidate = body["candidates"][0].clone();
	assert_eq!(candidate["key"], "series-alpha");

	let (status, body) = request(
		&t.app,
		"POST",
		"/api/v1/migrate",
		Some(json!({
			"mode": "copy",
			"entries": [{
				"fromMangaId": from_manga_id,
				"toSourceId": opds_id,
				"toMangaKey": "series-alpha",
				"toManga": candidate,
			}],
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["results"][0]["ok"], true, "migrate result: {body}");
	let to_manga_id = body["results"][0]["toMangaId"].as_i64().expect("target manga id");

	// 目标已按统一机制落库：章节对账完成、在库、页可读（PSE 路径）
	let chapters = chapter_ids(&t.app, to_manga_id).await;
	assert_eq!(chapters.len(), 2);
	let (status, body) = request(&t.app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let ids: Vec<i64> = body["entries"]
		.as_array()
		.expect("library entries")
		.iter()
		.filter_map(|e| e["id"].as_i64())
		.collect();
	assert!(ids.contains(&to_manga_id), "library ids: {ids:?}");
	let (status, _, bytes) = request_full(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{}/page/1", chapters[0].0),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes.as_slice(), PSE_PAGE_1);
}
