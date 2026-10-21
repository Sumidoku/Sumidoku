mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, connect_db, db_dump, register_example_manga, request, request_full, source_by_key, test_app};
use serde_json::{Value, json};

const SOURCE_KEY: &str = "en.example-source";

/// Fills every section with data on instance A: shelve + categories (with include-in-download flag and ordering), two progress entries, settings, source settings
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
		&format!("/api/v1/categories/{cat_evening}"),
		Some(json!({"includeInDownload": 2})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
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
	let (status, headers, bytes) = request_full(app, "GET", "/api/v1/backup", None).await;
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
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc.clone())).await;
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

	// categories: order, tri-state flags, membership
	let (_, body) = request(&b.app, "GET", "/api/v1/categories", None).await;
	let cats = body.as_array().expect("categories");
	assert_eq!(cats.len(), 2);
	assert_eq!(cats[0]["name"], "Morning");
	assert_eq!(cats[0]["sortOrder"], 0);
	assert_eq!(cats[0]["mangaCount"], 1);
	assert_eq!(cats[1]["name"], "Evening");
	assert!(!cats[1].as_object().unwrap().contains_key("includeInUpdate"));
	assert_eq!(cats[1]["includeInDownload"], 2);
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
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc)).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert_eq!(body["error"]["code"], "bad_request");
	assert_eq!(db_dump(&b.db_path).await, before);
}

#[tokio::test]
async fn import_rejects_unknown_version() {
	let b = fresh_target().await;
	let before = db_dump(&b.db_path).await;
	for doc in [json!({"version": 2, "library": []}), json!({"library": []})] {
		let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc)).await;
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
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc)).await;
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

	let (status, body) = request(&b.app, "POST", "/api/v1/backup?sections=settings", Some(doc.clone())).await;
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

	let (status, _) = request(&b.app, "POST", "/api/v1/backup?sections=bogus", Some(doc)).await;
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
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc)).await;
	assert_eq!(status, StatusCode::OK, "import failed: {body}");
	assert_eq!(body["restored"]["categories"], 2);

	// The Excluded category should be migrated into the per-user excluded_update_categories setting.
	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	let excluded: Vec<i64> = settings["library.excluded_update_categories"]
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
async fn backup_round_trip_maps_exclusions_to_target_category_ids() {
	let a = test_app().await;
	let (status, body) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Excluded"}))).await;
	assert_eq!(status, StatusCode::OK);
	let source_excluded_id = body["id"].as_i64().expect("source category id");
	let (status, _) = request(&a.app, "POST", "/api/v1/categories", Some(json!({"name": "Normal"}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(
		&a.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.excluded_update_categories": [source_excluded_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	let mut doc = export_doc(&a.app).await;
	let categories = doc["categories"].as_array().expect("backup categories");
	let excluded = categories.iter().find(|c| c["name"] == "Excluded").expect("excluded category");
	assert_eq!(excluded["includeInUpdate"], 2);
	let normal = categories.iter().find(|c| c["name"] == "Normal").expect("normal category");
	assert_eq!(normal["includeInUpdate"], 0);
	assert!(doc["settings"].get("library.excluded_update_categories").is_none());

	let b = test_app().await;
	let (status, body) = request(&b.app, "POST", "/api/v1/categories", Some(json!({"name": "Placeholder"}))).await;
	assert_eq!(status, StatusCode::OK);
	let placeholder_id = body["id"].as_i64().expect("placeholder category id");
	assert_eq!(placeholder_id, source_excluded_id);

	// Older documents may still carry the raw list; it must not replace the mapped target ids.
	doc["settings"]
		.as_object_mut()
		.expect("settings object")
		.insert("library.excluded_update_categories".into(), json!([source_excluded_id]));
	let (status, body) = request(&b.app, "POST", "/api/v1/backup", Some(doc)).await;
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
	let (_, settings) = request(&b.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["library.excluded_update_categories"], json!([target_excluded_id]));
}
