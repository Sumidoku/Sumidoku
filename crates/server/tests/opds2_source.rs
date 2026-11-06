//! OPDS 2.0 (JSON) catalog contract for the built-in OPDS source: per-response format detection (Content-Type
//! plus a JSON body sniff for mislabeled servers), templated search links ({?query}), series drill-down through
//! feed-typed self links, chapter pages enumerated from the publication manifest's readingOrder, and the
//! acquisition CBZ fallback kept from 1.2. The mock mirrors upstream OPDS 2.0 catalog and publication documents,
//! including a Komga-style publication document for the page list
mod common;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use common::{TestApp, base_config, request, request_full};
use serde_json::{Value, json};
use std::collections::HashMap;
use sumidoku_server::build_app;

const PAGE_0: &[u8] = b"v2-page-0";
const PAGE_1: &[u8] = b"v2-page-1";
const ALPHA_COVER: &[u8] = b"alpha-cover";
const CBZ_PAGE_1: &[u8] = b"cbz-first";
const CBZ_PAGE_2: &[u8] = b"cbz-second";
const FEED_TYPE: &str = "application/opds+json";
const PUBLICATION_TYPE: &str = "application/opds-publication+json";

#[derive(Clone)]
struct MockState {
	base: String,
	/// Serves JSON bodies under an Atom Content-Type: the body sniff, not the header, must pick the parser
	wrong_type: bool,
	gallery_search_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
	root_publication_only: bool,
	groups_only: bool,
	hide_featured: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn root_feed(state: &MockState) -> String {
	let base = &state.base;
	if state.root_publication_only {
		return json!({
			"metadata": {"title": "Root publications"},
			"publications": [{
				"metadata": {"identifier": "root-only", "title": "Root Only Book"},
				"links": [{"rel": "http://opds-spec.org/acquisition", "href": format!("{base}/opds/v2/books/root-only.cbz"), "type": "application/zip"}]
			}]
		}).to_string();
	}
	if state.groups_only {
		return json!({
			"metadata": {"title": "Groups only"},
			"links": [{"rel":"self", "href":format!("{base}/opds/v2/root"), "type":FEED_TYPE}],
			"groups": [{"metadata":{"title":"Only group"}, "navigation":[{"title":"Only child", "href":format!("{base}/opds/v2/nested"), "type":FEED_TYPE}]}]
		}).to_string();
	}
	let mut root = json!({
		"metadata": {"title": "Demo v2 catalog"},
		"links": [
			{"rel": "self", "href": format!("{base}/opds/v2/root"), "type": FEED_TYPE},
			{"rel": "start", "href": format!("{base}/opds/v2/root"), "type": FEED_TYPE},
			{
				"rel": "search",
				"href": format!("{base}/opds/v2/search?query={{searchTerms}}"),
				"type": FEED_TYPE,
				"templated": true
			},
		],
		"navigation": [
			{"title": "All series", "href": format!("{base}/opds/v2/series"), "type": FEED_TYPE},
		],
		"groups": [{
			"metadata": {"title": "Featured works"},
			"navigation": [{"title": "Nested shelf", "href": format!("{base}/opds/v2/nested"), "type": FEED_TYPE}],
			"publications": [
				{
					"metadata": {"title": "Featured Book"},
					"links": [{"rel": "http://opds-spec.org/acquisition", "href": "books/featured.cbz", "type": "application/zip"}],
					"images": [{"href": "cover/featured.jpg", "type": "image/jpeg"}]
				},
				{
					"metadata": {"title": "Featured Book Two"},
					"links": [{"rel": "http://opds-spec.org/acquisition", "href": "books/featured-2.cbz", "type": "application/zip"}]
				}
			]
		}],
	});
	if state.hide_featured.load(std::sync::atomic::Ordering::Relaxed) {
		root["groups"][0]["publications"] = json!([{
			"metadata": {"title": "Featured Book Two"},
			"links": [{"rel": "http://opds-spec.org/acquisition", "href": "books/featured-2.cbz", "type": "application/zip"}]
		}]);
	}
	root.to_string()
}

/// A series is a publication whose self link is feed-typed (an OPDS 2.0 publication document)
fn series_publication(base: &str, id: &str, title: &str) -> Value {
	json!({
		"metadata": {
			"@type": "http://schema.org/Book",
			"identifier": id,
			"title": title,
			"author": [{"name": "Author One"}],
			"modified": "2024-02-01T00:00:00Z",
			"description": format!("{title} description"),
		},
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/series/{id}"), "type": FEED_TYPE}],
		"images": [{"href": format!("{base}/opds/v2/cover/{id}"), "type": "image/jpeg"}],
	})
}

fn series_feed(query: Option<&str>, base: &str) -> String {
	let needle = query.map(str::to_lowercase);
	let mut publications = Vec::new();
	for (id, title) in [("series-alpha", "Alpha Series"), ("series-beta", "Beta Series")] {
		if needle.as_ref().is_some_and(|n| !title.to_lowercase().contains(n)) {
			continue;
		}
		publications.push(series_publication(base, id, title));
	}
	json!({
		"metadata": {"title": "All series"},
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/series"), "type": FEED_TYPE}],
		"publications": publications,
	})
	.to_string()
}

fn gallery_search_feed(base: &str) -> String {
	json!({
		"metadata": {"title": "Gallery search results"},
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/search?query=gallery"), "type": FEED_TYPE}],
		"groups": [{
			"metadata": {"title": "Galleries"},
			"publications": [
				{
					"metadata": {"identifier": "gallery-1", "title": "Gallery One"},
					"links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": format!("{base}/opds/v2/books/gallery-1.cbz"), "type": "application/zip"}]
				},
				{
					"metadata": {"identifier": "gallery-2", "title": "Gallery Two"},
					"links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": format!("{base}/opds/v2/books/gallery-2.cbz"), "type": "application/zip"}]
				}
			]
		}]
	})
	.to_string()
}

fn series_listing_feed(page: u32, base: &str) -> String {
	let publication = match page {
		1 => Some(series_publication(base, "series-alpha", "Alpha Series")),
		2 => Some(series_publication(base, "series-beta", "Beta Series")),
		_ => None,
	};
	let mut links = vec![json!({"rel": "self", "href": format!("{base}/opds/v2/series?page={page}"), "type": FEED_TYPE})];
	if page == 1 {
		links.push(json!({"rel": "next", "href": "?page=2", "type": FEED_TYPE}));
	}
	let navigation = if page == 1 {
		json!([{"title": "Nested child shelf", "href": "nested/child", "type": FEED_TYPE}])
	} else {
		json!([])
	};
	let groups = match page {
		1 => json!([{
			"metadata": {"title": "Child group"},
			"navigation": [{"title": "Nested group shelf", "href": "nested/group", "type": FEED_TYPE}],
			"publications": [{
				"metadata": {"identifier": "child-book", "title": "Child Volume"},
				"links": [
					{"rel": "self", "href": format!("{base}/opds/v2/books/child-book/manifest"), "type": PUBLICATION_TYPE},
					{"rel": "http://opds-spec.org/acquisition", "href": "books/child-book.cbz", "type": "application/zip"}
				]
			}]
		}]),
		2 => json!([{
			"metadata": {"title": "Second child group"},
			"publications": [{
				"metadata": {"identifier": "child-book-2", "title": "Second Child Volume"},
				"links": [{"rel": "http://opds-spec.org/acquisition", "href": "books/child-book-2.cbz", "type": "application/zip"}]
			}]
		}]),
		_ => json!([]),
	};
	json!({
		"metadata": {"title": "All series"},
		"links": links,
		"navigation": navigation,
		"groups": groups,
		"publications": publication.into_iter().collect::<Vec<_>>()
	})
	.to_string()
}

fn alpha_books_feed(base: &str) -> String {
	json!({
		"metadata": {"title": "Alpha Series"},
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/series/series-alpha"), "type": FEED_TYPE}],
		"publications": [
			{
				"metadata": {
					"@type": "http://schema.org/Book",
					"identifier": "book-1",
					"title": "Vol. 1",
					"author": [{"name": "Author One"}],
					"subject": ["Action", {"name": "Adventure"}],
					"artist": "Artist One",
					"contributor": [{"name": "Creator One", "role": "cre"}],
					"modified": "2024-03-01T10:00:00Z",
					"belongsTo": {"series": {"name": "Alpha Series", "links": [
						{"href": format!("{base}/opds/v2/series/series-alpha"), "type": FEED_TYPE},
					]}},
				},
				"links": [
					{"rel": "self", "href": format!("{base}/opds/v2/books/book-1/manifest"), "type": PUBLICATION_TYPE},
					{"rel": "http://opds-spec.org/acquisition", "type": "application/zip", "href": format!("{base}/opds/v2/books/book-1/file/vol1.cbz")},
				],
				"images": [{"href": format!("{base}/opds/v2/cover/book-1"), "type": "image/jpeg"}],
			},
			{
				"metadata": {
					"@type": "http://schema.org/Book",
					"identifier": "book-2",
					"title": "Vol. 2",
					"author": [{"name": "Author One"}],
					"modified": "2024-03-02T10:00:00Z",
				},
				"links": [
					{"rel": "http://opds-spec.org/acquisition", "type": "application/zip", "href": format!("{base}/opds/v2/books/book-2/file/vol2.cbz")},
				],
				"images": [{"href": format!("{base}/opds/v2/cover/book-2"), "type": "image/jpeg"}],
			},
		],
	})
	.to_string()
}

/// The publication document (Komga's manifest role): readingOrder hrefs are relative, resolved against the manifest URL
fn book1_manifest(base: &str) -> String {
	json!({
		"metadata": {"@type": "http://schema.org/Book", "identifier": "book-1", "title": "Vol. 1", "modified": "2024-03-01T10:00:00Z"},
		"links": [{"rel": "self", "href": format!("{base}/opds/v2/books/book-1/manifest"), "type": PUBLICATION_TYPE}],
		"readingOrder": [
			{"href": "pages/0", "type": "image/png"},
			{"href": "pages/1", "type": "image/png"},
		],
	})
	.to_string()
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

fn feed_response(state: &MockState, body: String, json_type: &'static str) -> Response {
	let content_type = if state.wrong_type {
		"application/atom+xml;profile=opds-catalog"
	} else {
		json_type
	};
	([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn binary(bytes: &'static [u8], mime: &'static str) -> Response {
	([(header::CONTENT_TYPE, mime)], bytes).into_response()
}

async fn mock_v2_catalog(wrong_type: bool) -> (u16, tokio::task::JoinHandle<()>) {
	let (port, handle, _) = mock_v2_catalog_mode(wrong_type, false, false).await;
	(port, handle)
}

async fn mock_v2_catalog_mode(
	wrong_type: bool,
	groups_only: bool,
	root_publication_only: bool,
) -> (u16, tokio::task::JoinHandle<()>, std::sync::Arc<std::sync::atomic::AtomicBool>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let state = MockState {
		base: format!("http://127.0.0.1:{port}"),
		wrong_type,
		gallery_search_requests: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
		root_publication_only,
		groups_only,
		hide_featured: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
	};
	let hide_featured = state.hide_featured.clone();
	let cbz = std::sync::Arc::new(cbz_bytes());
	let cbz_vol1 = cbz.clone();
	let cbz_vol2 = cbz.clone();
	let cbz_featured = cbz.clone();
	let cbz_root_only = cbz.clone();
	let cbz_featured_two = cbz.clone();
	let app = Router::new()
		.route(
			"/opds/v2/root",
			get(|State(state): State<MockState>| async move {
				let body = root_feed(&state);
				feed_response(&state, body, FEED_TYPE)
			}),
		)
		.route(
			"/opds/v2/series",
			get(|State(state): State<MockState>, Query(query): Query<HashMap<String, String>>| async move {
				let page = query.get("page").and_then(|p| p.parse::<u32>().ok()).unwrap_or(1);
				let body = series_listing_feed(page, &state.base);
				feed_response(&state, body, FEED_TYPE)
			}),
		)
		.route(
			"/opds/v2/nested",
			get(|State(state): State<MockState>| async move {
				let body = if state.groups_only {
					json!({"metadata":{"title":"Only child"},"navigation":[{"title":"Deep child","href":"/opds/v2/nested/deep","type":FEED_TYPE}],"publications":[{"metadata":{"identifier":"nested-book","title":"Nested book"},"links":[{"rel":"http://opds-spec.org/acquisition","href":"books/featured.cbz","type":"application/zip"}]}]}).to_string()
				} else {
					json!({"metadata":{"title":"Nested shelf"},"publications":[]}).to_string()
				};
				feed_response(&state, body, FEED_TYPE)
			}),
		)
		.route(
			"/opds/v2/nested/child",
			get(|| async move { ([(header::CONTENT_TYPE, FEED_TYPE)], json!({"metadata":{"title":"Nested child"},"publications":[]}).to_string()).into_response() }),
		)
		.route(
			"/opds/v2/nested/group",
			get(|| async move { ([(header::CONTENT_TYPE, FEED_TYPE)], json!({"metadata":{"title":"Nested group"},"publications":[]}).to_string()).into_response() }),
		)
		.route(
			"/opds/v2/nested/deep",
			get(|| async move { ([(header::CONTENT_TYPE, FEED_TYPE)], json!({"metadata":{"title":"Deep child"},"publications":[{"metadata":{"identifier":"deep-book","title":"Deep book"},"links":[{"rel":"http://opds-spec.org/acquisition","href":"/opds/v2/books/featured.cbz","type":"application/zip"}]}]}).to_string()).into_response() }),
		)
		.route(
			"/opds/v2/search",
			get(
				|State(state): State<MockState>, Query(query): Query<HashMap<String, String>>| async move {
					let body = if query.get("query").is_some_and(|value| value == "gallery") {
						if state
							.gallery_search_requests
							.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
							> 0
						{
							return (StatusCode::BAD_GATEWAY, "gallery search feed unexpectedly fetched again").into_response();
						}
						gallery_search_feed(&state.base)
					} else {
						series_feed(query.get("query").map(String::as_str), &state.base)
					};
					feed_response(&state, body, FEED_TYPE)
				},
			),
		)
		.route(
			"/opds/v2/series/series-alpha",
			get(|State(state): State<MockState>| async move {
				let body = alpha_books_feed(&state.base);
				feed_response(&state, body, FEED_TYPE)
			}),
		)
		.route(
			"/opds/v2/books/book-1/manifest",
			get(|State(state): State<MockState>| async move {
				let body = book1_manifest(&state.base);
				feed_response(&state, body, PUBLICATION_TYPE)
			}),
		)
		.route(
			"/opds/v2/books/child-book/manifest",
			get(|| async {
				(
					[(header::CONTENT_TYPE, PUBLICATION_TYPE)],
					json!({"readingOrder": [{"href": "pages/0", "type": "image/png"}]}).to_string(),
				)
					.into_response()
			}),
		)
		.route(
			"/opds/v2/books/child-book/pages/0",
			get(|| async { binary(PAGE_0, "image/png") }),
		)
		.route(
			"/opds/v2/books/book-1/pages/{n}",
			get(|Path(n): Path<u32>| async move {
				match n {
					0 => binary(PAGE_0, "image/png"),
					_ => binary(PAGE_1, "image/png"),
				}
			}),
		)
		.route(
			"/opds/v2/books/book-1/file/vol1.cbz",
			get(move || {
				let cbz = cbz_vol1.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.route(
			"/opds/v2/books/book-2/file/vol2.cbz",
			get(move || {
				let cbz = cbz_vol2.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.route(
			"/opds/v2/books/featured.cbz",
			get(move || {
				let cbz = cbz_featured.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.route(
			"/opds/v2/books/root-only.cbz",
			get(move || {
				let cbz = cbz_root_only.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.route(
			"/opds/v2/books/featured-2.cbz",
			get(move || {
				let cbz = cbz_featured_two.clone();
				async move { ([(header::CONTENT_TYPE, "application/zip")], cbz.as_ref().clone()).into_response() }
			}),
		)
		.route(
			"/opds/v2/cover/{id}",
			get(|| async move { binary(ALPHA_COVER, "image/jpeg") }),
		)
		.with_state(state);
	let handle = tokio::spawn(async move {
		axum::serve(listener, app).await.expect("serve");
	});
	(port, handle, hide_featured)
}

async fn opds_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp::new(app, tmp, db_path)
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
async fn v2_catalog_search_update_and_pages() {
	let t = opds_app().await;
	let (port, _server) = mock_v2_catalog(false).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, body) = request(&t.app, "POST", "/api/v1/sources/internal", Some(json!({"type": "opds", "name": "Demo V2", "url": &url}))).await;
	assert_eq!(status, StatusCode::OK, "register opds instance: {body}");
	assert_eq!(body["key"], "opds.demo-v2");
	let source_id = body["id"].as_i64().expect("source id");

	// templated search link ({?query}): no OpenSearch descriptor, no degraded flag
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["degraded"], false);
	assert_eq!(body["hasNextPage"], false);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["key"], "series-alpha");
	assert_eq!(entries[0]["title"], "Alpha Series");
	assert_eq!(
		entries[0]["url"].as_str().expect("entry url"),
		format!("http://127.0.0.1:{port}/opds/v2/series/series-alpha")
	);
	assert_eq!(
		entries[0]["cover"].as_str().expect("entry cover"),
		format!("http://127.0.0.1:{port}/opds/v2/cover/series-alpha")
	);

	// Grouped acquisition publications are standalone works and must survive search result mapping.
	let (status, gallery_results) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "gallery"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{gallery_results}");
	let gallery_entries = gallery_results["entries"].as_array().expect("gallery entries");
	assert_eq!(gallery_entries.len(), 2);
	assert_eq!(gallery_entries[0]["title"], "Gallery One");
	assert!(gallery_entries[0]["key"].as_str().unwrap().starts_with("opds:work:"));
	assert_eq!(gallery_entries[0]["chapters"].as_array().unwrap().len(), 1);
	assert_eq!(gallery_entries[0]["chapters"][0]["key"], "gallery-1");
	let (status, gallery_manga) = request(
		&t.app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": gallery_entries[0]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{gallery_manga}");
	let gallery_chapters = chapter_ids(&t.app, gallery_manga["id"].as_i64().unwrap()).await;
	assert_eq!(gallery_chapters.len(), 1);
	assert_eq!(gallery_chapters[0].1, "Gallery One");

	// browsing (empty query) drills the navigation items: both series come out
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

	let (manga_id, _) = register_alpha_manga(&t.app, source_id).await;
	let (status, detail) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{detail}");
	assert_eq!(detail["tags"], json!(["Action", "Adventure"]));
	assert_eq!(detail["artists"], json!(["Artist One"]));
	assert_eq!(detail["authors"], json!(["Author One", "Creator One"]));
	let chapters = chapter_ids(&t.app, manga_id).await;
	assert_eq!(chapters.len(), 2);
	assert_eq!(chapters[0].1, "Vol. 1");
	assert_eq!(chapters[1].1, "Vol. 2");

	// readingOrder pages: the publication manifest enumerates per-page Urls (relative hrefs resolved against it)
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{}/pages", chapters[0].0), None).await;
	assert_eq!(status, StatusCode::OK, "readingOrder pages: {body}");
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	for (index, want) in [PAGE_0, PAGE_1].iter().enumerate() {
		assert_eq!(
			pages[index]["content"]["Url"][0].as_str().expect("page url"),
			format!("http://127.0.0.1:{port}/opds/v2/books/book-1/pages/{index}")
		);
		let (status, headers, bytes) =
			request_full(&t.app, "GET", &format!("/api/v1/chapter/{}/page/{index}", chapters[0].0), None).await;
		assert_eq!(status, StatusCode::OK, "page {index}: {}", String::from_utf8_lossy(&bytes));
		assert_eq!(headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()), Some("image/png"));
		assert_eq!(bytes.as_slice(), *want, "page {index} bytes");
	}

	// acquisition CBZ keeps the 1.2 path: download once, split through the ZipCache (OpdsZip variant)
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

	// cover = the first book entry's image link through the disk cache
	let (status, headers, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()), Some("image/jpeg"));
	assert_eq!(bytes.as_slice(), ALPHA_COVER);
}

#[tokio::test]
async fn opds2_listings_keep_root_navigation_and_grouped_single_work() {
	let t = opds_app().await;
	let (port, _server) = mock_v2_catalog(false).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, registered) = request(
		&t.app,
		"POST",
		"/api/v1/sources/internal",
		Some(json!({"type": "opds", "name": "Grouped OPDS", "url": url})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{registered}");
	let source_id = registered["id"].as_i64().expect("source id");
	let (status, listings) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::OK);
	let listings = listings.as_array().expect("listings");
	assert_eq!(listings[0]["id"], "opds:all");
	assert_eq!(listings[0]["name"], "All");
	assert_eq!(listings.len(), 2);
	assert_eq!(listings[1]["name"], "All series");
	let (status, all) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/opds:all"), None).await;
	assert_eq!(status, StatusCode::OK, "{all}");
	assert!(all["navigation"].as_array().unwrap().is_empty(), "root navigation is exposed by listing tabs");
	assert_eq!(all["groups"][0]["navigation"][0]["title"], "Nested shelf");
	let group_nav_id = all["groups"][0]["navigation"][0]["listingId"].as_str().unwrap();
	assert_eq!(all["groups"][0]["entries"].as_array().unwrap().len(), 2);
	assert!(!listings.iter().any(|listing| listing["name"] == "Nested shelf"), "group navigation stays inline");
	let (status, nested) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/{group_nav_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{nested}");
	assert_eq!(nested["parentId"], "opds:all");
	assert_eq!(nested["entries"].as_array().unwrap().len(), 0);
	let listing_id = listings[1]["id"].as_str().expect("root navigation listing id");
	let path = format!("/api/v1/sources/{source_id}/listings/{listing_id}");
	let (status, first) = request(&t.app, "GET", &format!("{path}?page=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{first}");
	assert_eq!(first["entries"].as_array().unwrap().len(), 1);
	assert_eq!(first["entries"][0]["key"], "series-alpha");
	assert_eq!(first["groups"][0]["title"], "Child group");
	assert_eq!(first["groups"][0]["entries"].as_array().unwrap().len(), 1);
	assert_eq!(first["navigation"][0]["title"], "Nested child shelf");
	assert_eq!(first["groups"][0]["navigation"][0]["title"], "Nested group shelf");
	let nested_id = first["navigation"][0]["listingId"].as_str().expect("nested listing id");
	assert!(nested_id.starts_with("opds:path:"));
	let (status, nested) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/{nested_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{nested}");
	assert_eq!(nested["parentId"], listing_id);
	assert!(first["groups"][0]["entries"][0]["key"].as_str().unwrap().starts_with("opds:work:"));
	assert_eq!(first["hasNextPage"], true);
	let (status, child_manga) = request(
		&t.app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": first["groups"][0]["entries"][0]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{child_manga}");
	let child_id = child_manga["id"].as_i64().expect("child manga id");
	let child_chapters = chapter_ids(&t.app, child_id).await;
	assert_eq!(child_chapters.len(), 1);
	assert_eq!(child_chapters[0].1, "Child Volume");
	let (status, child_pages) = request(
		&t.app,
		"GET",
		&format!("/api/v1/chapter/{}/pages", child_chapters[0].0),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{child_pages}");
	assert_eq!(child_pages[0]["content"]["Url"][0], format!("http://127.0.0.1:{port}/opds/v2/books/child-book/pages/0"));
	let (status, second) = request(&t.app, "GET", &format!("{path}?page=2"), None).await;
	assert_eq!(status, StatusCode::OK, "{second}");
	assert_eq!(second["entries"][0]["key"], "series-beta");
	assert_eq!(second["groups"][0]["title"], "Second child group");
	assert_eq!(second["hasNextPage"], false);

	let (status, all) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/opds:all"), None).await;
	assert_eq!(status, StatusCode::OK, "{all}");
	assert_eq!(all["entries"].as_array().unwrap().len(), 0);
	assert!(all["navigation"].as_array().unwrap().is_empty());
	assert_eq!(all["groups"][0]["navigation"][0]["title"], "Nested shelf");
	assert_eq!(all["groups"][0]["title"], "Featured works");
	assert_eq!(all["groups"][0]["entries"].as_array().unwrap().len(), 2);
	let (status, unknown) = request(
		&t.app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/opds:nav:aHR0cHM6Ly9ldmlsLnRlc3Q"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
	assert!(all["groups"][0]["entries"][0]["key"].as_str().unwrap().starts_with("opds:work:"));
	assert_eq!(all["groups"][0]["entries"][0]["url"], url);
	assert_eq!(
		all["groups"][0]["entries"][0]["cover"],
		format!("http://127.0.0.1:{port}/opds/v2/cover/featured.jpg")
	);

	for (index, title) in ["Featured Book", "Featured Book Two"].iter().enumerate() {
		let (status, manga) = request(
			&t.app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": all["groups"][0]["entries"][index]})),
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{manga}");
		let manga_id = manga["id"].as_i64().expect("manga id");
		if *title == "Featured Book Two" {
			assert!(manga["cover"].is_null(), "standalone without a cover must not inherit a sibling cover: {manga}");
		}
		let chapters = chapter_ids(&t.app, manga_id).await;
		assert_eq!(chapters.len(), 1);
		assert_eq!(chapters[0].1, *title);
		let (status, pages) = request(
			&t.app,
			"GET",
			&format!("/api/v1/chapter/{}/pages", chapters[0].0),
			None,
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{pages}");
		assert_eq!(pages.as_array().unwrap().len(), 2);
	}
}

#[tokio::test]
async fn degraded_search_includes_direct_root_publications() {
	let t = opds_app().await;
	let (port, _server, _) = mock_v2_catalog_mode(false, false, true).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, registered) = request(
		&t.app,
		"POST",
		"/api/v1/sources/internal",
		Some(json!({"type":"opds", "name":"Root book catalog", "url":url})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{registered}");
	let source_id = registered["id"].as_i64().unwrap();
	let (status, result) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query":"Root Only Book"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{result}");
	assert_eq!(result["degraded"], true);
	assert_eq!(result["entries"].as_array().unwrap().len(), 1);
	assert_eq!(result["entries"][0]["title"], "Root Only Book");
}

#[tokio::test]
async fn groups_only_root_lists_and_browses_group_navigation() {
	let t = opds_app().await;
	let (port, _server, _) = mock_v2_catalog_mode(false, true, false).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, registered) = request(
		&t.app,
		"POST",
		"/api/v1/sources/internal",
		Some(json!({"type":"opds", "name":"Groups only", "url":url})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{registered}");
	let source_id = registered["id"].as_i64().unwrap();
	let (status, listings) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::OK, "{listings}");
	assert_eq!(listings.as_array().unwrap().len(), 1);
	assert_eq!(listings[0]["id"], "opds:all");
	let (status, all) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/opds:all"), None).await;
	assert_eq!(status, StatusCode::OK, "{all}");
	assert!(all["entries"].as_array().unwrap().is_empty());
	assert_eq!(all["groups"][0]["entries"].as_array().unwrap().len(), 0);
	assert_eq!(all["groups"][0]["navigation"][0]["title"], "Only child");
	let listing_id = all["groups"][0]["navigation"][0]["listingId"].as_str().unwrap();
	let (status, child) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/{listing_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{child}");
	assert_eq!(child["parentId"], "opds:all");
	assert_eq!(child["entries"][0]["title"], "Nested book");
	assert_eq!(child["navigation"][0]["title"], "Deep child");
	let deep_id = child["navigation"][0]["listingId"].as_str().unwrap();
	assert!(deep_id.starts_with("opds:path:"));
	let (status, deep) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/{deep_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{deep}");
	assert_eq!(deep["parentId"], listing_id);
	assert_eq!(deep["entries"][0]["title"], "Deep book");
}

#[tokio::test]
async fn standalone_locator_survives_catalog_disappearance_for_read_and_refresh() {
	let t = opds_app().await;
	let (port, _server, hide_featured) = mock_v2_catalog_mode(false, false, false).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, registered) = request(
		&t.app,
		"POST",
		"/api/v1/sources/internal",
		Some(json!({"type":"opds", "name":"Standalone catalog", "url":url})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{registered}");
	let source_id = registered["id"].as_i64().unwrap();
	let (status, listings) = request(&t.app, "GET", &format!("/api/v1/sources/{source_id}/listings/opds:all"), None).await;
	assert_eq!(status, StatusCode::OK, "{listings}");
	let work = &listings["groups"][0]["entries"][0];
	let (status, registered_work) = request(&t.app, "POST", "/api/v1/manga", Some(json!({"sourceId":source_id,"manga":work}))).await;
	assert_eq!(status, StatusCode::OK, "{registered_work}");
	let manga_id = registered_work["id"].as_i64().unwrap();
	let chapters = chapter_ids(&t.app, manga_id).await;
	assert_eq!(chapters.len(), 1);
	hide_featured.store(true, std::sync::atomic::Ordering::Relaxed);
	let (status, refreshed) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=true"), None).await;
	assert_eq!(status, StatusCode::OK, "{refreshed}");
	let (status, pages) = request(&t.app, "GET", &format!("/api/v1/chapter/{}/pages", chapters[0].0), None).await;
	assert_eq!(status, StatusCode::OK, "{pages}");
	assert_eq!(pages.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn v2_mislabeled_content_type_still_parses_json() {
	let t = opds_app().await;
	let (port, _server) = mock_v2_catalog(true).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, body) = request(&t.app, "POST", "/api/v1/sources/internal", Some(json!({"type": "opds", "name": "Mislabeled", "url": &url}))).await;
	assert_eq!(status, StatusCode::OK, "register opds instance: {body}");
	let source_id = body["id"].as_i64().expect("source id");

	// every feed arrives with an Atom Content-Type but a JSON body: the sniff keeps the whole flow on the v2 path
	let (status, body) = request(
		&t.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "alpha"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "search: {body}");
	assert_eq!(body["degraded"], false);
	let entries = body["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["key"], "series-alpha");

	let (manga_id, _) = register_alpha_manga(&t.app, source_id).await;
	let chapters = chapter_ids(&t.app, manga_id).await;
	assert_eq!(chapters.len(), 2);
	let (status, body) = request(&t.app, "GET", &format!("/api/v1/chapter/{}/pages", chapters[0].0), None).await;
	assert_eq!(status, StatusCode::OK, "readingOrder pages: {body}");
	let pages = body.as_array().expect("pages array");
	assert_eq!(pages.len(), 2);
	assert_eq!(
		pages[0]["content"]["Url"][0].as_str().expect("page url"),
		format!("http://127.0.0.1:{port}/opds/v2/books/book-1/pages/0")
	);
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/chapter/{}/page/0", chapters[0].0), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes.as_slice(), PAGE_0);
}
