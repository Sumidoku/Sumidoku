//! Alternate cover endpoints: GET /api/v1/manga/{id}/covers/alternates fetches the alternates list
//! from the source; PUT /api/v1/manga/{id}/cover persists the choice as manga.cover_url and
//! invalidates the on-disk cover cache (body + sidecar), so the next GET cover fetches the new URL
//! from the source. The fixture source lives in crates/wasm/tests/fixtures/altcovers-src (no_std
//! cdylib); rebuild with
//! `cd crates/wasm/tests/fixtures/altcovers-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_altcovers.wasm
//! to crates/wasm/tests/fixtures/altcovers/main.wasm.
mod common;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use common::{
	TestApp, altcovers_wasm_path, base_config, connect_db, install_fixture_source,
	pagedesc_wasm_path, request, request_full,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use sumidoku_server::build_app;

// each of the three alternates returns different bytes so assertions can tell which was fetched; the server does not validate image bytes
const PNG_ONE: &[u8] = b"fake-png-one";
const PNG_TWO: &[u8] = b"fake-png-two";
const PNG_THREE: &[u8] = b"fake-png-three";

type SeenPaths = Arc<Mutex<Vec<String>>>;

/// Minimal image host that records request paths: /cover-N.png returns different bytes per N
async fn serve_images() -> (u16, SeenPaths, tokio::task::JoinHandle<()>) {
	let seen: SeenPaths = Arc::new(Mutex::new(Vec::new()));
	let seen_handler = seen.clone();
	let router = Router::new().fallback(move |request: Request| {
		let seen = seen_handler.clone();
		async move {
			let path = request.uri().path().to_string();
			seen.lock().expect("seen lock").push(path.clone());
			let body = match path.as_str() {
				"/cover-1.png" => PNG_ONE,
				"/cover-2.png" => PNG_TWO,
				"/cover-3.png" => PNG_THREE,
				_ => b"unknown",
			};
			([(axum::http::header::CONTENT_TYPE, "image/png")], body.to_vec())
		}
	});
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, seen, handle)
}

/// App with two sources: altcovers (has AlternateCoverProvider) + pagedesc (no export, the 501 control)
async fn dual_source_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.altcovers", "AltCovers Source", &altcovers_wasm_path());
	install_fixture_source(&tmp, "en.pagedesc", "PageDesc Source", &pagedesc_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp::new(app, tmp, db_path)
}

async fn source_id_by_key(app: &Router, key: &str) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("source id")
}

async fn register_manga(app: &Router, source_id: i64) -> i64 {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	body["id"].as_i64().expect("manga id")
}

#[tokio::test]
async fn lists_alternate_covers() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		body,
		json!([
			"https://altcovers.fixture/cover-1.png",
			"https://altcovers.fixture/cover-2.png",
			"https://altcovers.fixture/cover-3.png",
		])
	);
}

#[tokio::test]
async fn selection_persists_and_is_served() {
	let (port, seen, _upstream) = serve_images().await;
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	// point coverBase at the mock image host so the alternates list becomes local URLs that actually
	// serve images (written before registration, so the lazily loaded instance reads it from the DB)
	let base = format!("http://127.0.0.1:{port}");
	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"coverBase": base})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let covers: Vec<String> = serde_json::from_value(body).expect("covers array");
	assert_eq!(
		covers,
		vec![
			format!("{base}/cover-1.png"),
			format!("{base}/cover-2.png"),
			format!("{base}/cover-3.png"),
		]
	);

	// selecting the first one: the fetched image lands in the on-disk cache (body + sidecar)
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": covers[0]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!(covers[0]));
	// Selecting an alternate cover hands the cover column back to the entry's own source:
	// the lock names the source key, so the fetched bytes take the source image chain and a
	// refresh keeps the pick
	assert_eq!(body["metadataLocks"]["cover"]["provider"], "source:en.altcovers");
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/cover"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, PNG_ONE);
	let cache = t.tmp.path().join("covers").join(manga_id.to_string());
	let sidecar = t.tmp.path().join("covers").join(format!("{manga_id}.content-type"));
	assert!(cache.is_file(), "cover cache body should exist");
	assert!(sidecar.is_file(), "cover cache sidecar should exist");

	// switching to the second one: while persisting, the cache is cleared together with the sidecar, and the next GET fetches the new URL from the source
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/manga/{manga_id}/cover"),
		Some(json!({"url": covers[1]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!(covers[1]));
	assert!(!cache.exists(), "stale cover cache should be removed");
	assert!(!sidecar.exists(), "stale sidecar should be removed");
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/cover"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, PNG_TWO);

	let pool = connect_db(&t.db_path).await;
	let (cover_url,): (Option<String>,) = sqlx::query_as("SELECT cover_url FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("cover_url row");
	assert_eq!(cover_url.as_deref(), Some(covers[1].as_str()));

	// each of the two was fetched from the source exactly once: the second is not served from the stale cache
	assert_eq!(
		*seen.lock().expect("seen lock"),
		vec!["/cover-1.png".to_string(), "/cover-2.png".to_string()]
	);
}

#[tokio::test]
async fn select_rejects_bad_cover_url() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.altcovers").await;
	let manga_id = register_manga(&app, source_id).await;

	for url in ["file:///etc/passwd", "not a url", "ftp://127.0.0.1/c.png"] {
		let (status, _) = request(
			&app,
			"PUT",
			&format!("/api/v1/manga/{manga_id}/cover"),
			Some(json!({"url": url})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "url={url}");
	}
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["cover"], json!("https://altcovers.fixture/cover-0.png"));
}

#[tokio::test]
async fn alternates_unsupported_is_501() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.pagedesc").await;
	let manga_id = register_manga(&app, source_id).await;

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/covers/alternates"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
	assert_eq!(body["error"]["code"], json!("unsupported"));
}

#[tokio::test]
async fn unknown_manga_is_404() {
	let t = dual_source_app().await;
	let app = t.app.clone();

	let (status, _) = request(&app, "GET", "/api/v1/manga/9999/covers/alternates", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(
		&app,
		"PUT",
		"/api/v1/manga/9999/cover",
		Some(json!({"url": "https://altcovers.fixture/cover-1.png"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}
