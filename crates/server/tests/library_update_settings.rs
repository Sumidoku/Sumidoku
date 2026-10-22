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
	assert_eq!(body["library"]["skipTitles"], json!(["hasUnread", "completed", "notStarted"]));
	assert_eq!(body["library"]["refreshMetadata"], false);
	assert_eq!(body["library"]["updateIntervalHours"], 36);

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
	assert_eq!(body["library"]["skipTitles"], json!([]));
	assert_eq!(body["library"]["refreshMetadata"], true);
	assert_eq!(body["library"]["updateIntervalHours"], 12);

	let (_, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["library"]["skipTitles"], json!([]));
	assert_eq!(body["library"]["refreshMetadata"], true);
	assert_eq!(body["library"]["updateIntervalHours"], 12);
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
	assert_eq!(body["library"]["skipTitles"], json!([]));
	assert_eq!(body["library"]["refreshMetadata"], true);
	assert_eq!(body["library"]["updateIntervalHours"], 12);
}

#[tokio::test]
async fn structured_settings_preserve_last_run_while_null_clears_overrides() {
	let app = app_with_interval(0).await;
	let (status, mut settings) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({
			"local.oneshots_dir": "oneshots",
			"backup.dir": "custom-backups",
			"backup.last_run": 123,
			"custom.option": {"keep": true}
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{settings}");
	assert_eq!(settings["local"]["oneshotsDir"], "oneshots");
	assert_eq!(settings["backup"]["dir"], "custom-backups");
	assert_eq!(settings["backup"]["lastRun"], 123);

	settings["local"]["oneshotsDir"] = serde_json::Value::Null;
	settings["backup"]["dir"] = serde_json::Value::Null;
	settings["backup"]["lastRun"] = serde_json::Value::Null;
	let (status, body) = request(&app.app, "PUT", "/api/v1/settings", Some(settings)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["local"]["oneshotsDir"], serde_json::Value::Null);
	assert_eq!(body["backup"]["dir"], serde_json::Value::Null);
	assert_eq!(body["backup"]["lastRun"], 123);
	assert_eq!(body["readOnly"]["localOneshotsDir"], serde_json::Value::Null);
	assert_eq!(body["custom.option"], json!({"keep": true}));

	let pool = connect_db(&app.db_path).await;
	let remaining: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM setting WHERE key IN ('local.oneshots_dir', 'backup.dir')",
	)
	.fetch_one(&pool)
	.await
	.expect("cleared overrides");
	assert_eq!(remaining, 0);
	let last_run: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'backup.last_run'")
		.fetch_one(&pool)
		.await
		.expect("preserved last run");
	assert_eq!(last_run, "123");

	let (status, body) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"backup.last_run": null})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["backup"]["lastRun"], serde_json::Value::Null);
	let last_run: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'backup.last_run'")
		.fetch_one(&pool)
		.await
		.expect("legacy last run clear");
	assert_eq!(last_run, "null");
}

#[tokio::test]
async fn structured_settings_require_present_nullable_fields() {
	let app = app_with_interval(0).await;
	let (status, settings) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"local.oneshots_dir": "oneshots", "backup.dir": "custom-backups"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{settings}");

	for (group, field) in [
		("backup", "dir"),
		("backup", "lastRun"),
		("download", "movePath"),
		("local", "oneshotsDir"),
		("readOnly", "localOneshotsDir"),
	] {
		let mut body = settings.clone();
		let removed = body[group]
			.as_object_mut()
			.expect("settings group")
			.remove(field);
		assert!(removed.is_some(), "{group}.{field}");
		let (status, response) = request(&app.app, "PUT", "/api/v1/settings", Some(body)).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "missing {group}.{field}: {response}");
	}

	let (_, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["local"]["oneshotsDir"], "oneshots");
	assert_eq!(body["backup"]["dir"], "custom-backups");
}

#[tokio::test]
async fn structured_settings_reject_partial_or_malformed_groups() {
	let app = app_with_interval(0).await;
	for body in [
		json!({"library": {"updateIntervalHours": 12}}),
		json!({"incognito": {"enabled": "not-a-boolean"}}),
		json!({"download": null}),
	] {
		let (status, response) = request(&app.app, "PUT", "/api/v1/settings", Some(body.clone())).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {response}");
	}

	let (_, body) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["library"]["updateIntervalHours"], 0);
	assert_eq!(body["incognito"]["enabled"], false);
	let pool = connect_db(&app.db_path).await;
	let garbage: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM setting WHERE key IN ('library', 'incognito', 'download')",
	)
	.fetch_one(&pool)
	.await
	.expect("no structured groups stored as flat keys");
	assert_eq!(garbage, 0);
}
