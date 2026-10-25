//! Per-call source logging: each SourceHandle op must emit exactly one structured
//! line per outcome (info on success, warn on failure) with source key, op name,
//! duration and the entries count or sanitized error.
use aidoku_wasm::{
	DirectPolicy, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{Semaphore, mpsc};

// One process-wide subscriber with a shared buffer; the mutex serializes tests so
// cargo test (shared process) and nextest (process per test) both see only their
// own lines
static BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct TestWriter;

impl std::io::Write for TestWriter {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		BUFFER.lock().expect("log buffer").extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

struct MakeTestWriter;

impl tracing_subscriber::fmt::MakeWriter<'_> for MakeTestWriter {
	type Writer = TestWriter;

	fn make_writer(&self) -> TestWriter {
		TestWriter
	}
}

fn install() {
	let _ = tracing_subscriber::fmt()
		.with_writer(MakeTestWriter)
		.with_ansi(false)
		.with_max_level(tracing::Level::INFO)
		.try_init();
}

fn call_lines() -> Vec<String> {
	let text = String::from_utf8(BUFFER.lock().expect("log buffer").clone()).expect("logs are utf8");
	text.lines()
		.filter(|line| line.contains("source call"))
		.map(str::to_owned)
		.collect()
}

fn config(key: &str, fixture: &str) -> SourceConfig {
	SourceConfig {
		key: key.to_string(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("tests/fixtures")
			.join(fixture)
			.join("main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	}
}

async fn spawn(config: SourceConfig) -> SourceHandle {
	let (handle, _) = SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	handle
}

#[tokio::test]
async fn success_logs_source_op_duration_and_entries() {
	let _guard = GUARD.lock().await;
	BUFFER.lock().expect("log buffer").clear();
	install();

	let handle = spawn(config("en.example", "example")).await;
	let result = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect("search");
	assert_eq!(result.entries.len(), 20);

	let lines = call_lines();
	assert_eq!(lines.len(), 1, "expected exactly one source-call line, got: {lines:?}");
	let line = &lines[0];
	assert!(line.contains("INFO"), "success must log at info: {line}");
	assert!(line.contains("source=en.example"), "source key missing: {line}");
	assert!(line.contains("op=search"), "op missing: {line}");
	assert!(line.contains("dur_ms="), "duration missing: {line}");
	assert!(line.contains("entries=20"), "entries count missing: {line}");
}

#[tokio::test]
async fn success_without_meaningful_count_omits_entries() {
	let _guard = GUARD.lock().await;
	BUFFER.lock().expect("log buffer").clear();
	install();

	let handle = spawn(config("en.example", "example")).await;
	handle
		.handle_notification("test".into())
		.await
		.expect("notification");

	let lines = call_lines();
	assert_eq!(lines.len(), 1, "expected exactly one source-call line, got: {lines:?}");
	let line = &lines[0];
	assert!(line.contains("INFO"), "success must log at info: {line}");
	assert!(line.contains("op=notification"), "op missing: {line}");
	assert!(!line.contains("entries"), "notification has no countable result: {line}");
}

#[tokio::test]
async fn failure_logs_warn_with_error() {
	let _guard = GUARD.lock().await;
	BUFFER.lock().expect("log buffer").clear();
	install();

	let handle = spawn(config("en.example", "example")).await;
	let unsupported = handle
		.get_manga_list(
			aidoku::Listing {
				id: "test".into(),
				name: "Test".into(),
				kind: aidoku::ListingKind::Default,
			},
			1,
		)
		.await;
	assert!(unsupported.is_err(), "fixture rejects unknown listings");

	let lines = call_lines();
	assert_eq!(lines.len(), 1, "expected exactly one source-call line, got: {lines:?}");
	let line = &lines[0];
	assert!(line.contains("WARN"), "failure must log at warn: {line}");
	assert!(line.contains("source=en.example"), "source key missing: {line}");
	assert!(line.contains("op=listing"), "op missing: {line}");
	assert!(line.contains("dur_ms="), "duration missing: {line}");
	assert!(line.contains("error="), "error field missing: {line}");
	assert!(line.contains("message: Not supported"), "sanitized error text missing: {line}");
}

#[tokio::test]
async fn streaming_logs_once_on_termination_with_final_count() {
	let _guard = GUARD.lock().await;
	BUFFER.lock().expect("log buffer").clear();
	install();

	let handle = spawn(config("en.partial", "partial")).await;
	let (tx, mut rx) = mpsc::unbounded_channel();
	let result = handle
		.get_search_manga_list_streaming(None, 1, Vec::new(), tx)
		.await
		.expect("search result");
	let mut partials = Vec::new();
	while let Ok(manga) = rx.try_recv() {
		partials.push(manga);
	}
	assert_eq!(partials.len(), 3, "fixture pushes three partials");
	assert_eq!(result.entries.len(), 2, "final state carries two entries");

	let lines = call_lines();
	assert_eq!(lines.len(), 1, "streaming must log once per logical call, got: {lines:?}");
	let line = &lines[0];
	assert!(line.contains("INFO"), "success must log at info: {line}");
	assert!(line.contains("op=search"), "op missing: {line}");
	assert!(line.contains("entries=2"), "final entry count missing: {line}");
}
