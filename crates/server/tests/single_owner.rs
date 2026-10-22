mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{base_config, fixture_dir, request, request_with_headers, source_by_key, test_app};
use serde_json::{Value, json};
use sumidoku_server::build_app;

const ADMIN_PW: &str = "secret";

async fn login(app: &Router, username: &str, password: &str) -> (StatusCode, Value) {
	request(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": username, "password": password})),
	)
	.await
}

async fn as_admin(app: &Router, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
	let (status, _, bytes) = request_with_headers(app, method, path, body, &[("authorization", token)]).await;
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

async fn admin_app() -> (Router, tempfile::TempDir, String) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.auth.password = Some(ADMIN_PW.into());
	let app = build_app(config).await.expect("build app");
	let (status, body) = login(&app, "admin", ADMIN_PW).await;
	assert_eq!(status, StatusCode::OK);
	let token = format!("Bearer {}", body["token"].as_str().expect("token"));
	(app, tmp, token)
}

async fn example_source_id(app: &Router, admin: &str) -> i64 {
	let (status, _) = as_admin(app, admin, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_admin(app, admin, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id")
}

#[tokio::test]
async fn admin_login_and_status_have_no_role() {
	let (app, _tmp, admin) = admin_app().await;
	let (status, body) = login(&app, "admin", ADMIN_PW).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"], json!({"id": 1, "username": "admin"}));

	let (status, body) = as_admin(&app, &admin, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], true);
	assert_eq!(body["user"], json!({"id": 1, "username": "admin"}));
	let (status, body) = request(&app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["user"], Value::Null);

	let open = test_app().await;
	let (status, body) = request(&open.app, "GET", "/api/v1/auth/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["enabled"], false);
	assert_eq!(body["user"], Value::Null);
}

#[tokio::test]
async fn user_management_routes_are_gone() {
	use tower::ServiceExt;

	let (app, _tmp, admin) = admin_app().await;
	for (method, path, body) in [
		("GET", "/api/v1/users", None),
		("POST", "/api/v1/users", Some(json!({"username": "bob", "password": "hunter2"}))),
		("GET", "/api/v1/users/2", None),
		("PATCH", "/api/v1/users/2", Some(json!({"username": "robert"}))),
		("DELETE", "/api/v1/users/2", None),
	] {
		let mut builder = axum::http::Request::builder()
			.method(method)
			.uri(path)
			.header("authorization", admin.as_str());
		if body.is_some() {
			builder = builder.header("content-type", "application/json");
		}
		let request = builder
			.body(match body {
				Some(value) => axum::body::Body::from(serde_json::to_vec(&value).expect("serialize body")),
				None => axum::body::Body::empty(),
			})
			.expect("build request");
		let response = app.clone().oneshot(request).await.expect("response");
		assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
	}
}

#[tokio::test]
async fn admin_settings_and_backup_cover_every_section() {
	let (app, _tmp, admin) = admin_app().await;
	let source_id = example_source_id(&app, &admin).await;
	let (status, body) = as_admin(
		&app,
		&admin,
		"PUT",
		"/api/v1/settings",
		Some(json!({
			"backup.enabled": true,
			"browse.pinned_sources": [],
			"library.skip_titles": [],
			"incognito.enabled": true
		})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["backup"]["enabled"], true);
	assert_eq!(body["incognito"]["enabled"], true);
	let (status, body) = as_admin(
		&app,
		&admin,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"username": "agent", "password": "site-credential"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let (status, body) = as_admin(
		&app,
		&admin,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_key = body["entries"][0]["key"].as_str().expect("manga key").to_string();
	let (status, body) = as_admin(
		&app,
		&admin,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let manga_id = body["id"].as_i64().expect("manga id");
	let (status, body) = as_admin(&app, &admin, "POST", "/api/v1/categories", Some(json!({"name": "Evening"}))).await;
	assert_eq!(status, StatusCode::OK);
	let category_id = body["id"].as_i64().expect("category id");
	let (status, _) = as_admin(
		&app,
		&admin,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [category_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = as_admin(&app, &admin, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapter_id = body[0]["id"].as_i64().expect("chapter id");
	let (status, _) = as_admin(
		&app,
		&admin,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 4})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, mut doc) = as_admin(&app, &admin, "GET", "/api/v1/backups/document", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(doc["library"].as_array().expect("library").len(), 1);
	assert_eq!(doc["categories"].as_array().expect("categories").len(), 1);
	assert_eq!(doc["progress"].as_array().expect("progress").len(), 1);
	assert_eq!(doc["settings"]["backup.enabled"], true);
	assert_eq!(doc["settings"]["browse.pinned_sources"], json!([]));
	assert_eq!(doc["settings"]["library.skip_titles"], json!([]));
	assert_eq!(doc["settings"]["incognito.enabled"], true);
	assert_eq!(doc["sourceSettings"]["en.example-source"]["password"], "site-credential");

	doc["settings"]["backup.enabled"] = json!(false);
	doc["settings"]["incognito.enabled"] = json!(false);
	doc["sourceSettings"]["en.example-source"]["password"] = json!("restored-credential");
	doc["library"][0]["inLibrary"] = json!(false);
	doc["progress"][0]["lastPageRead"] = json!(1);
	let (status, body) = as_admin(&app, &admin, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["restored"]["library"], 1);
	assert_eq!(body["restored"]["categories"], 1);
	assert_eq!(body["restored"]["progress"], 1);
	assert!(body["restored"]["settings"].as_i64().expect("settings count") >= 4);
	assert!(body["restored"]["sourceSettings"].as_i64().expect("source settings count") >= 2);

	let (_, settings) = as_admin(&app, &admin, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["backup"]["enabled"], false);
	assert_eq!(settings["incognito"]["enabled"], false);
	let (_, source_settings) = as_admin(
		&app,
		&admin,
		"GET",
		&format!("/api/v1/sources/{source_id}/settings"),
		None,
	)
	.await;
	assert_eq!(source_settings["values"]["password"], "restored-credential");
	let (_, manga) = as_admin(&app, &admin, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(manga["inLibrary"], false);
	let (_, chapters) = as_admin(&app, &admin, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(chapters[0]["lastPageRead"], 1);
}

#[tokio::test]
async fn openapi_describes_only_the_admin_owner() {
	let (app, _tmp, admin) = admin_app().await;
	let (status, doc) = as_admin(&app, &admin, "GET", "/api/openapi.json", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(doc["paths"].get("/api/v1/users").is_none());
	assert!(doc["paths"].get("/api/v1/users/{id}").is_none());
	assert_eq!(doc["components"]["schemas"]["SyncChange"]["properties"]["originUserId"]["enum"], json!([1]));
	assert!(doc["components"]["responses"].get("Forbidden").is_none());
	let codes = doc["components"]["schemas"]["ApiError"]["properties"]["code"]["enum"]
		.as_array()
		.expect("error codes");
	assert!(!codes.iter().any(|code| code == "forbidden"));
}
