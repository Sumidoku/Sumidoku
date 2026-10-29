pub mod api;
pub mod auth;
pub mod backup;
pub mod cache;
pub mod config;
pub mod db;
pub mod desktop;
pub mod download;
pub mod error;
pub mod events;
pub mod internal;
pub mod logging;
pub mod model;
pub mod rss;
pub mod source;
pub mod state;
pub mod sync;
pub mod task;
pub mod tracker;
#[cfg(target_os = "windows")]
pub mod tray;
pub mod update;
pub mod webui_assets;

/// Built-in admin (user 1): scheduled backups always build the full export as it (see backup/mod.rs)
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
use tower_http::trace::{DefaultOnResponse, TraceLayer};

pub async fn build_app(config: Config) -> Result<Router, Error> {
	std::fs::create_dir_all(&config.covers_dir)?;
	std::fs::create_dir_all(&config.downloads_dir)?;
	std::fs::create_dir_all(config.sources_dir())?;
	let pool = db::create_pool(&config.database_url).await?;
	let (defaults_tx, defaults_rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_defaults_writer(pool.clone(), defaults_rx));
	let (cookie_tx, cookie_rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_cookie_writer(pool.clone(), cookie_rx));
	let state = state::AppState::new(pool, config, defaults_tx, cookie_tx);
    std::fs::create_dir_all(state.config.page_blobs_dir())?;
    std::fs::create_dir_all(state.config.config_dir.join("opds-cbz"))?;
    cache::reconcile_all(
        &state.pool,
        &state.config.page_blobs_dir(),
        &state.config.config_dir.join("opds-cbz"),
    )
    .await
    .map_err(|error| Error::Local(error.message().to_string()))?;
    {
        let cache_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(error) = cache::enforce_limit(&cache_state).await {
                    tracing::warn!("chapter cache sweep failed: {}", error.message());
                }
            }
        });
    }
	auth::init(&state).await?;
	internal::local::ensure_default_instance(&state)
		.await
		.map_err(|e| Error::Local(e.message().to_string()))?;
	let downloads = download::DownloadManager::new(&state);
	assert!(state.downloads.set(downloads.clone()).is_ok());
	let updater = update::UpdateScheduler::new(&state);
	assert!(state.updater.set(updater.clone()).is_ok());
	let event_hub = events::EventHub::new();
	assert!(state.events.set(event_hub.clone()).is_ok());
	event_hub.forward_downloads(downloads.clone());
	event_hub.forward_update(updater.clone());
	let tasks = task::TaskManager::new(&state);
	assert!(state.tasks.set(tasks.clone()).is_ok());
	event_hub.forward_tasks(tasks.clone());
	let tracker_services = tracker::TrackerServices::from_db(state.pool.clone());
	let (tracker_tx, tracker_rx) = tokio::sync::mpsc::unbounded_channel();
	assert!(state.tracker_tx.set(tracker_tx).is_ok());
	tracker::sync::consume_events(state.pool.clone(), tracker_services.clone(), tracker_rx);
	assert!(state.tracker.set(std::sync::Arc::new(tracker_services)).is_ok());
	// A task left running by the previous process goes back to pending before the dispatcher starts
	tasks.recover().await?;
	internal::local::scan::register_handlers(&tasks).await;
	tokio::spawn(tasks.run());
	internal::local::scan::enqueue_startup_scan(&state)
		.await
		.map_err(|e| Error::Local(e.message().to_string()))?;
	tokio::spawn(internal::local::scan::run_scheduler(state.clone()));
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
	// Backfill features on old rows: compile and stamp serially in the background without blocking startup
	{
		let pool = state.pool.clone();
		tokio::spawn(async move { source::backfill_source_metadata(&pool).await });
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
		// Probe once at startup: a permanently unreadable RSS would leave the monitor spinning on None forever
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
	let trace = TraceLayer::new_for_http()
		.make_span_with(|request: &Request| {
			tracing::info_span!(
				"http.request",
				method = %request.method(),
				path = %request.uri().path(),
			)
		})
		.on_response(DefaultOnResponse::new().level(tracing::Level::INFO));
	Ok(webui_fallback(app, &state.config.webui_dir).layer(trace))
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

/// Disk branch: wins when SUMIDOKU_WEBUI_DIR explicitly points at a real directory (dev/Docker mount)
fn webui_disk_fallback(app: Router, webui_dir: &std::path::Path) -> Router {
	// not_found_service forces the response status to 404 (meant for 404 pages); the SPA needs 200, so use fallback
	let serve = ServeDir::new(webui_dir).fallback(ServeFile::new(webui_dir.join("index.html")));
	app.fallback(move |req: Request| {
		let serve = serve.clone();
		async move {
			// /api and /opds must not enter the SPA fallback; unknown API paths keep the JSON 404
			let path = req.uri().path();
			if path.starts_with("/api") || path.starts_with("/opds") {
				return ApiError::not_found("route not found").into_response();
			}
			let no_cache = matches!(path, "/sw.js" | "/manifest.webmanifest");
			match serve.oneshot(req).await {
				Ok(response) => {
					let mut response = response.into_response();
					if no_cache {
						response.headers_mut().insert(
							axum::http::header::CACHE_CONTROL,
							axum::http::HeaderValue::from_static("no-cache"),
						);
					}
					response
				}
				Err(e) => match e {},
			}
		}
	})
}

async fn webui_embedded_fallback(req: Request) -> Response {
	// /api and /opds must not enter the SPA fallback; unknown API paths keep the JSON 404
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
	// SPA client-side routing: unmatched non-API paths fall back to index.html
	match webui_assets::get("index.html") {
		Some((mime, body)) => webui_assets::embedded_response("index.html", mime, body),
		None => ApiError::not_found("route not found").into_response(),
	}
}
