mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

// Real .aix layout (aidoku-cli packaging convention): Payload/source.json manifest + Payload/main.wasm payload
fn write_aix(path: &std::path::Path, entries: &[(&str, &[u8])]) {
	let file = std::fs::File::create(path).expect("create aix");
	let mut writer = zip::ZipWriter::new(file);
	let options =
		zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
	for (name, data) in entries {
		writer.start_file(name, options).expect("start entry");
		std::io::Write::write_all(&mut writer, data).expect("write entry");
	}
	writer.finish().expect("finish aix");
}

fn fixture_wasm() -> Vec<u8> {
	std::fs::read(fixture_dir().join("main.wasm")).expect("fixture wasm")
}

fn write_source(dir: &std::path::Path, source_json: &str, settings: Option<&str>, filters: Option<&str>) {
	std::fs::create_dir_all(dir).expect("mkdir");
	std::fs::write(dir.join("source.json"), source_json).expect("source.json");
	std::fs::copy(fixture_dir().join("main.wasm"), dir.join("main.wasm")).expect("main.wasm");
	if let Some(settings) = settings {
		std::fs::write(dir.join("settings.json"), settings).expect("settings.json");
	}
	if let Some(filters) = filters {
		std::fs::write(dir.join("filters.json"), filters).expect("filters.json");
	}
}

async fn source_id_by_key(app: &axum::Router, key: &str) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.and_then(|s| s["id"].as_i64())
		.expect("source id")
}

#[tokio::test]
async fn scan_loads_aix_package() {
	let app = test_app().await;
	write_aix(
		&app.tmp.path().join("sources/en.packed-source-v3.aix"),
		&[
			(
				"Payload/source.json",
				br#"{
					"info": {
						"id": "en.packed-source", "name": "Packed", "version": 3,
						"url": "https://packed.example", "languages": ["en"], "contentRating": 0
					}
				}"#,
			),
			("Payload/main.wasm", &fixture_wasm()),
			("Payload/icon.png", b"PNG-packed-icon"),
			(
				"Payload/settings.json",
				br#"[{"type": "switch", "key": "dataSaver", "title": "DS", "default": true}]"#,
			),
		],
	);
	let (status, body) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(
		body["installed"].as_array().unwrap().contains(&json!("en.packed-source")),
		"{body}"
	);

	// the registration path matches a directory source: wasm_path points at the .aix itself, settings defaults are still seeded
	let pool = connect_db(&app.db_path).await;
	let (wasm_path,): (String,) = sqlx::query_as("SELECT wasm_path FROM source WHERE key = 'en.packed-source'")
		.fetch_one(&pool)
		.await
		.expect("source row");
	assert!(wasm_path.ends_with(".aix"), "{wasm_path}");
	let seeded: Option<(String,)> = sqlx::query_as(
		"SELECT ss.value FROM source_setting ss JOIN source s ON s.id = ss.source_id
		 WHERE s.key = 'en.packed-source' AND ss.key = 'dataSaver'",
	)
	.fetch_optional(&pool)
	.await
	.expect("setting row");
	let seeded: Value = serde_json::from_str(&seeded.expect("seeded default").0).expect("value json");
	assert_eq!(seeded, json!({"Bool": true}));

	// the instance boots from the .aix: a working search proves the wasm was read from inside the package
	let id = source_id_by_key(&app.app, "en.packed-source").await;
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert!(!body["entries"].as_array().unwrap().is_empty(), "{body}");

	// the icon embedded in the .aix lands on disk next to the package, readable through the icon endpoint
	assert_eq!(
		std::fs::read(app.tmp.path().join("sources/en.packed-source-v3.png")).expect("icon written"),
		b"PNG-packed-icon"
	);
	let (status, headers, bytes) =
		request_full(&app.app, "GET", &format!("/api/v1/sources/{id}/icon"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		headers.get("content-type").and_then(|v| v.to_str().ok()),
		Some("image/png")
	);
	assert_eq!(bytes, b"PNG-packed-icon");
}

#[tokio::test]
async fn scan_skips_corrupt_aix() {
	let app = test_app().await;
	std::fs::write(app.tmp.path().join("sources/broken.aix"), b"not a zip at all").expect("write");
	let (status, body) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let skipped = body["skipped"].as_array().unwrap();
	let entry = skipped
		.iter()
		.find(|s| s["dir"] == "broken.aix")
		.unwrap_or_else(|| panic!("broken.aix should be skipped: {body}"));
	assert!(entry["reason"].as_str().unwrap().contains("archive"), "{entry}");
}

// payload without a get_settings export: static settings.json must go through the fallback and be
// normalized to the nested shape, covering both read paths — .aix package (Payload/ inside the zip)
// and unpacked directory (sibling files)
#[tokio::test]
async fn static_settings_fall_back_to_nested_shape() {
	let app = test_app().await;
	let wasm = std::fs::read(mutation_wasm_path()).expect("mutation wasm");
	let settings = br#"[
		{"type": "group", "title": "G", "footer": "F", "items": [
			{"type": "switch", "key": "dataSaver", "title": "DS", "default": true},
			{"type": "select", "key": "quality", "title": "Q", "values": ["low", "high"], "default": "high"}
		]},
		{"type": "select", "key": "mode", "title": "M", "values": ["a", "b"], "requiresFalse": "dataSaver"}
	]"#;

	write_aix(
		&app.tmp.path().join("sources/en.packed-static.aix"),
		&[
			(
				"Payload/source.json",
				br#"{"info":{"id":"en.packed-static","name":"PackedStatic","version":1,"languages":["en"]}}"#,
			),
			("Payload/main.wasm", wasm.as_slice()),
			("Payload/settings.json", &settings[..]),
		],
	);
	let dir = app.tmp.path().join("sources/en.dir-static");
	std::fs::create_dir_all(&dir).expect("mkdir");
	std::fs::write(
		dir.join("source.json"),
		br#"{"info":{"id":"en.dir-static","name":"DirStatic","version":1,"languages":["en"]}}"#,
	)
	.expect("source.json");
	std::fs::write(dir.join("main.wasm"), &wasm).expect("main.wasm");
	std::fs::write(dir.join("settings.json"), settings).expect("settings.json");

	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);

	for key in ["en.packed-static", "en.dir-static"] {
		let id = source_id_by_key(&app.app, key).await;
		let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/settings"), None).await;
		assert_eq!(status, StatusCode::OK, "{key}: {body}");
		let list = body["settings"]
			.as_array()
			.unwrap_or_else(|| panic!("{key}: settings array: {body}"));
		assert_eq!(list[0]["type"], "group", "{key}");
		let items = list[0]["value"]["Group"]["items"].as_array().expect("group items");
		assert_eq!(items[0]["value"]["Toggle"]["default"], true, "{key}");
		assert_eq!(items[1]["value"]["Select"]["values"], json!(["low", "high"]), "{key}");
		assert_eq!(list[1]["requires_false"], "dataSaver", "{key}");
		assert_eq!(body["values"]["dataSaver"], true, "{key}");
		assert_eq!(body["values"]["quality"], "high", "{key}");
	}
}

#[tokio::test]
async fn scan_skips_aix_missing_entries() {
	let app = test_app().await;
	write_aix(
		&app.tmp.path().join("sources/no-manifest.aix"),
		&[("Payload/main.wasm", &fixture_wasm())],
	);
	write_aix(
		&app.tmp.path().join("sources/bad-manifest.aix"),
		&[
			("Payload/source.json", b"{not json"),
			("Payload/main.wasm", &fixture_wasm()),
		],
	);
	write_aix(
		&app.tmp.path().join("sources/no-wasm.aix"),
		&[(
			"Payload/source.json",
			br#"{"info": {"id": "en.no-wasm", "version": 1}}"#,
		)],
	);
	let (status, body) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let skipped = body["skipped"].as_array().unwrap();
	let reason = |name: &str| {
		skipped
			.iter()
			.find(|s| s["dir"] == name)
			.unwrap_or_else(|| panic!("{name} should be skipped: {body}"))["reason"]
			.as_str()
			.unwrap()
			.to_string()
	};
	assert!(reason("no-manifest.aix").contains("Payload/source.json"));
	assert!(reason("bad-manifest.aix").contains("parse"));
	assert!(reason("no-wasm.aix").contains("Payload/main.wasm"));

	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<&str> = body
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|s| s["key"].as_str())
		.collect();
	assert_eq!(keys, ["local.local", "en.example-source"]);
}

#[tokio::test]
async fn scan_seeds_settings_languages_and_base_url() {
	let app = test_app().await;
	write_source(
		&app.tmp.path().join("sources/multi.custom"),
		r#"{
			"info": {
				"id": "multi.custom", "name": "Custom", "version": 1,
				"url": "https://main.example",
				"urls": ["https://m1.example", "https://m2.example"],
				"languages": ["en", "ja"], "contentRating": 0
			},
			"config": {"allowsBaseUrlSelect": true}
		}"#,
		Some(r#"[
			{"type": "group", "title": "G", "items": [
				{"type": "switch", "key": "dataSaver", "title": "Data Saver", "default": false},
				{"type": "select", "key": "quality", "title": "Quality", "values": ["low", "high"], "default": "high"},
				{"type": "select", "key": "mode", "title": "Mode", "values": ["a", "b"]},
				{"type": "multi-select", "key": "ratings", "title": "Ratings", "values": ["safe", "nsfw"], "default": ["safe"]},
				{"type": "switch", "key": "noDefault", "title": "No Default"},
				{"type": "picker", "key": "theme", "title": "Theme", "values": ["dark", "light"]},
				{"type": "segment", "key": "seg", "title": "Seg", "options": ["a", "b"]},
				{"type": "page", "title": "Advanced", "items": [
					{"type": "group", "items": [
						{"type": "switch", "key": "nested", "title": "Nested", "default": true}
					]}
				]}
			]}
		]"#),
		None,
	);
	let (status, body) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body["installed"].as_array().unwrap().contains(&json!("multi.custom")));

	let pool = connect_db(&app.db_path).await;
	let rows: Vec<(String, String)> = sqlx::query_as(
		"SELECT ss.key, ss.value FROM source_setting ss
		 JOIN source s ON s.id = ss.source_id WHERE s.key = 'multi.custom'",
	)
	.fetch_all(&pool)
	.await
	.expect("query settings");
	let seeded: std::collections::HashMap<String, Value> = rows
		.into_iter()
		.map(|(k, v)| (k, serde_json::from_str(&v).expect("value json")))
		.collect();

	// multi-language without isDefault falls back to the first; url falls back to info.url (first of the urls list)
	assert_eq!(seeded["languages"], json!({"StringArray": ["en"]}));
	assert_eq!(seeded["url"], json!({"String": "https://main.example"}));
	assert_eq!(seeded["dataSaver"], json!({"Bool": false}));
	assert_eq!(seeded["quality"], json!({"String": "high"}));
	// a select without an explicit default falls back to the first candidate
	assert_eq!(seeded["mode"], json!({"String": "a"}));
	assert_eq!(seeded["ratings"], json!({"StringArray": ["safe"]}));
	// groups inside pages are recursed too; picker, like select, falls back to the first; switch without a default falls back to false
	assert_eq!(seeded["noDefault"], json!({"Bool": false}));
	assert_eq!(seeded["theme"], json!({"String": "dark"}));
	assert_eq!(seeded["nested"], json!({"Bool": true}));
	// segment has no values, so nothing is seeded out of thin air
	assert!(!seeded.contains_key("seg"));

	// the source settings endpoint fills in the synthesized language/base-url settings so the webui can
	// show the switchers; order matches AidokuRunner: extraSettings first, language before base url
	let id = source_id_by_key(&app.app, "multi.custom").await;
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let list = body["settings"].as_array().expect("settings array");

	let language = &list[0]["value"]["Group"]["items"][0];
	assert_eq!(language["type"], "multi-select", "{body}");
	assert_eq!(language["key"], "languages", "{body}");
	assert_eq!(language["value"]["MultiSelect"]["values"], json!(["en", "ja"]), "{body}");
	assert_eq!(language["value"]["MultiSelect"]["default"], json!(["en"]), "{body}");
	assert_eq!(body["values"]["languages"], json!(["en"]), "{body}");

	let base_url = &list[1]["value"]["Group"]["items"][0];
	assert_eq!(base_url["type"], "select", "{body}");
	assert_eq!(base_url["key"], "url", "{body}");
	assert_eq!(
		base_url["value"]["Select"]["values"],
		json!(["https://main.example", "https://m1.example", "https://m2.example"]),
		"{body}"
	);
	assert_eq!(body["values"]["url"], "https://main.example", "{body}");
}

#[tokio::test]
async fn get_settings_seeds_dynamic_defaults() {
	let app = test_app().await;
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let id = source_id_by_key(&app.app, "en.example-source").await;

	// this fixture has no settings.json: source_setting is empty before the first get_settings;
	// after settings are fetched, the defaults the source declares in code (here the toggle "setting") should be seeded
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/settings"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body["values"]["setting"].is_boolean(), "{body}");

	let pool = connect_db(&app.db_path).await;
	let (value,): (String,) = sqlx::query_as(
		"SELECT ss.value FROM source_setting ss
		 JOIN source s ON s.id = ss.source_id
		 WHERE s.key = 'en.example-source' AND ss.key = 'setting'",
	)
	.fetch_one(&pool)
	.await
	.expect("dynamic default should be seeded");
	let seeded: Value = serde_json::from_str(&value).expect("value json");
	assert!(seeded.get("Bool").and_then(Value::as_bool).is_some(), "{seeded}");
}

#[tokio::test]
async fn filters_and_listings_merge_static_and_dynamic() {
	let app = test_app().await;
	// example-source has dynamic exports (filters: text; listings: listing); static ones must sort ahead of them
	std::fs::write(
		app.tmp.path().join("sources/en.example-source/filters.json"),
		r#"[{"id": "static_text", "type": "text", "title": "Static"}]"#,
	)
	.expect("filters.json");
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let id = source_id_by_key(&app.app, "en.example-source").await;

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/filters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let ids: Vec<&str> = body
		.as_array()
		.expect("filters array")
		.iter()
		.filter_map(|f| f["id"].as_str())
		.collect();
	// static first, dynamic exports (text/sort/check/...) appended after
	assert!(ids.starts_with(&["static_text", "text"]), "{ids:?}");

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/listings"), None).await;
	assert_eq!(status, StatusCode::OK);
	let ids: Vec<&str> = body
		.as_array()
		.expect("listings array")
		.iter()
		.filter_map(|l| l["id"].as_str())
		.collect();
	assert_eq!(ids, ["test", "listing"]);
}

// static filters (filters.json and the source.json `filters` array) are normalized to the dynamic
// export's flat snake_case shape: derive a stable id when missing (id ?? title ?? type), map camelCase
// capability keys, fill missing variant fields with null; dynamic exports pass through verbatim
#[tokio::test]
async fn static_filters_normalize_to_runtime_shape() {
	let app = test_app().await;
	std::fs::write(
		app.tmp.path().join("sources/en.example-source/filters.json"),
		r#"[
			{"type": "select", "title": "Genre", "options": ["A", "B"], "isGenre": true, "usesTagStyle": true},
			{"type": "check", "id": "official", "title": "Official", "canExclude": true, "default": true},
			{"type": "sort", "id": "order", "canAscend": false, "options": ["New", "Hot"], "default": {"index": 0, "ascending": false}},
			{"type": "multi-select", "title": "Tags", "options": ["x"], "canExclude": true,
			 "defaultIncluded": ["x"], "hideFromHeader": true}
		]"#,
	)
	.expect("filters.json");
	write_source(
		&app.tmp.path().join("sources/en.manifest-filters"),
		r#"{
			"info": {"id": "en.manifest-filters", "name": "ManifestFilters", "version": 1, "languages": ["en"]},
			"filters": [{"type": "check", "title": "Hide", "canExclude": true}]
		}"#,
		None,
		None,
	);
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);

	let id = source_id_by_key(&app.app, "en.example-source").await;
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/filters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let list = body.as_array().expect("filters array");

	// static select without an id: id falls back to title, capability keys mapped, missing variant fields filled with null
	assert_eq!(list[0]["id"], "Genre", "{body}");
	assert_eq!(list[0]["is_genre"], true, "{body}");
	assert_eq!(list[0]["uses_tag_style"], true, "{body}");
	assert_eq!(list[0]["options"], json!(["A", "B"]), "{body}");
	assert_eq!(list[0]["ids"], Value::Null, "{body}");
	assert_eq!(list[0]["default"], Value::Null, "{body}");
	assert_eq!(list[0]["hide_from_header"], Value::Null, "{body}");

	// canExclude / canAscend are no longer dropped
	assert_eq!(list[1]["can_exclude"], true, "{body}");
	assert_eq!(list[1]["name"], Value::Null, "{body}");
	assert_eq!(list[2]["can_ascend"], false, "{body}");
	assert_eq!(list[2]["default"], json!({"index": 0, "ascending": false}), "{body}");
	assert_eq!(list[3]["id"], "Tags", "{body}");
	assert_eq!(list[3]["default_included"], json!(["x"]), "{body}");
	assert_eq!(list[3]["default_excluded"], Value::Null, "{body}");
	assert_eq!(list[3]["hide_from_header"], true, "{body}");

	// dynamic exports (the example payload's first exported filter is text) sit verbatim after the static ones
	assert_eq!(list[4]["id"], "text", "{body}");
	assert_eq!(list[4]["type"], "text", "{body}");

	// the source.json `filters` array goes through the same normalizer
	let id = source_id_by_key(&app.app, "en.manifest-filters").await;
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/sources/{id}/filters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let list = body.as_array().expect("filters array");
	assert_eq!(list[0]["id"], "Hide", "{body}");
	assert_eq!(list[0]["can_exclude"], true, "{body}");
	assert_eq!(list[0]["default"], Value::Null, "{body}");
}

#[tokio::test]
async fn chapter_number_serializes_cleanly() {
	let app = test_app().await;
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let source_id = source_id_by_key(&app.app, "en.example-source").await;

	let pool = connect_db(&app.db_path).await;
	sqlx::query("INSERT INTO manga (source_id, key, title) VALUES (?, 'm1', 'Manga')")
		.bind(source_id)
		.execute(&pool)
		.await
		.expect("insert manga");
	let manga_id: i64 = sqlx::query_scalar("SELECT id FROM manga WHERE key = 'm1'")
		.fetch_one(&pool)
		.await
		.expect("manga id");
	sqlx::query("INSERT INTO chapter (manga_id, key, number, source_order) VALUES (?, 'c1', 281.1, 0)")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("insert chapter");

	let (status, body) = request(
		&app.app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/chapters"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	// f32 281.1 printed directly comes out as 281.1000061035156
	assert_eq!(body[0]["chapter_number"], json!(281.1));
}
