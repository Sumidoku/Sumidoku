use suzuri_server::build_app;
use suzuri_server::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
	let config = Config::load();
	tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::try_new(&config.log).unwrap_or_else(|_| EnvFilter::new("info")))
		.init();
	let app = build_app(config.clone()).await.expect("failed to build app");
	let listener = tokio::net::TcpListener::bind(&config.addr)
		.await
		.expect("failed to bind addr");
	tracing::info!(addr = %config.addr, "suzuri-server listening");
	axum::serve(listener, app).await.expect("server error");
}
