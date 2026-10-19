pub mod api;
pub mod auth;
pub mod backup;
pub mod browser_login;
pub mod config;
pub mod db;
pub mod download;
pub mod error;
pub mod model;
pub mod opds;
pub mod rss;
pub mod source;
pub mod state;
pub mod update;
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
	build_app_with(config, None).await
}

/// browser_connector 为 Some 时压过 config.net.browser_ws 的组装（测试注入 mock 用）
pub async fn build_app_with(
	config: Config,
	browser_connector: Option<Box<dyn aidoku_wasm::browser::CdpConnector>>,
) -> Result<Router, Error> {
	// 两个浏览器后端并存没有合并语义，配置错误要在启动时暴露而不是运行时行为漂移
	if config.net.browser_ws.is_some() && config.net.browser_trawl.is_some() {
		return Err(Error::Config(
			"net.browser_ws and net.browser_trawl are mutually exclusive: configure one browser backend".into(),
		));
	}
	std::fs::create_dir_all(&config.covers_dir)?;
	std::fs::create_dir_all(&config.downloads_dir)?;
	std::fs::create_dir_all(config.sources_dir())?;
	let pool = db::create_pool(&config.database_url).await?;
	let (defaults_tx, defaults_rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_defaults_writer(pool.clone(), defaults_rx));
	let (cookie_tx, cookie_rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_cookie_writer(pool.clone(), cookie_rx));
	let state = state::AppState::new(pool, config, defaults_tx, cookie_tx);
	let connector = browser_connector.map_or_else(
		|| {
			state
				.config
				.net
				.browser_ws
				.as_deref()
				.map(|ws| aidoku_wasm::browser::ws_connector_ambient(ws, tokio::runtime::Handle::current()))
		},
		Some,
	);
	let manager = browser_login::BrowserLoginManager::new(connector, browser_login::SESSION_TTL);
	assert!(state.browser_logins.set(manager).is_ok());
	auth::init(&state).await?;
	let downloads = download::DownloadManager::new(&state);
	assert!(state.downloads.set(downloads.clone()).is_ok());
	let updater = update::UpdateScheduler::new(&state);
	assert!(state.updater.set(updater.clone()).is_ok());
	let backups = backup::BackupScheduler::new(state.pool.clone(), state.config.clone());
	downloads.recover().await;
	source::sweep_page_blobs(&state.pool, &state.config.page_blobs_dir()).await;
	tokio::spawn(downloads.run());
	tokio::spawn(updater.run());
	tokio::spawn(backups.run());
	if state.config.repo_refresh_interval_hours > 0 {
		tokio::spawn(source::repo::run_periodic_refresh(
			state.clone(),
			state.config.repo_refresh_interval_hours,
		));
	}
	if state.config.source_idle_evict_minutes > 0 {
		let max_idle =
			std::time::Duration::from_secs(state.config.source_idle_evict_minutes as u64 * 60);
		tokio::spawn(state::run_idle_eviction(
			state.clone(),
			max_idle,
			state::IDLE_SWEEP_INTERVAL,
		));
	}
	if state.config.rss_threshold_mb > 0 {
		// 读不出 RSS 的平台（Windows）监控无从谈起，只记一次日志
		if rss::current_rss().is_some() {
			tokio::spawn(state::run_rss_monitor(
				state.clone(),
				state.config.rss_threshold_mb * 1024 * 1024,
				state::IDLE_SWEEP_INTERVAL,
				rss::current_rss,
			));
		} else {
			tracing::info!("current RSS unreadable on this platform; RSS monitor disabled");
		}
	}
	let app = api::router(state.clone());
	let app = app
		.nest("/opds/v1.2", opds::router(state.clone()))
		.nest("/opds/v2", opds::v2::router(state.clone()));
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
