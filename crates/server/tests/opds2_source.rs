//! OPDS 2.0 (JSON) catalog contract for the built-in OPDS source: per-response format detection (Content-Type
//! plus a JSON body sniff for mislabeled servers), templated search links ({?query}), series drill-down through
//! feed-typed self links, chapter pages enumerated from the publication manifest's readingOrder, and the
//! acquisition CBZ fallback kept from 1.2. The mock mirrors this server's own /opds/v2 output (opds/v2.rs) plus
//! a Komga-style publication document for the page list
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
}

fn root_feed(state: &MockState) -> String {
	let base = &state.base;
	json!({
		"metadata": {"title": "Demo v2 catalog"},
		"links": [
			{"rel": "self", "href": format!("{base}/opds/v2/root"), "type": FEED_TYPE},
			{"rel": "start", "href": format!("{base}/opds/v2/root"), "type": FEED_TYPE},
			{"rel": "search", "href": format!("{base}/opds/v2/search{{?query}}"), "type": FEED_TYPE, "templated": true},
		],
		"navigation": [
			{"title": "All series", "href": format!("{base}/opds/v2/series"), "type": FEED_TYPE},
		],
	})
	.to_string()
}

/// A series is a publication whose self link is feed-typed (this server's own manga_publication shape)
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
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let state = MockState {
		base: format!("http://127.0.0.1:{port}"),
		wrong_type,
	};
	let cbz = std::sync::Arc::new(cbz_bytes());
	let cbz_vol1 = cbz.clone();
	let cbz_vol2 = cbz.clone();
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
			get(|State(state): State<MockState>| async move {
				let body = series_feed(None, &state.base);
				feed_response(&state, body, FEED_TYPE)
			}),
		)
		.route(
			"/opds/v2/search",
			get(
				|State(state): State<MockState>, Query(query): Query<HashMap<String, String>>| async move {
					let body = series_feed(query.get("query").map(String::as_str), &state.base);
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
			"/opds/v2/cover/{id}",
			get(|| async move { binary(ALPHA_COVER, "image/jpeg") }),
		)
		.with_state(state);
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
	let (status, body) = request(&t.app, "POST", "/api/v1/sources/opds", Some(json!({"name": "Demo V2", "url": &url}))).await;
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
async fn v2_mislabeled_content_type_still_parses_json() {
	let t = opds_app().await;
	let (port, _server) = mock_v2_catalog(true).await;
	let url = format!("http://127.0.0.1:{port}/opds/v2/root");
	let (status, body) = request(&t.app, "POST", "/api/v1/sources/opds", Some(json!({"name": "Mislabeled", "url": &url}))).await;
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
