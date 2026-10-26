//! Log output. Off Windows this is the plain stdout subscriber it always was;
//! the Windows desktop build (its console detaches before serving) additionally
//! tees to a daily rolling file under `<config_dir>/logs` — the only
//! observability a GUI-launched server has left.

use crate::config::Config;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// The guard keeps the non-blocking file writer alive; main holds it for the
/// process lifetime. None off Windows or when the logs dir is unusable.
pub fn init(config: &Config) -> Option<WorkerGuard> {
	let filter = EnvFilter::try_new(&config.log).unwrap_or_else(|_| EnvFilter::new("info"));
	// cfg! (not #[cfg]) keeps the file-logging path compiled — and unit-testable —
	// on every host; only its execution is Windows-only
	if cfg!(target_os = "windows") {
		return init_with_file(filter, &config.config_dir.join("logs"));
	}
	tracing_subscriber::fmt().with_env_filter(filter).init();
	None
}

fn init_with_file(filter: EnvFilter, logs_dir: &std::path::Path) -> Option<WorkerGuard> {
	use tracing_subscriber::layer::SubscriberExt;
	use tracing_subscriber::util::SubscriberInitExt;

	let stdout = tracing_subscriber::fmt::layer();
	let appender = std::fs::create_dir_all(logs_dir)
		.map_err(|e| e.to_string())
		.and_then(|()| rolling_appender(logs_dir).map_err(|e| e.to_string()));
	match appender {
		Ok(appender) => {
			let (writer, guard) = tracing_appender::non_blocking(appender);
			let file = tracing_subscriber::fmt::layer().with_writer(writer).with_ansi(false);
			tracing_subscriber::registry().with(filter).with(stdout).with(file).init();
			Some(guard)
		}
		Err(e) => {
			// a read-only data root must not take stdout logging down with it
			tracing_subscriber::registry().with(filter).with(stdout).init();
			tracing::warn!("file logging disabled: {e}");
			None
		}
	}
}

// daily rotation with 7 kept files bounds the dir without any retention code
fn rolling_appender(
	logs_dir: &std::path::Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
	tracing_appender::rolling::Builder::new()
		.rotation(tracing_appender::rolling::Rotation::DAILY)
		.filename_prefix("sumidoku")
		.filename_suffix("log")
		.max_log_files(7)
		.build(logs_dir)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Write;

	#[test]
	fn rolling_appender_writes_dated_sumidoku_log() {
		let dir = tempfile::tempdir().expect("tmp");
		let appender = rolling_appender(dir.path()).expect("appender");
		let (mut writer, guard) = tracing_appender::non_blocking(appender);
		writeln!(writer, "hello sumidoku").expect("write");
		drop(writer);
		drop(guard);

		let files: Vec<String> = std::fs::read_dir(dir.path())
			.expect("read dir")
			.map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
			.collect();
		assert_eq!(files.len(), 1, "expected a single log file, got {files:?}");
		assert!(
			files[0].starts_with("sumidoku.") && files[0].ends_with(".log"),
			"unexpected log file name: {}",
			files[0]
		);
		let content = std::fs::read_to_string(dir.path().join(&files[0])).expect("read log");
		assert!(content.contains("hello sumidoku"));
	}
}
