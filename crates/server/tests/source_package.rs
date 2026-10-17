mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

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
				{"type": "multi-select", "key": "ratings", "title": "Ratings", "values": ["safe", "nsfw"], "default": ["safe"]}
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
