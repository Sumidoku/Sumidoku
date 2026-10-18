mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

// 真实 .aix 布局（aidoku-cli 打包约定）：Payload/source.json 清单 + Payload/main.wasm 载荷
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

	// 注册路径与目录源一致：wasm_path 指 .aix 本体，settings 默认值照样播种
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

	// 实例能从 .aix 拉起：搜索走通即证明 wasm 从包内读出
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

	// .aix 内嵌图标落盘到包旁，图标接口可读
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

// 无 get_settings 导出的载荷：静态 settings.json 必须经回退归一成嵌套形状，
// .aix 包（zip 内 Payload/）与解包目录（同级文件）两条读取路径都要覆盖
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
	assert_eq!(keys, ["en.example-source"]);
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

	// 多语言无 isDefault 落第一个；url 落 info.url（urls 列表首位）
	assert_eq!(seeded["languages"], json!({"StringArray": ["en"]}));
	assert_eq!(seeded["url"], json!({"String": "https://main.example"}));
	assert_eq!(seeded["dataSaver"], json!({"Bool": false}));
	assert_eq!(seeded["quality"], json!({"String": "high"}));
	// select 无显式默认落第一个候选值
	assert_eq!(seeded["mode"], json!({"String": "a"}));
	assert_eq!(seeded["ratings"], json!({"StringArray": ["safe"]}));
	// page 里的 group 也要递归；picker 同 select 落第一个；switch 无默认落 false
	assert_eq!(seeded["noDefault"], json!({"Bool": false}));
	assert_eq!(seeded["theme"], json!({"String": "dark"}));
	assert_eq!(seeded["nested"], json!({"Bool": true}));
	// segment 没有 values，不应凭空播种
	assert!(!seeded.contains_key("seg"));

	// 源设置端点补出合成的语言/基础地址设置，webui 才能显示切换项
	// 顺序对齐 AidokuRunner：extraSettings 在前，语言在 base url 之前
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

	// 该 fixture 没有 settings.json：首次 get_settings 前 source_setting 是空的，
	// 取完设置后源在代码里声明的默认值（此处是 toggle "setting"）应被补种
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
	// example-source 有动态导出（filters: text；listings: listing），静态应排在其前
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
	// 静态在前，动态导出（text/sort/check/...）追加在后
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
	// f32 281.1 直出会打成 281.1000061035156
	assert_eq!(body[0]["chapter_number"], json!(281.1));
}
