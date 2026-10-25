use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let config = Config::load();
	tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::try_new(&config.log).unwrap_or_else(|_| EnvFilter::new("info")))
		.init();
	let app = build_app(config.clone())
		.await
		.map_err(|e| format!("failed to build app: {e}"))?;
	let listener = tokio::net::TcpListener::bind(&config.addr)
		.await
		.map_err(|e| format!("failed to bind addr: {e}"))?;
	tracing::info!(addr = %config.addr, "sumidoku-server listening");
	// the tray Exit action takes this sender over once the tray lands; binding it
	// (not `_`) keeps the channel open — a dropped sender reads as a shutdown request
	let (_shutdown_tx, shutdown_rx) = watch::channel(false);
	axum::serve(listener, app)
		.with_graceful_shutdown(shutdown_signal(shutdown_rx))
		.await
		.map_err(|e| format!("server error: {e}"))?;
	Ok(())
}

async fn shutdown_signal(mut shutdown_rx: watch::Receiver<bool>) {
	tokio::select! {
		_ = tokio::signal::ctrl_c() => {},
		// the channel only ever carries `true`, so any change is a stop request
		_ = shutdown_rx.changed() => {},
	}
	tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::time::Duration;

	#[tokio::test]
	async fn shutdown_signal_waits_for_the_watch_and_returns_once_it_fires() {
		let (tx, rx) = watch::channel(false);
		let signal = tokio::spawn(shutdown_signal(rx));
		for _ in 0..10 {
			tokio::task::yield_now().await;
		}
		assert!(!signal.is_finished(), "resolved without any shutdown request");
		tx.send(true).expect("receiver alive");
		tokio::time::timeout(Duration::from_secs(5), signal)
			.await
			.expect("must resolve once the watch channel fires")
			.expect("shutdown_signal panicked");
	}

	#[tokio::test]
	async fn serve_returns_normally_once_the_watch_fires() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind loopback");
		let (tx, rx) = watch::channel(false);
		let server = tokio::spawn(async move {
			axum::serve(listener, axum::Router::new())
				.with_graceful_shutdown(shutdown_signal(rx))
				.await
		});
		tx.send(true).expect("receiver alive");
		let result = tokio::time::timeout(Duration::from_secs(5), server)
			.await
			.expect("serve must return once the watch channel fires")
			.expect("serve task panicked");
		result.expect("serve must return Ok after graceful shutdown");
	}
}
