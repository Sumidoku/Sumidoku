mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{
	TestApp, base_config, connect_db, db_dump, register_example_manga, request, request_full, request_with_headers,
	source_by_key, test_app,
};
use serde_json::{Value, json};
use sumidoku_server::build_app;

const SOURCE_KEY: &str = "en.example-source";

/// Fills every section with data on instance A: shelve + ordered categories, two progress entries, settings, source settings
async fn seed_full(app: &Router) {
	let manga_id = register_example_manga(app).await;
	let (_, sources) = request(app, "GET", "/api/v1/sources", None).await;
	let example = source_by_key(&sources, SOURCE_KEY);
	let source_id = example["id"].as_i64().expect("source id");

	let (status, body) = request(app, "POST", "/api/v1/categories", Some(json!({"name": "Evening"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_evening = body["id"].as_i64().expect("category id");
	let (status, body) = request(app, "POST", "/api/v1/categories", Some(json!({"name": "Morning"}))).await;
	assert_eq!(status, StatusCode::OK);
	let cat_morning = body["id"].as_i64().expect("category id");
	let (status, _) = request(
		app,
		"PATCH",
		"/api/v1/categories/reorder",
		Some(json!({"ids": [cat_morning, cat_evening]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, _) = request(
		app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id, "categoryIds": [cat_morning]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (_, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	let chapters = body.as_array().expect("chapters");
	assert!(chapters.len() >= 2);
	let ch1 = chapters[0]["id"].as_i64().expect("chapter id");
	let ch2 = chapters[1]["id"].as_i64().expect("chapter id");
	let (status, _) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{ch1}"),
		Some(json!({"lastPageRead": 7, "isBookmarked": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{ch2}"),
		Some(json!({"isRead": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, _) = request(
		app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"backup.test": "on", "backup.num": 42})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		app,
		"PUT",
		&format!("/api/v1/sources/{source_id}/settings"),
		Some(json!({"username": "agent", "password": "s3cret"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
}

/// Exports and validates the response header contract, returning the parsed backup document
async fn export_doc(app: &Router) -> Value {
	let (status, headers, bytes) = request_full(app, "GET", "/api/v1/backups/document", None).await;
	assert_eq!(status, StatusCode::OK);
	let content_type = headers
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.expect("content-type");
	assert!(content_type.starts_with("application/json"), "content-type: {content_type}");
	let disposition = headers
		.get("content-disposition")
		.and_then(|v| v.to_str().ok())
		.expect("content-disposition");
	assert!(
		disposition.starts_with("attachment; filename=\"sumidoku-backup-")
			&& disposition.ends_with(".json\""),
		"content-disposition: {disposition}"
	);
	serde_json::from_slice(&bytes).expect("backup json")
}

/// Fresh instance B: the fixture source is in place (installed via scan), everything else empty
async fn fresh_target() -> TestApp {
	let app = test_app().await;
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	app
}

async fn authenticated_app(password: &str) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.auth.password = Some(password.into());
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

async fn login_token(app: &Router, password: &str) -> String {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/auth/login",
		Some(json!({"username": "admin", "password": password})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["token"].as_str().expect("session token").to_string()
}

async fn create_api_key(app: &Router, session: &str) -> (String, String) {
	let auth = format!("Bearer {session}");
	let (status, _, bytes) = request_with_headers(
		app,
		"POST",
		"/api/v1/auth/api-keys",
		Some(json!({"name": "cli"})),
		&[("authorization", &auth)],
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let body: Value = serde_json::from_slice(&bytes).expect("created key");
	(
		body["key"].as_str().expect("plaintext key").to_string(),
		body["id"].as_str().expect("key fingerprint").to_string(),
	)
}

async fn credential_status(app: &Router, credential: &str) -> StatusCode {
	let auth = format!("Bearer {credential}");
	let (status, _, _) = request_with_headers(app, "GET", "/api/v1/sources", None, &[("authorization", &auth)]).await;
	status
}

#[tokio::test]
async fn backup_keeps_admin_credentials_instance_local() {
	let a = authenticated_app("source-password").await;
	let a_session = login_token(&a.app, "source-password").await;
	let (a_key, a_key_hash) = create_api_key(&a.app, &a_session).await;
	assert_eq!(credential_status(&a.app, &a_session).await, StatusCode::OK);
	assert_eq!(credential_status(&a.app, &a_key).await, StatusCode::OK);

	let a_auth = format!("Bearer {a_session}");
	let (status, _, bytes) = request_with_headers(
		&a.app,
		"GET",
		"/api/v1/backups/document",
		None,
		&[("authorization", &a_auth)],
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let exported = String::from_utf8(bytes).expect("backup json");
	let a_session_hash = sumidoku_server::auth::sha256_hex(&a_session);
	for secret in [a_session.as_str(), a_session_hash.as_str(), a_key.as_str(), a_key_hash.as_str()] {
		assert!(!exported.contains(secret), "backup contains an admin credential");
	}
	let mut doc: Value = serde_json::from_str(&exported).expect("backup document");
	doc["apiKeys"] = json!([{"id": a_key_hash, "name": "cli", "createdAt": 1, "lastUsedAt": null}]);
	doc["authTokens"] = json!([{"tokenHash": a_session_hash, "userId": 1, "createdAt": 1, "expiresAt": 9999999999_i64}]);

	let b = authenticated_app("target-password").await;
	let b_session = login_token(&b.app, "target-password").await;
	let (b_key, b_key_hash) = create_api_key(&b.app, &b_session).await;
	let b_auth = format!("Bearer {b_session}");
	let (status, _, bytes) = request_with_headers(
		&b.app,
		"POST",
		"/api/v1/backups/document",
		Some(doc),
		&[("authorization", &b_auth)],
	)
	.await;
	let body: Value = serde_json::from_slice(&bytes).expect("import response");
	assert_eq!(status, StatusCode::OK, "{body}");

	for credential in [&a_session, &a_key] {
		assert_eq!(credential_status(&b.app, credential).await, StatusCode::UNAUTHORIZED);
	}
	for credential in [&b_session, &b_key] {
		assert_eq!(credential_status(&b.app, credential).await, StatusCode::OK);
	}
	let pool = connect_db(&b.db_path).await;
	let keys: Vec<String> = sqlx::query_scalar("SELECT key_hash FROM api_key ORDER BY key_hash")
		.fetch_all(&pool)
		.await
		.expect("target API keys");
	let sessions: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM auth_token ORDER BY token_hash")
		.fetch_all(&pool)
		.await
		.expect("target sessions");
	assert_eq!(keys, vec![b_key_hash]);
	assert_eq!(sessions, vec![sumidoku_server::auth::sha256_hex(&b_session)]);
	pool.close().await;
}

#[tokio::test]
async fn export_import_round_trip() {
	let a = test_app().await;
	seed_full(&a.app).await;
	let (_, a_library) = request(&a.app, "GET", "/api/v1/library", None).await;
	let a_manga = a_library["entries"][0].clone();
	let a_manga_id = a_manga["id"].as_i64().expect("manga id");
	// the source-side viewer and chapter thumbnails must also round-trip through the backup
	let pool = connect_db(&a.db_path).await;
	sqlx::query("UPDATE manga SET viewer = 2 WHERE id = ?")
		.bind(a_manga_id)
		.execute(&pool)
		.await
		.expect("seed viewer");
	sqlx::query("UPDATE chapter SET thumbnail = 'https://img.example/thumb.png' WHERE manga_id = ?")
		.bind(a_manga_id)
		.execute(&pool)
		.await
		.expect("seed thumbnail");
	let doc = export_doc(&a.app).await;

	assert_eq!(doc["version"], 1);
	assert!(doc["createdAt"].is_i64());
	let doc_source = source_by_key(&doc["sources"], SOURCE_KEY);
	assert_eq!(doc_source["name"], "Example Source");
	assert!(doc_source["repoUrl"].is_null());
	assert_eq!(doc["library"].as_array().expect("library").len(), 1);
	assert_eq!(doc["library"][0]["sourceKey"], SOURCE_KEY);
	assert!(doc.get("syncChanges").is_none());
	assert!(doc["library"][0].get("lastModifiedAt").is_none());
	assert!(doc["library"][0]["inLibraryAt"].is_i64());
	assert_eq!(doc["library"][0]["viewer"], 2);
	assert_eq!(doc["categories"].as_array().expect("categories").len(), 2);
	assert_eq!(doc["progress"].as_array().expect("progress").len(), 2);
	assert_eq!(doc["progress"][0]["chapter"]["language"], "en");
	assert_eq!(
		doc["progress"][0]["chapter"]["thumbnail"],
		"https://img.example/thumb.png"
	);

	let b = fresh_target().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc.clone())).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["warnings"], json!([]));
	let restored = &body["restored"];
	assert_eq!(restored["library"], 1);
	assert_eq!(restored["categories"], 2);
	assert_eq!(restored["progress"], 2);
	assert!(restored["settings"].as_i64().expect("settings count") >= 2);
	assert!(restored["sourceSettings"].as_i64().expect("source settings count") >= 2);

	// library and manga metadata
	let (_, b_library) = request(&b.app, "GET", "/api/v1/library", None).await;
	let entries = b_library["entries"].as_array().expect("entries");
	assert_eq!(entries.len(), 1);
	let b_manga_id = entries[0]["id"].as_i64().expect("manga id");
	assert_eq!(entries[0]["title"], a_manga["title"]);
	assert_eq!(entries[0]["authors"], a_manga["authors"]);
	assert_eq!(entries[0]["tags"], a_manga["tags"]);
	assert_eq!(entries[0]["viewer"], "RightToLeft");
	assert_eq!(entries[0]["inLibraryAt"], doc["library"][0]["inLibraryAt"]);
	let b_pool = connect_db(&b.db_path).await;
	let tracking: (i64, i64) = sqlx::query_as(
		"SELECT last_modified_at,
		 (SELECT COUNT(*) FROM sync_change WHERE entity_kind = 'manga_user' AND entity_key = ?)
		 FROM manga_user WHERE manga_id = ? AND user_id = 1",
	)
	.bind(format!("{b_manga_id}:1"))
	.bind(b_manga_id)
	.fetch_one(&b_pool)
	.await
	.expect("restored tracking");
	assert!(tracking.0 >= 1_000_000_000_000);
	assert_eq!(tracking.1, 1);
	b_pool.close().await;

	// categories: order, public shape, membership
	let (_, body) = request(&b.app, "GET", "/api/v1/categories", None).await;
	let cats = body.as_array().expect("categories");
	assert_eq!(cats.len(), 2);
	assert_eq!(cats[0]["name"], "Morning");
	assert_eq!(cats[0]["sortOrder"], 0);
	assert_eq!(cats[0]["mangaCount"], 1);
	assert_eq!(cats[1]["name"], "Evening");
	assert!(!cats[1].as_object().unwrap().contains_key("includeInUpdate"));
	assert!(!cats[1].as_object().unwrap().contains_key("includeInDownload"));
	assert_eq!(cats[1]["mangaCount"], 0);

	// progress is reattached by chapter key (the fresh instance has only these two chapter rows)
	let (_, a_chapters) = request(
		&a.app,
		"GET",
		&format!("/api/v1/manga/{a_manga_id}/chapters"),
		None,
	)
	.await;
	let a_chapters = a_chapters.as_array().expect("a chapters");
	let progressed: Vec<&Value> = a_chapters
		.iter()
		.filter(|c| c["lastPageRead"] != 0 || c["isRead"] == true || c["isBookmarked"] == true)
		.collect();
	assert_eq!(progressed.len(), 2);
	let (_, b_chapters) = request(
		&b.app,
		"GET",
		&format!("/api/v1/manga/{b_manga_id}/chapters"),
		None,
	)
	.await;
	let b_chapters = b_chapters.as_array().expect("b chapters");
	assert_eq!(b_chapters.len(), 2);
	for a_ch in progressed {
		let b_ch = b_chapters
			.iter()
			.find(|c| c["key"] == a_ch["key"])
			.expect("chapter by key");
		assert_eq!(b_ch["title"], a_ch["title"]);
		assert_eq!(b_ch["thumbnail"], a_ch["thumbnail"]);
		assert_eq!(b_ch["language"], a_ch["language"]);
		assert_eq!(b_ch["lastPageRead"], a_ch["lastPageRead"]);
		assert_eq!(b_ch["isRead"], a_ch["isRead"]);
		assert_eq!(b_ch["isBookmarked"], a_ch["isBookmarked"]);
		assert_eq!(b_ch["lastReadAt"], a_ch["lastReadAt"]);
	}

	// app settings and source settings (including credentials)
	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["backup.test"], "on");
	assert_eq!(settings["backup.num"], 42);
	let (_, sources) = request(&b.app, "GET", "/api/v1/sources", None).await;
	let b_source_id = source_by_key(&sources, SOURCE_KEY)["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		&b.app,
		"GET",
		&format!("/api/v1/sources/{b_source_id}/settings"),
		None,
	)
	.await;
	assert_eq!(body["values"]["username"], "agent");
	assert_eq!(body["values"]["password"], "s3cret");

	let before = db_dump(&b.db_path).await;
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc.clone())).await;
	assert_eq!(status, StatusCode::OK, "identical restore failed: {body}");
	assert_eq!(db_dump(&b.db_path).await, before);
}

#[tokio::test]
async fn import_rolls_back_on_invalid_row() {
	let a = test_app().await;
	seed_full(&a.app).await;
	let mut doc = export_doc(&a.app).await;
	let b = fresh_target().await;
	let before = db_dump(&b.db_path).await;

	// the last category's include-in-download value is out of range: it only trips mid-write, verifying the whole import rolls back
	doc["categories"].as_array_mut().expect("categories").push(json!({
		"name": "Broken",
		"sortOrder": 9,
		"isDefault": false,
		"includeInDownload": 7,
		"manga": []
	}));
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert_eq!(body["error"]["code"], "bad_request");
	assert_eq!(db_dump(&b.db_path).await, before);
}

#[tokio::test]
async fn import_rejects_unknown_version() {
	let b = fresh_target().await;
	let before = db_dump(&b.db_path).await;
	for doc in [json!({"version": 2, "library": []}), json!({"library": []})] {
		let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
		assert_eq!(status, StatusCode::BAD_REQUEST);
		assert_eq!(body["error"]["code"], "bad_request");
	}
	assert_eq!(db_dump(&b.db_path).await, before);
}

#[tokio::test]
async fn import_skips_missing_sources_with_warnings() {
	let a = test_app().await;
	seed_full(&a.app).await;
	let mut doc = export_doc(&a.app).await;
	let chapter = json!({
		"key": "c1", "title": null, "number": null, "volume": null, "scanlators": [],
		"language": null, "url": null, "dateUploaded": null, "sourceOrder": 0, "pageCount": 0
	});
	doc["library"].as_array_mut().expect("library").push(json!({
		"sourceKey": "en.ghost-source", "key": "ghost-manga", "title": "Ghost",
		"authors": [], "artists": [], "description": null, "tags": [],
		"status": 0, "contentRating": 0, "coverUrl": null, "url": null,
		"initialized": false, "updateStrategy": 0,
		"inLibrary": true, "inLibraryAt": 1700000000, "viewerFlags": 0, "sortKey": ""
	}));
	doc["progress"].as_array_mut().expect("progress").push(json!({
		"sourceKey": "en.ghost-source", "mangaKey": "ghost-manga", "chapter": chapter,
		"lastPageRead": 3, "isRead": false, "isBookmarked": false, "lastReadAt": null
	}));
	doc["progress"].as_array_mut().expect("progress").push(json!({
		"sourceKey": SOURCE_KEY, "mangaKey": "manga-not-in-backup", "chapter": chapter,
		"lastPageRead": 3, "isRead": false, "isBookmarked": false, "lastReadAt": null
	}));
	doc["sourceSettings"]
		.as_object_mut()
		.expect("sourceSettings")
		.insert("en.ghost-source".into(), json!({"password": "x"}));

	let b = fresh_target().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	let warnings: Vec<&str> = body["warnings"]
		.as_array()
		.expect("warnings")
		.iter()
		.map(|w| w.as_str().expect("warning text"))
		.collect();
	assert_eq!(warnings.len(), 4, "warnings: {warnings:?}");
	assert_eq!(warnings.iter().filter(|w| w.contains("en.ghost-source")).count(), 3);
	assert!(warnings.iter().any(|w| w.contains("manga-not-in-backup")));
	assert_eq!(body["restored"]["library"], 1);
	assert_eq!(body["restored"]["progress"], 2);

	// valid entries are restored as usual
	let (_, lib) = request(&b.app, "GET", "/api/v1/library", None).await;
	assert_eq!(lib["entries"].as_array().expect("entries").len(), 1);
}

#[tokio::test]
async fn import_honors_section_selection() {
	let a = test_app().await;
	seed_full(&a.app).await;
	let doc = export_doc(&a.app).await;
	let b = fresh_target().await;

	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document?sections=settings", Some(doc.clone())).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body["restored"]["settings"].as_i64().expect("settings count") >= 2);
	assert_eq!(body["restored"]["library"], 0);
	assert_eq!(body["restored"]["categories"], 0);
	assert_eq!(body["restored"]["progress"], 0);
	assert_eq!(body["restored"]["sourceSettings"], 0);
	let (_, lib) = request(&b.app, "GET", "/api/v1/library", None).await;
	assert_eq!(lib["entries"].as_array().expect("entries").len(), 0);
	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["backup.test"], "on");

	let (status, _) = request(&b.app, "POST", "/api/v1/backups/document?sections=bogus", Some(doc)).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_migrates_legacy_include_in_update_exclude() {
	let a = test_app().await;
	let _manga_id = register_example_manga(&a.app).await;

	// Create categories and simulate the legacy include_in_update = 2 flag directly in the DB,
	// since the API no longer exposes it.
	let (status, body) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Excluded"}))).await;
	assert_eq!(status, StatusCode::OK);
	let excluded_id = body["id"].as_i64().expect("category id");
	let (status, body) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Normal"}))).await;
	assert_eq!(status, StatusCode::OK);
	let _normal_id = body["id"].as_i64().expect("category id");

	let pool = connect_db(&a.db_path).await;
	sqlx::query("UPDATE category SET include_in_update = 2 WHERE id = ?")
		.bind(excluded_id)
		.execute(&pool)
		.await
		.expect("set legacy exclude flag");

	// Build a legacy backup document that still carries includeInUpdate.
	let doc = json!({
		"version": 1,
		"createdAt": 1700000000,
		"sources": [],
		"library": [],
		"categories": [
			{
				"name": "Excluded",
				"sortOrder": 0,
				"isDefault": false,
				"includeInUpdate": 2,
				"includeInDownload": 0,
				"manga": []
			},
			{
				"name": "Normal",
				"sortOrder": 1,
				"isDefault": false,
				"includeInUpdate": 0,
				"includeInDownload": 0,
				"manga": []
			}
		],
		"progress": [],
		"settings": {},
		"sourceSettings": {}
	});

	let b = fresh_target().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["restored"]["categories"], 2);

	// The Excluded category should be migrated into the per-user excluded_update_categories setting.
	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	let excluded: Vec<i64> = settings["library"]["excludedUpdateCategories"]
		.as_array()
		.expect("excluded categories array")
		.iter()
		.map(|v| v.as_i64().expect("category id"))
		.collect();
	assert_eq!(excluded, vec![excluded_id]);

	// The restored categories should not expose the deprecated field.
	let (_, cats) = request(&b.app, "GET", "/api/v1/categories", None).await;
	let excluded_cat = cats.as_array().unwrap().iter().find(|c| c["name"] == "Excluded").unwrap();
	assert!(!excluded_cat.as_object().unwrap().contains_key("includeInUpdate"));
	let normal_cat = cats.as_array().unwrap().iter().find(|c| c["name"] == "Normal").unwrap();
	assert!(!normal_cat.as_object().unwrap().contains_key("includeInUpdate"));
}

#[tokio::test]
async fn import_legacy_download_include_only_disables_auto_download() {
	let a = test_app().await;
	let mut doc = export_doc(&a.app).await;
	doc["categories"] = json!([{
		"name": "Include only",
		"sortOrder": 0,
		"isDefault": false,
		"includeInUpdate": 0,
		"includeInDownload": 1,
		"manga": []
	}]);
	doc["settings"]["auto_download_new_chapters"] = json!(true);

	let b = fresh_target().await;
	let pool = connect_db(&b.db_path).await;
	sqlx::query("INSERT INTO setting (key, value) VALUES ('auto_download_new_chapters', 'true')")
		.execute(&pool)
		.await
		.expect("seed target auto download");
	pool.close().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["download"]["autoDownloadNewChapters"], false);
}

#[tokio::test]
async fn backup_round_trip_maps_exclusions_to_target_category_ids() {
	let a = test_app().await;
	let (status, body) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Excluded"}))).await;
	assert_eq!(status, StatusCode::OK);
	let source_excluded_id = body["id"].as_i64().expect("source category id");
	let (status, _) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Normal"}))).await;
	assert_eq!(status, StatusCode::OK);
	let pool = connect_db(&a.db_path).await;
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES
		 ('user.1.library.excluded_update_categories', ?),
		 ('user.1.library.excluded_download_categories', ?)",
	)
	.bind(format!("[{source_excluded_id}]"))
	.bind(format!("[{source_excluded_id}]"))
	.execute(&pool)
	.await
	.expect("seed source exclusions");
	pool.close().await;

	let mut doc = export_doc(&a.app).await;
	let categories = doc["categories"].as_array().expect("backup categories");
	let excluded = categories.iter().find(|c| c["name"] == "Excluded").expect("excluded category");
	assert_eq!(excluded["includeInUpdate"], 2);
	assert_eq!(excluded["includeInDownload"], 2);
	let normal = categories.iter().find(|c| c["name"] == "Normal").expect("normal category");
	assert_eq!(normal["includeInUpdate"], 0);
	assert_eq!(normal["includeInDownload"], 0);
	assert!(doc["settings"].get("library.excluded_update_categories").is_none());
	assert!(doc["settings"].get("library.excluded_download_categories").is_none());

	let b = test_app().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/categories", Some(json!({"name": "Placeholder"}))).await;
	assert_eq!(status, StatusCode::OK);
	let placeholder_id = body["id"].as_i64().expect("placeholder category id");
	assert_eq!(placeholder_id, source_excluded_id);
	let pool = connect_db(&b.db_path).await;
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES
		 ('user.1.library.excluded_update_categories', ?),
		 ('user.1.library.excluded_download_categories', ?)",
	)
	.bind(format!("[{placeholder_id}]"))
	.bind(format!("[{placeholder_id}]"))
	.execute(&pool)
	.await
	.expect("seed target exclusions");
	pool.close().await;

	// Older documents may still carry raw lists; they must not replace mapped ids or existing exclusions.
	let settings = doc["settings"].as_object_mut().expect("settings object");
	settings.insert("library.excluded_update_categories".into(), json!([source_excluded_id]));
	settings.insert("library.excluded_download_categories".into(), json!([source_excluded_id]));
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let (_, categories) = request(&b.app, "GET", "/api/v1/categories", None).await;
	let target_excluded_id = categories
		.as_array()
		.expect("target categories")
		.iter()
		.find(|c| c["name"] == "Excluded")
		.expect("restored excluded category")["id"]
		.as_i64()
		.expect("target category id");
	assert_ne!(target_excluded_id, source_excluded_id);
	let pool = connect_db(&b.db_path).await;
	for key in [
		"user.1.library.excluded_update_categories",
		"user.1.library.excluded_download_categories",
	] {
		let raw: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
			.bind(key)
			.fetch_one(&pool)
			.await
			.expect("restored exclusions");
		let ids: Vec<i64> = serde_json::from_str(&raw).expect("exclusion ids");
		assert_eq!(ids, vec![placeholder_id, target_excluded_id], "{key}");
	}
	let raw_settings: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM setting WHERE key IN (
		 'library.excluded_update_categories', 'library.excluded_download_categories')",
	)
	.fetch_one(&pool)
	.await
	.expect("raw settings are ignored");
	assert_eq!(raw_settings, 0);
	let legacy: i64 = sqlx::query_scalar("SELECT include_in_download FROM category WHERE id = ?")
		.bind(target_excluded_id)
		.fetch_one(&pool)
		.await
		.expect("legacy download flag");
	assert_eq!(legacy, 0);
	pool.close().await;
}

#[tokio::test]
async fn sync_instance_id_stays_out_of_public_settings_and_backups() {
	let a = test_app().await;
	let mut doc = export_doc(&a.app).await;
	assert!(doc["settings"].get("sync.instance_id").is_none());
	doc["settings"]
		.as_object_mut()
		.expect("settings object")
		.insert("sync.instance_id".into(), json!("peer-instance"));

	let b = fresh_target().await;
	let pool = connect_db(&b.db_path).await;
	let before: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("target instance id");
	let (status, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(settings.get("sync.instance_id").is_none());
	let (status, _) = request(
		&b.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"sync.instance_id": "public-overwrite"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&b.app, "POST", "/api/v1/backups/document", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let after: String = sqlx::query_scalar("SELECT value FROM setting WHERE key = 'sync.instance_id'")
		.fetch_one(&pool)
		.await
		.expect("restored instance id");
	assert_eq!(after, before);
}
