//! GET /api/v1/sources/{id}/image image proxy: source image hosts hotlink-protect the browse/home
//! covers, so only the server can fetch them carrying the source-declared request headers. The
//! fixture source lives in crates/wasm/tests/fixtures/imgreq-src (no_std cdylib); rebuild with
//! `cd crates/wasm/tests/fixtures/imgreq-src && cargo build --release --target wasm32-unknown-unknown`,
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_imgreq.wasm
//! to crates/wasm/tests/fixtures/imgreq/main.wasm.
mod common;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, base_config, imgreq_wasm_path, install_fixture_source, mutation_wasm_path, request, request_full};
use std::sync::{Arc, Mutex};
use sumidoku_server::build_app;

// 1x1 transparent PNG, the mock image-host payload
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

// reconciles with REFERER in crates/wasm/tests/fixtures/imgreq-src/src/lib.rs
const FIXTURE_REFERER: &str = "https://imgreq.fixture";

type SeenHeaders = Arc<Mutex<Vec<(String, String)>>>;

/// Minimal image host that records the most recent request headers: responds with the PNG only
async fn serve_image() -> (u16, SeenHeaders, tokio::task::JoinHandle<()>) {
	let seen: SeenHeaders = Arc::new(Mutex::new(Vec::new()));
	let seen_handler = seen.clone();
	let router = Router::new().route(
		"/cover.png",
		get(move |request: Request| {
			let seen = seen_handler.clone();
			async move {
				let mut headers: Vec<(String, String)> = request
					.headers()
					.iter()
					.filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
					.collect();
				headers.sort();
				*seen.lock().expect("seen lock") = headers;
				([(axum::http::header::CONTENT_TYPE, "image/png")], PNG.to_vec())
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let port = listener.local_addr().expect("local addr").port();
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.expect("serve");
	});
	(port, seen, handle)
}

/// App with two sources: mutation (no image_request and no image processor) + imgreq (has image_request):
/// the plain pass-through control requires a source without a processor — with one, the proxy returns
/// processed bytes instead of the upstream originals
async fn dual_source_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &mutation_wasm_path());
	install_fixture_source(&tmp, "en.imgreq", "Imgreq Source", &imgreq_wasm_path());
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

#[tokio::test]
async fn proxies_with_source_request_headers() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.imgreq").await;
	let (port, seen, _upstream) = serve_image().await;

	let (status, headers, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/image?url=http://127.0.0.1:{port}/cover.png"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, PNG);

	let seen = seen.lock().expect("seen lock");
	assert!(
		seen.iter()
			.any(|(k, v)| k == "referer" && v == FIXTURE_REFERER),
		"upstream should see the source-declared Referer, got {seen:?}"
	);
}

#[tokio::test]
async fn proxies_plain_get_without_feature() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.mutation").await;
	let (port, seen, _upstream) = serve_image().await;

	let (status, headers, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/image?url=http://127.0.0.1:{port}/cover.png"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, PNG);

	let seen = seen.lock().expect("seen lock");
	assert!(
		!seen.iter().any(|(k, _)| k == "referer"),
		"plain fetch should not invent a Referer, got {seen:?}"
	);
}

#[tokio::test]
async fn proxies_svg_content_type() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.mutation").await;
	let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>"#.to_vec();
	let (port, _upstream) = common::serve_bytes(vec![("/cover.svg", "image/svg+xml", svg.clone())]).await;
	let (status, headers, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/image?url=http://127.0.0.1:{port}/cover.svg"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/svg+xml");
	assert_eq!(bytes, svg);
}

#[tokio::test]
async fn rejects_non_http_schemes() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let source_id = source_id_by_key(&app, "en.imgreq").await;

	for url in [
		"file:///etc/passwd",
		"data:image/png;base64,AAAA",
		"ftp://127.0.0.1/cover.png",
		"not%20a%20url",
	] {
		let (status, _, _) = request_full(
			&app,
			"GET",
			&format!("/api/v1/sources/{source_id}/image?url={url}"),
			None,
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "url={url}");
	}
}

#[tokio::test]
async fn unknown_source_is_404() {
	let t = dual_source_app().await;
	let app = t.app.clone();
	let (port, _, _upstream) = serve_image().await;

	let (status, _, _) = request_full(
		&app,
		"GET",
		&format!("/api/v1/sources/9999/image?url=http://127.0.0.1:{port}/cover.png"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}
