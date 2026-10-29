// windows subsystem in release: no console window is ever created and the server
// surfaces through the tray icon (tray.rs); debug builds keep the console so
// `cargo run` retains log output
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

use sumidoku_server::build_app;
use sumidoku_server::config::Config;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let result = run().await;
	#[cfg(target_os = "windows")]
	if let Err(e) = &result {
		sumidoku_server::tray::show_startup_error(e.as_ref());
	}
	result
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
	// before parsing args, so CLI and startup errors print to the invoking
	// terminal despite the windows subsystem
	#[cfg(target_os = "windows")]
	sumidoku_server::tray::attach_parent_console();
	let cli_config_dir = sumidoku_server::desktop::parse_config_dir_args(std::env::args_os().skip(1))?;
	let config = Config::load(cli_config_dir);
	let _log_guard = sumidoku_server::logging::init(&config);
	// build_app opens (creating) the database, so its absence beforehand marks
	// the very first start
	#[cfg(target_os = "windows")]
	let first_run = !config.config_dir.join("sumidoku.db").exists();
	let app = build_app(config.clone())
		.await
		.map_err(|e| format!("failed to build app: {e}"))?;
	let listener = tokio::net::TcpListener::bind(&config.addr)
		.await
		.map_err(|e| format!("failed to bind addr: {e}"))?;
	tracing::info!(addr = %config.addr, "sumidoku-server listening");
	// startup output had its chance to reach an invoking terminal; serving must
	// not die with a terminal the user closes later
	#[cfg(target_os = "windows")]
	{
		sumidoku_server::tray::detach_console();
	}
	let (shutdown_tx, shutdown_rx) = watch::channel(false);
	#[cfg(target_os = "windows")]
	let tray = sumidoku_server::tray::start(
		sumidoku_server::desktop::browser_url(&config.addr),
		sumidoku_server::tray::TrayPaths {
			config_dir: config.config_dir.clone(),
			logs_dir: config.config_dir.join("logs"),
		},
		shutdown_tx.clone(),
	);
	// the web UI is the only interface; only in an interactive session (tray
	// present) is there a desktop to show it on
	#[cfg(target_os = "windows")]
	if first_run && tray.is_some() {
		sumidoku_server::tray::open_in_browser(&sumidoku_server::desktop::browser_url(&config.addr));
	}
	// binding the sender (not `_`) keeps the channel open when no tray took the
	// clone — a dropped sender reads as a shutdown request
	let _keep_open = shutdown_tx;
	let serve_result = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
		.with_graceful_shutdown(shutdown_signal(shutdown_rx))
		.await;
	// the tray stands for a serving server: it goes as soon as serving ends, on
	// the error path too
	#[cfg(target_os = "windows")]
	if let Some(tray) = tray {
		tray.stop();
	}
	serve_result.map_err(|e| format!("server error: {e}"))?;
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
