mod common;

use axum::http::StatusCode;
use common::{connect_db, request, test_app};
use serde_json::{Value, json};

#[tokio::test]
async fn download_settings_expose_defaults_and_validate_writes() {
	let app = test_app().await;
	let (status, settings) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	for (key, expected) in [
		("autoDownloadNewChapters", json!(false)),
		("autoDownloadLimit", json!(5)),
		("downloadFormat", json!("cbz")),
		("downloadConcurrency", json!(3)),
		("deleteAfterManualRead", json!(false)),
		("deleteAfterRead", json!(false)),
		("allowDeleteBookmarked", json!(false)),
		("autoSkipMangaWithUnread", json!(true)),
		("autoIgnoreReuploads", json!(false)),
		("downloadAhead", json!(false)),
		("downloadAheadCount", json!(2)),
		("movePath", json!(null)),
	] {
		assert_eq!(settings["download"][key], expected, "{key}");
	}
	assert_eq!(settings["library"]["excludedDownloadCategories"], json!([]));

	let (_, mut structured) = request(&app.app, "GET", "/api/v1/settings", None).await;
	let updated = json!({
		"autoDownloadNewChapters": true,
		"autoDownloadLimit": 7,
		"downloadFormat": "folder",
		"downloadConcurrency": 2,
		"deleteAfterManualRead": true,
		"deleteAfterRead": true,
		"allowDeleteBookmarked": true,
		"autoSkipMangaWithUnread": false,
		"autoIgnoreReuploads": true,
		"downloadAhead": true,
		"downloadAheadCount": 4,
		"movePath": null
	});
	structured["download"] = updated.clone();
	structured["library"]["excludedDownloadCategories"] = json!([1, 2]);
	let (status, settings) = request(&app.app, "PUT", "/api/v1/settings", Some(structured)).await;
	assert_eq!(status, StatusCode::OK, "{settings}");
	for (key, expected) in updated.as_object().expect("updated settings") {
		assert_eq!(settings["download"][key], *expected, "{key}");
	}
	assert_eq!(settings["library"]["excludedDownloadCategories"], json!([1, 2]));
	let pool = connect_db(&app.db_path).await;
	let stored: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'library.excluded_download_categories'")
		.fetch_one(&pool)
		.await
		.expect("excluded categories");
	assert_eq!(serde_json::from_str::<Value>(&stored).expect("stored categories"), json!([1, 2]));
	pool.close().await;

	for (key, value) in [
		("delete_after_manual_read", json!(1)),
		("auto_download_limit", json!(-1)),
		("download_format", json!("zip")),
		("download_concurrency", json!(0)),
		("library.excluded_download_categories", json!("1")),
	] {
		let (status, body) = request(&app.app, "PUT", "/api/v1/settings", Some(json!({key: value}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {body}");
	}
}

#[tokio::test]
async fn download_concurrency_rejects_zero_and_normalizes_stored_zero() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = common::base_config(&tmp);
	config.download_concurrency = 0;
	let db_path = tmp.path().join("test.db");
	let app = sumidoku_server::build_app(config).await.expect("build app");

	let (status, settings) = request(&app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(settings["download"]["downloadConcurrency"], 1);
	let (status, body) = request(&app, "PUT", "/api/v1/settings", Some(json!({"download_concurrency": 0}))).await;
	assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

	let pool = connect_db(&db_path).await;
	sqlx::query("INSERT INTO setting (key, value) VALUES ('download_concurrency', '0')")
		.execute(&pool)
		.await
		.expect("seed legacy zero");
	let (status, settings) = request(&app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(settings["download"]["downloadConcurrency"], 1);
}
