//! GET /api/v1/sources/{id}/image 图片代理：browse/home 封面的源图床防盗链只能靠服务端
//! 带上源声明的请求头去取。fixture 源在 crates/wasm/tests/fixtures/imgreq-src（no_std cdylib），
//! 重建方式：`cd crates/wasm/tests/fixtures/imgreq-src && cargo build --release --target wasm32-unknown-unknown`，
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_imgreq.wasm
//! 复制为 crates/wasm/tests/fixtures/imgreq/main.wasm。
mod common;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::get;
use common::{TestApp, base_config, imgreq_wasm_path, install_fixture_source, mutation_wasm_path, request, request_full};
use std::sync::{Arc, Mutex};
use sumidoku_server::build_app;

// 1x1 透明 PNG，mock 图床载荷
const PNG: &[u8] = &[
	137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
	0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
	13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

// 与 crates/wasm/tests/fixtures/imgreq-src/src/lib.rs 的 REFERER 对账
const FIXTURE_REFERER: &str = "https://imgreq.fixture";

type SeenHeaders = Arc<Mutex<Vec<(String, String)>>>;

/// 记录最近一次请求头的微型图床：只回 PNG
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

/// mutation（无 image_request 也无图像处理器）+ imgreq（有 image_request）双源的 app：
/// 纯透传对照要求源无处理器——有处理器时代理返回处理后的字节而非上游原字节
async fn dual_source_app() -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	install_fixture_source(&tmp, "en.mutation", "Mutation Source", &mutation_wasm_path());
	install_fixture_source(&tmp, "en.imgreq", "Imgreq Source", &imgreq_wasm_path());
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
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
