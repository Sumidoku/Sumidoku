mod common;

use axum::http::StatusCode;
use common::{
	TestApp, install_fixture_source_with_doc, mutation_wasm_path, request, source_by_key, test_app,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct LogBuffer {
	bytes: Arc<Mutex<Vec<u8>>>,
}

impl LogBuffer {
	fn take(&self) -> String {
		let mut guard = self.bytes.lock().expect("log buffer");
		String::from_utf8(std::mem::take(&mut *guard)).expect("utf8 logs")
	}
}

struct LogWriter {
	bytes: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for LogWriter {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.bytes.lock().expect("log buffer").extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl tracing_subscriber::fmt::MakeWriter<'_> for LogBuffer {
	type Writer = LogWriter;

	fn make_writer(&self) -> Self::Writer {
		LogWriter {
			bytes: self.bytes.clone(),
		}
	}
}

#[tokio::test]
async fn static_capabilities_skip_undeclared_dynamic_calls() {
	let logs = LogBuffer::default();
	tracing_subscriber::fmt()
		.with_writer(logs.clone())
		.with_ansi(false)
		.with_max_level(tracing::Level::INFO)
		.try_init()
		.expect("install log subscriber");

	let TestApp { app, tmp, .. } = test_app().await;
	let doc = json!({
		"info": {"id": "en.mutation", "name": "Mutation Source", "version": 1, "languages": ["en"]},
		"listings": [{"id": "static", "name": "Static Picks", "kind": 1}]
	});
	install_fixture_source_with_doc(&tmp, &doc, &mutation_wasm_path());
	let source_dir = tmp.path().join("sources/en.mutation");
	std::fs::write(
		source_dir.join("filters.json"),
		r#"[{"id":"static_text","type":"text","title":"Static"}]"#,
	)
	.expect("filters.json");
	std::fs::write(
		source_dir.join("settings.json"),
		r#"[{"type":"switch","key":"dataSaver","title":"Data Saver","default":true}]"#,
	)
	.expect("settings.json");

	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = source_by_key(&body, "en.mutation")["id"].as_i64().expect("source id");
	let mut observed = Vec::new();

	logs.take();
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/filters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
	assert_eq!(body[0]["id"], "static_text", "{body}");
	observed.push(("GET filters", logs.take()));

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body, json!([{"id": "static", "name": "Static Picks", "kind": "list"}]));
	observed.push(("GET listings", logs.take()));

	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["settings"].as_array().map(Vec::len), Some(1), "{body}");
	assert_eq!(body["settings"][0]["key"], "dataSaver", "{body}");
	assert_eq!(body["values"]["dataSaver"], true, "{body}");
	observed.push(("GET settings", logs.take()));

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/static"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert_eq!(body["error"]["code"], "unsupported", "{body}");
	observed.push(("GET manga list", logs.take()));

	let mut unexpected = Vec::new();
	for (endpoint, text) in observed {
		for op in ["filters", "listings", "settings"] {
			if text.contains(&format!("op={op}")) {
				unexpected.push(format!("{endpoint}: op={op}\n{text}"));
			}
		}
	}
	assert!(
		unexpected.is_empty(),
		"undeclared dynamic source calls:\n{}",
		unexpected.join("\n")
	);
}
