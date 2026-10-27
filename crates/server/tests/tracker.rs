mod common;

use axum::http::StatusCode;
use common::{TestApp, connect_db, request, test_app};
use serde_json::{Value, json};

async fn seed_manga(pool: &sqlx::SqlitePool) -> i64 {
	let source_id: i64 = sqlx::query_scalar(
		"INSERT INTO source (key, name, wasm_path) VALUES ('en.tracker-test', 'Tracker Test', '') RETURNING id",
	)
	.fetch_one(pool)
	.await
	.expect("insert source");
	sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'm1', 'Series') RETURNING id")
		.bind(source_id)
		.fetch_one(pool)
		.await
		.expect("insert manga")
}

#[tokio::test]
async fn tracker_links_crud() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let pool = connect_db(&db_path).await;
	let manga_id = seed_manga(&pool).await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/trackers/links/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!([]));

	for (provider, track_id) in [("anilist", "42"), ("mal", "99")] {
		let (status, body) = request(
			&app,
			"PUT",
			&format!("/api/v1/trackers/links/{manga_id}"),
			Some(json!({
				"provider": provider,
				"trackId": track_id,
				"title": "Series",
				"trackMode": "auto",
				"chapterOffset": 2
			})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(body["provider"], provider);
		assert_eq!(body["trackId"], track_id);
		assert_eq!(body["trackMode"], "auto");
		assert_eq!(body["chapterOffset"], 2);
		assert_eq!(body["mangaId"], manga_id);
	}

	// upsert replaces the same-provider binding
	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/trackers/links/{manga_id}"),
		Some(json!({"provider": "anilist", "trackId": "43", "trackMode": "volume"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["trackId"], "43");
	assert_eq!(body["trackMode"], "volume");
	assert_eq!(body["chapterOffset"], 0);

	let (_, body) = request(&app, "GET", &format!("/api/v1/trackers/links/{manga_id}"), None).await;
	let links = body.as_array().unwrap();
	assert_eq!(links.len(), 2);
	assert_eq!(links[0]["provider"], "anilist");
	assert_eq!(links[0]["trackId"], "43");

	let (_, body) = request(&app, "GET", "/api/v1/trackers/links", None).await;
	assert_eq!(body["entries"].as_array().unwrap().len(), 2);
	assert_eq!(body["hasNextPage"], json!(false));

	let (status, _) = request(&app, "DELETE", &format!("/api/v1/trackers/links/{manga_id}/anilist"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let (_, body) = request(&app, "GET", &format!("/api/v1/trackers/links/{manga_id}"), None).await;
	let links = body.as_array().unwrap();
	assert_eq!(links.len(), 1);
	assert_eq!(links[0]["provider"], "mal");
}

#[tokio::test]
async fn tracker_link_upsert_validates_input() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let pool = connect_db(&db_path).await;
	let manga_id = seed_manga(&pool).await;

	let (status, body) = request(
		&app,
		"PUT",
		&format!("/api/v1/trackers/links/{manga_id}"),
		Some(json!({"provider": "anilist", "trackId": "  "})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert!(body["error"]["message"].as_str().unwrap().contains("trackId"));

	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/trackers/links/{manga_id}"),
		Some(json!({"provider": "anilist", "trackId": "1", "trackMode": "issues"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	let (status, _) = request(
		&app,
		"PUT",
		&format!("/api/v1/trackers/links/{manga_id}"),
		Some(json!({"provider": "anilist", "trackId": "1", "chapterOffset": 10001})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	for provider in ["AniList", "foo"] {
		let (status, _) = request(
			&app,
			"PUT",
			&format!("/api/v1/trackers/links/{manga_id}"),
			Some(json!({"provider": provider, "trackId": "1"})),
		)
		.await;
		assert_eq!(status, StatusCode::NOT_FOUND, "{provider}");
	}
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/trackers/links/{manga_id}/foo"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	let (_, body) = request(&app, "GET", &format!("/api/v1/trackers/links/{manga_id}"), None).await;
	assert_eq!(body, json!([]));
}

/// No account connected: every remote-touching route answers the dedicated
/// 409 tracker_auth, which the web UI turns into a connect prompt instead of
/// a re-login.
#[tokio::test]
async fn remote_routes_require_a_connected_account() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;

	for path in [
		"/api/v1/trackers/anilist/search?name=Series",
		"/api/v1/trackers/mal/state?trackId=42",
		"/api/v1/trackers/bangumi/search?name=Series",
		"/api/v1/trackers/mangabaka/state?trackId=42",
	] {
		let (status, body) = request(&app, "GET", path, None).await;
		assert_eq!(status, StatusCode::CONFLICT, "{path}");
		assert_eq!(body["error"]["code"], "tracker_auth", "{path}");
	}

	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/trackers/anilist/update",
		Some(json!({"trackId": "42", "status": "reading", "lastReadChapter": 9})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "tracker_auth");

	let (_, body) = request(&app, "GET", "/api/v1/trackers/ledger", None).await;
	assert_eq!(body, json!([]));
}

#[tokio::test]
async fn oauth_status_and_start_without_build_credentials() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;

	let (status, body) = request(&app, "GET", "/api/v1/trackers/oauth/anilist/status", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body, json!({"logged_in": false, "username": null}));

	let (status, body) = request(&app, "POST", "/api/v1/trackers/oauth/anilist/logout", None).await;
	assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

	// the OAuth start needs the instance origin for the state payload; a
	// request without any host information is rejected first
	let (status, _) = request(&app, "GET", "/api/v1/trackers/oauth/anilist/start", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// release builds bake the client credentials in; the test binary has none,
	// so start fails with the build-time configuration error
	let (status, body) = request_with_host(&app, "/api/v1/trackers/oauth/anilist/start").await;
	assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
	assert!(body["error"]["message"].as_str().unwrap().contains("SUMIDOKU_OAUTH_ANILIST_CLIENT_ID"));

	let (status, _) = request(&app, "GET", "/api/v1/trackers/oauth/foo/status", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The shared request helper sends no Host header; OAuth routes need one to
/// derive the instance origin.
async fn request_with_host(app: &axum::Router, path: &str) -> (StatusCode, Value) {
	let request = axum::http::Request::builder()
		.method("GET")
		.uri(path)
		.header("host", "sumidoku.test")
		.body(axum::body::Body::empty())
		.unwrap();
	let response = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
	let status = response.status();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
	(status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn tracker_links_are_paginated_and_scoped() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let pool = connect_db(&db_path).await;
	let source_id: i64 = sqlx::query_scalar(
		"INSERT INTO source (key, name, wasm_path) VALUES ('en.tracker-pagination', 'Tracker Pagination', '') RETURNING id",
	)
	.fetch_one(&pool)
	.await
	.unwrap();
	let other_user: i64 = sqlx::query_scalar("INSERT INTO user (username, password_hash) VALUES ('other', 'x') RETURNING id")
		.fetch_one(&pool)
		.await
		.unwrap();
	let mut manga_ids = Vec::new();
	for index in 0..51 {
		let manga_id: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, ?, ?) RETURNING id")
			.bind(source_id)
			.bind(format!("m{index}"))
			.bind(format!("Series {index}"))
			.fetch_one(&pool)
			.await
			.unwrap();
		manga_ids.push(manga_id);
		sqlx::query(
			"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, created_at, updated_at)
			 VALUES (?, 1, 'anilist', ?, 0, 0)",
		)
		.bind(manga_id)
		.bind(index.to_string())
		.execute(&pool)
		.await
		.unwrap();
		if index == 0 {
			sqlx::query(
				"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, created_at, updated_at)
				 VALUES (?, ?, 'bangumi', 'other', 0, 0)",
			)
			.bind(manga_id)
			.bind(other_user)
			.execute(&pool)
			.await
			.unwrap();
		}
	}

	let (status, first) = request(&app, "GET", "/api/v1/trackers/links", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(first["hasNextPage"], json!(true));
	let first_entries = first["entries"].as_array().unwrap();
	assert_eq!(first_entries.len(), 50);
	assert_eq!(first_entries[0]["mangaId"], json!(manga_ids[0]));
	assert!(first_entries.iter().all(|entry| entry["provider"] == "anilist"));

	let (status, second) = request(&app, "GET", "/api/v1/trackers/links?page=2", None).await;
	assert_eq!(status, StatusCode::OK);
	let second_entries = second["entries"].as_array().unwrap();
	assert_eq!(second_entries.len(), 1);
	assert_eq!(second_entries[0]["mangaId"], json!(manga_ids[50]));
	assert_eq!(second["hasNextPage"], json!(false));
	let seen: std::collections::HashSet<i64> = first_entries
		.iter()
		.chain(second_entries.iter())
		.map(|entry| entry["mangaId"].as_i64().unwrap())
		.collect();
	assert_eq!(seen.len(), 51);

	let (status, _) = request(&app, "GET", "/api/v1/trackers/links?page=0", None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn manga_responses_include_only_current_users_tracker_providers() {
	let TestApp { app, tmp: _tmp, db_path } = test_app().await;
	let pool = connect_db(&db_path).await;
	let manga_id = seed_manga(&pool).await;
	let source_id: i64 = sqlx::query_scalar("SELECT source_id FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.unwrap();
	let other_manga: i64 = sqlx::query_scalar("INSERT INTO manga (source_id, key, title) VALUES (?, 'm2', 'Other') RETURNING id")
		.bind(source_id)
		.fetch_one(&pool)
		.await
		.unwrap();
	for id in [manga_id, other_manga] {
		let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
	}
	let other_user: i64 = sqlx::query_scalar("INSERT INTO user (username, password_hash) VALUES ('other', 'x') RETURNING id")
		.fetch_one(&pool)
		.await
		.unwrap();
	for (user_id, provider) in [(1, "bangumi"), (other_user, "anilist")] {
		sqlx::query(
			"INSERT INTO tracker_link (manga_id, user_id, provider, track_id, created_at, updated_at)
			 VALUES (?, ?, ?, 'remote', 0, 0)",
		)
		.bind(manga_id)
		.bind(user_id)
		.bind(provider)
		.execute(&pool)
		.await
		.unwrap();
	}

	let (_, detail) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(detail["trackerProviders"], json!(["bangumi"]));
	let (_, library) = request(&app, "GET", "/api/v1/library?page=1", None).await;
	let entries = library["entries"].as_array().unwrap();
	let linked = entries.iter().find(|entry| entry["id"] == manga_id).unwrap();
	let unlinked = entries.iter().find(|entry| entry["id"] == other_manga).unwrap();
	assert_eq!(linked["trackerProviders"], json!(["bangumi"]));
	assert_eq!(unlinked["trackerProviders"], json!([]));
}
