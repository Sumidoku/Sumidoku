mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

/// In-memory .aix (real aidoku-cli layout: Payload/source.json + Payload/main.wasm)
fn aix_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
	let mut cursor = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut cursor);
		let options =
			zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
		for (name, data) in entries {
			writer.start_file(name, options).expect("start entry");
			std::io::Write::write_all(&mut writer, data).expect("write entry");
		}
		writer.finish().expect("finish aix");
	}
	cursor.into_inner()
}

fn aix_with_manifest(manifest: &[u8], wasm: &[u8]) -> Vec<u8> {
	aix_bytes(&[("Payload/source.json", manifest), ("Payload/main.wasm", wasm)])
}

fn fixture_wasm() -> Vec<u8> {
	std::fs::read(fixture_dir().join("main.wasm")).expect("fixture wasm")
}

fn multipart_upload(filename: &str, bytes: &[u8]) -> (Vec<u8>, String) {
	let boundary = "aix-import-boundary";
	let mut body = Vec::new();
	body.extend_from_slice(
		format!(
			"--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
		)
		.as_bytes(),
	);
	body.extend_from_slice(bytes);
	body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
	(body, format!("multipart/form-data; boundary={boundary}"))
}

async fn import_aix(app: &axum::Router, filename: &str, bytes: &[u8]) -> (StatusCode, Value) {
	let (body, content_type) = multipart_upload(filename, bytes);
	let (status, _, raw) = request_bytes(app, "POST", "/api/v1/sources/import", body, &content_type).await;
	(status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn sources_dir_entries(app: &TestApp) -> Vec<String> {
	let mut names: Vec<String> = std::fs::read_dir(app.tmp.path().join("sources"))
		.expect("sources dir")
		.map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
		.collect();
	names.sort();
	names
}

/// Searches the source and shelves the first result in the (built-in admin's) library
async fn register_first_manga_in_library(app: &axum::Router, source_id: i64, title: &str) -> (i64, String) {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entry = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.find(|e| e["title"] == title)
		.unwrap_or_else(|| panic!("entry {title}: {body}"))
	.clone();
	let key = entry["key"].as_str().expect("manga key").to_string();
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": entry})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let id = body["id"].as_i64().expect("manga id");
	let (status, body) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	(id, key)
}

#[tokio::test]
async fn import_installs_aix_package() {
	let app = test_app().await;
	let aix = aix_bytes(&[
		(
			"Payload/source.json",
			br#"{
				"info": {
					"id": "en.imported-source", "name": "Imported", "version": 1,
					"url": "https://imported.example", "languages": ["en"], "contentRating": 0
				}
			}"#,
		),
		("Payload/main.wasm", &fixture_wasm()),
		("Payload/icon.png", b"PNG-imported-icon"),
		(
			"Payload/settings.json",
			br#"[{"type": "switch", "key": "dataSaver", "title": "DS", "default": true}]"#,
		),
	]);
	// the uploaded file name does not matter: the landed name comes from the manifest key
	let (status, body) = import_aix(&app.app, "some-download.aix", &aix).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	// the response mirrors source_json (plus the migration fields), so the webui appends it without a refetch
	assert_eq!(body["key"], "en.imported-source", "{body}");
	assert_eq!(body["name"], "Imported", "{body}");
	assert_eq!(body["version"], 1, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");
	assert_eq!(body["migrationWarnings"], json!([]), "{body}");
	let id = body["id"].as_i64().expect("source id");

	// landed as <key>.aix with no temp residue, nothing under the uploaded name
	assert_eq!(sources_dir_entries(&app), ["en.example-source", "en.imported-source.aix", "en.imported-source.png"]);

	// an imported source is attached to no repo (same as a directory sideload); settings defaults seed as on the scan path
	let pool = connect_db(&app.db_path).await;
	let (repo_id, wasm_path): (Option<i64>, String) =
		sqlx::query_as("SELECT repo_id, wasm_path FROM source WHERE key = 'en.imported-source'")
			.fetch_one(&pool)
			.await
			.expect("source row");
	assert_eq!(repo_id, None, "{repo_id:?}");
	assert!(wasm_path.ends_with("en.imported-source.aix"), "{wasm_path}");
	let (seeded,): (String,) = sqlx::query_as(
		"SELECT ss.value FROM source_setting ss JOIN source s ON s.id = ss.source_id
		 WHERE s.key = 'en.imported-source' AND ss.key = 'dataSaver'",
	)
	.fetch_one(&pool)
	.await
	.expect("seeded default");
	assert_eq!(serde_json::from_str::<Value>(&seeded).unwrap(), json!({"Bool": true}));

	// the instance boots from the imported package: a working search proves the wasm was read from inside it
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(!body["entries"].as_array().unwrap().is_empty(), "{body}");

	// the embedded icon serves through the icon endpoint like the scan path's sidecar
	let (status, headers, bytes) =
		request_full(&app.app, "GET", &format!("/api/v1/sources/{id}/icon"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").and_then(|v| v.to_str().ok()),
		Some("image/png")
	);
	assert_eq!(bytes, b"PNG-imported-icon");
}

#[tokio::test]
async fn import_rejects_invalid_packages() {
	let app = test_app().await;

	// not a zip at all
	let (status, body) = import_aix(&app.app, "broken.aix", b"not a zip at all").await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert!(
		body["error"]["message"].as_str().unwrap().contains("invalid archive"),
		"{body}"
	);

	// a zip without the manifest
	let no_manifest = aix_bytes(&[("Payload/main.wasm", &fixture_wasm())]);
	let (status, body) = import_aix(&app.app, "no-manifest.aix", &no_manifest).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert!(
		body["error"]["message"].as_str().unwrap().contains("Payload/source.json"),
		"{body}"
	);

	// a manifest without the wasm payload (this one lands on disk before validation fails, so the residue assertion below also proves the cleanup)
	let no_wasm = aix_bytes(&[("Payload/source.json", br#"{"info": {"id": "en.no-wasm", "version": 1}}"#)]);
	let (status, body) = import_aix(&app.app, "no-wasm.aix", &no_wasm).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	assert!(
		body["error"]["message"].as_str().unwrap().contains("Payload/main.wasm"),
		"{body}"
	);

	// a multipart body with no file field
	let boundary = "aix-import-boundary";
	let body = format!(
		"--{boundary}\r\ncontent-disposition: form-data; name=\"note\"\r\n\r\nhello\r\n--{boundary}--\r\n"
	);
	let (status, _, raw) = request_bytes(
		&app.app,
		"POST",
		"/api/v1/sources/import",
		body.into_bytes(),
		&format!("multipart/form-data; boundary={boundary}"),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let body: Value = serde_json::from_slice(&raw).unwrap();
	assert!(
		body["error"]["message"].as_str().unwrap().contains("missing"),
		"{body}"
	);

	// nothing landed anywhere: no source row, no package file, no temp file, no icon sidecar
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<&str> = body
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|s| s["key"].as_str())
		.collect();
	assert_eq!(keys, ["local.local"]);
	assert_eq!(sources_dir_entries(&app), ["en.example-source"]);
}

#[tokio::test]
async fn import_replaces_same_key_source() {
	let app = test_app().await;
	let v1 = aix_with_manifest(
		br#"{"info": {"id": "en.replace-me", "name": "ReplaceMe", "version": 1, "languages": ["en"]}}"#,
		&fixture_wasm(),
	);
	let (status, body) = import_aix(&app.app, "first.aix", &v1).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 1, "{body}");
	let id = body["id"].as_i64().expect("source id");

	// the same package again refreshes in place: same row, no migration report
	let (status, body) = import_aix(&app.app, "first-again.aix", &v1).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 1, "{body}");
	assert_eq!(body["id"], id, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");
	assert_eq!(body["migrationWarnings"], json!([]), "{body}");

	// a newer version without a breaking change is a plain replace on the same row
	let v2 = aix_with_manifest(
		br#"{"info": {"id": "en.replace-me", "name": "ReplaceMe", "version": 2, "languages": ["en"]}}"#,
		&fixture_wasm(),
	);
	let (status, body) = import_aix(&app.app, "second.aix", &v2).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["id"], id, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");

	let pool = connect_db(&app.db_path).await;
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM source WHERE key = 'en.replace-me'")
		.fetch_one(&pool)
		.await
		.expect("count");
	assert_eq!(count, 1, "replacement never adds a second row");
	assert_eq!(
		sources_dir_entries(&app),
		["en.example-source", "en.replace-me.aix"]
	);
}

/// A cross-breaking replace on a self-migrating package rekeys in place, exactly like the repo update path
#[tokio::test]
async fn import_crossing_breaking_rekeys_in_place() {
	let app = test_app().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	let v1 = aix_with_manifest(
		br#"{"info": {"id": "en.breaking-import", "name": "BreakingImport", "version": 1, "languages": ["en"]}}"#,
		&rekey_wasm,
	);
	let (status, body) = import_aix(&app.app, "v1.aix", &v1).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let id = body["id"].as_i64().expect("source id");
	let (manga_id, old_key) = register_first_manga_in_library(&app.app, id, "Rekey One").await;
	assert_eq!(old_key, "old/manga-1");

	let v2 = aix_with_manifest(
		br#"{"info": {"id": "en.breaking-import", "name": "BreakingImport", "version": 2, "languages": ["en"]},
			"config": {"breakingChangeVersion": 2}}"#,
		&rekey_wasm,
	);
	let (status, body) = import_aix(&app.app, "v2.aix", &v2).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");
	assert_eq!(body["migrationWarnings"], json!([]), "{body}");

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "new/manga-1", "{body}");
	assert_eq!(body["inLibrary"], true, "{body}");
}

/// A cross-breaking replace on a handler-less package only flags migrationRequired, rows untouched
#[tokio::test]
async fn import_crossing_breaking_without_handler_flags_migration() {
	let app = test_app().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	let v1 = aix_with_manifest(
		br#"{"info": {"id": "en.breaking-import", "name": "BreakingImport", "version": 1, "languages": ["en"]}}"#,
		&rekey_wasm,
	);
	let (status, body) = import_aix(&app.app, "v1.aix", &v1).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let id = body["id"].as_i64().expect("source id");
	let (manga_id, _) = register_first_manga_in_library(&app.app, id, "Rekey One").await;

	let mutation_wasm = std::fs::read(mutation_wasm_path()).expect("mutation wasm");
	let v2 = aix_with_manifest(
		br#"{"info": {"id": "en.breaking-import", "name": "BreakingImport", "version": 2, "languages": ["en"]},
			"config": {"breakingChangeVersion": 2}}"#,
		&mutation_wasm,
	);
	let (status, body) = import_aix(&app.app, "v2.aix", &v2).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], true, "{body}");

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "old/manga-1", "{body}");
}

#[tokio::test]
async fn import_body_over_limit_is_413() {
	let app = test_app().await;
	// the route's cap derives from the package entry caps (68 MiB); junk inside the file field trips it mid-stream
	let big = vec![b'x'; 69 * 1024 * 1024];
	let (status, _, raw) = {
		let (body, content_type) = multipart_upload("big.aix", &big);
		request_bytes(&app.app, "POST", "/api/v1/sources/import", body, &content_type).await
	};
	assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{}", String::from_utf8_lossy(&raw));
	assert_eq!(sources_dir_entries(&app), ["en.example-source"]);
}
