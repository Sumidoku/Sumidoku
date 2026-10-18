pub mod api;
pub mod config;
pub mod cover;
pub mod db;
pub mod download;
pub mod download_api;
pub mod error;
pub mod library;
pub mod model;
pub mod opds;
pub mod opds2;
pub mod page;
pub mod reader;
pub mod routed;
pub mod service;
pub mod state;
pub mod update;
pub mod update_api;
pub mod webui_assets;

/// 单用户阶段所有用户态操作恒为内置 admin
pub(crate) const USER_ID: i64 = 1;
pub(crate) const PAGE_SIZE: i64 = 50;

use crate::config::Config;
use crate::error::{ApiError, Error};
use axum::Router;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub async fn build_app(config: Config) -> Result<Router, Error> {
	std::fs::create_dir_all(&config.covers_dir)?;
	std::fs::create_dir_all(&config.downloads_dir)?;
	let pool = db::create_pool(&config.database_url).await?;
	let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_defaults_writer(pool.clone(), rx));
	let state = state::AppState::new(pool, config, tx);
	let downloads = download::DownloadManager::new(&state);
	assert!(state.downloads.set(downloads.clone()).is_ok());
	let updater = update::UpdateScheduler::new(&state);
	assert!(state.updater.set(updater.clone()).is_ok());
	downloads.recover().await;
	tokio::spawn(downloads.run());
	tokio::spawn(updater.run());
	let app = api::router(state.clone());
	let app = app
		.nest("/opds/v1.2", opds::router(state.clone()))
		.nest("/opds/v2", opds2::router(state.clone()));
	Ok(webui_fallback(app, &state.config.webui_dir).layer(TraceLayer::new_for_http()))
}

fn webui_fallback(app: Router, webui_dir: &std::path::Path) -> Router {
	if webui_dir.is_dir() {
		return webui_disk_fallback(app, webui_dir);
	}
	if webui_assets::has_index() {
		return app.fallback(webui_embedded_fallback);
	}
	app.route("/", get(|| async { "Sumidoku API server (webui not built)" }))
}

/// 磁盘分支：SUMIDOKU_WEBUI_DIR 显式指向真实目录（开发/Docker 挂载）时优先
fn webui_disk_fallback(app: Router, webui_dir: &std::path::Path) -> Router {
	// not_found_service 会把响应状态码强制改成 404（给 404 页用的），SPA 要 200，用 fallback
	let serve = ServeDir::new(webui_dir).fallback(ServeFile::new(webui_dir.join("index.html")));
	app.fallback(move |req: Request| {
		let serve = serve.clone();
		async move {
			// /api 与 /opds 不许进 SPA fallback，未知 API 路径保持 JSON 404
			let path = req.uri().path();
			if path.starts_with("/api") || path.starts_with("/opds") {
				return ApiError::not_found("route not found").into_response();
			}
			match serve.oneshot(req).await {
				Ok(response) => response.into_response(),
				Err(e) => match e {},
			}
		}
	})
}

async fn webui_embedded_fallback(req: Request) -> Response {
	// /api 与 /opds 不许进 SPA fallback，未知 API 路径保持 JSON 404
	let path = req.uri().path();
	if path.starts_with("/api") || path.starts_with("/opds") {
		return ApiError::not_found("route not found").into_response();
	}
	let Some(normalized) = webui_assets::normalize_embed_path(path) else {
		return ApiError::not_found("route not found").into_response();
	};
	if let Some((mime, body)) = webui_assets::get(&normalized) {
		return webui_assets::embedded_response(&normalized, mime, body);
	}
	// SPA 前端路由：未命中的非 API 路径回退 index.html
	match webui_assets::get("index.html") {
		Some((mime, body)) => webui_assets::embedded_response("index.html", mime, body),
		None => ApiError::not_found("route not found").into_response(),
	}
}
