pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod model;
pub mod service;
pub mod state;

use crate::config::Config;
use crate::error::Error;
use axum::Router;
use tower_http::trace::TraceLayer;

pub async fn build_app(config: Config) -> Result<Router, Error> {
	let pool = db::create_pool(&config.database_url).await?;
	let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
	tokio::spawn(state::run_defaults_writer(pool.clone(), rx));
	let state = state::AppState::new(pool, config, tx);
	Ok(api::router(state).layer(TraceLayer::new_for_http()))
}
