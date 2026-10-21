mod common;

use axum::http::StatusCode;
use common::{TestApp, base_config, connect_db, fixture_dir, request};
use serde_json::json;
use sumidoku_server::build_app;

async fn app_with_interval(update_interval_hours: u64) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.update_interval_hours = update_interval_hours;
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

#[tokio::test]
async fn library_update_settings_expose_effective_values() {
	let app = app_with_interval(36).await;
	let (status, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["library.skip_titles"], json!(["hasUnread", "completed", "notStarted"]));
	assert_eq!(body["library.refresh_metadata"], false);
	assert_eq!(body["library.update_interval_hours"], 36);

	let (status, body) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({
			"library.skip_titles": [],
			"library.refresh_metadata": true,
			"library.update_interval_hours": 12
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["library.skip_titles"], json!([]));
	assert_eq!(body["library.refresh_metadata"], true);
	assert_eq!(body["library.update_interval_hours"], 12);

	let (_, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["library.skip_titles"], json!([]));
	assert_eq!(body["library.refresh_metadata"], true);
	assert_eq!(body["library.update_interval_hours"], 12);
	let pool = connect_db(&app.db_path).await;
	let (global,): (i64,) = sqlx::query_as(
		"SELECT COUNT(*) FROM setting WHERE key IN (
		 'library.skip_titles', 'library.refresh_metadata', 'library.update_interval_hours')",
	)
	.fetch_one(&pool)
	.await
	.expect("global settings");
	assert_eq!(global, 3);
	let (namespaced,): (i64,) = sqlx::query_as(
		"SELECT COUNT(*) FROM setting WHERE key LIKE 'user.1.library.%'
		 AND key IN (
		 'user.1.library.skip_titles', 'user.1.library.refresh_metadata', 'user.1.library.update_interval_hours')",
	)
	.fetch_one(&pool)
	.await
	.expect("namespaced settings");
	assert_eq!(namespaced, 0);
}

#[tokio::test]
async fn library_update_settings_validate_writes() {
	let app = app_with_interval(0).await;
	let (status, body) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({
			"library.skip_titles": [],
			"library.refresh_metadata": true,
			"library.update_interval_hours": 12
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	for value in [json!("hasUnread"), json!(["unknown"]), json!([1]), json!(null)] {
		let (status, body) = request(
			&app.app,
			"PUT",
			"/api/v1/settings",
			Some(json!({"library.skip_titles": value})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}
	for value in [json!(1), json!("true"), json!(null)] {
		let (status, body) = request(
			&app.app,
			"PUT",
			"/api/v1/settings",
			Some(json!({"library.refresh_metadata": value})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}
	for value in [json!(-1), json!(1.5), json!("12"), json!(null)] {
		let (status, body) = request(
			&app.app,
			"PUT",
			"/api/v1/settings",
			Some(json!({"library.update_interval_hours": value})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}

	let (_, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["library.skip_titles"], json!([]));
	assert_eq!(body["library.refresh_metadata"], true);
	assert_eq!(body["library.update_interval_hours"], 12);
}
