//! Metadata matching: trackers and other sources supply metadata, field locks record who owns what, and the
//! owning source refresh must leave locked fields alone.

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, register_example_manga, request, source_by_key, test_app};
use serde_json::{Value, json};
use sumidoku_server::sync::changes_since;

const SOURCE_KEY: &str = "en.example-source";

async fn example_source_id(app: &Router) -> i64 {
	let (_, sources) = request(app, "GET", "/api/v1/sources", None).await;
	source_by_key(&sources, SOURCE_KEY)["id"]
		.as_i64()
		.expect("source id")
}

async fn detail(app: &Router, manga_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

fn lock_provider<'a>(manga: &'a Value, field: &str) -> Option<&'a Value> {
	manga["metadataLocks"].get(field).map(|lock| &lock["provider"])
}

#[tokio::test]
async fn patch_records_manual_locks_and_unlocks_them() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;

	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Hand Written", "authors": ["Someone"], "status": "Hiatus"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["title"], "Hand Written");
	assert_eq!(body["authors"], json!(["Someone"]));
	assert_eq!(body["status"], "Hiatus");
	assert_eq!(lock_provider(&body, "title"), Some(&json!("manual")));
	assert_eq!(lock_provider(&body, "authors"), Some(&json!("manual")));
	assert_eq!(lock_provider(&body, "status"), Some(&json!("manual")));
	assert!(body["metadataLocks"]["title"]["appliedAt"].as_i64().unwrap_or(0) > 0);

	// Invalid values and unknown unlock field names answer 400
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"status": "Ongoing-ish"})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"metadataUnlock": ["nope"]})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	// Unlocking one field hands only that field back to the source
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"metadataUnlock": ["title"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert!(body["metadataLocks"].get("title").is_none());
	assert_eq!(lock_provider(&body, "authors"), Some(&json!("manual")));
}

#[tokio::test]
async fn source_refresh_keeps_locked_metadata() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let source_title = detail(&app, manga_id).await["title"].as_str().expect("title").to_string();

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Locked Title"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// A refresh re-reads the source, which would win the title back if it were not locked
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["title"], "Locked Title");
	assert_eq!(lock_provider(&body, "title"), Some(&json!("manual")));

	// Unlocking with refresh takes the source value immediately
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/manga/{manga_id}/metadata/restore"),
		Some(json!({"fields": ["title"], "refresh": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["manga"]["title"], source_title);
	assert!(body["manga"]["metadataLocks"].get("title").is_none());
	assert_eq!(body["unlockedFields"], json!(["title"]));
}

#[tokio::test]
async fn preview_and_apply_from_another_source() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let source_id = example_source_id(&app).await;
	let key = detail(&app, manga_id).await["key"].as_str().expect("key").to_string();

	let (status, preview) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/metadata/preview?provider=source:{source_id}&remoteId={key}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{preview}");
	assert_eq!(preview["candidate"]["provider"], format!("source:{source_id}"));
	assert_eq!(preview["candidate"]["remoteId"], key);
	assert_eq!(preview["linked"], false);
	let fields = preview["fields"].as_array().expect("fields");
	assert_eq!(fields.len(), 8);
	let title = fields.iter().find(|field| field["field"] == "title").expect("title field");
	assert_eq!(title["selectable"], true);
	assert_eq!(title["incoming"], detail(&app, manga_id).await["title"]);
	assert_eq!(title["lock"], Value::Null);

	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/manga/{manga_id}/metadata/apply"),
		Some(json!({"provider": format!("source:{source_id}"), "remoteId": key, "fields": ["title"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["appliedFields"], json!(["title"]));
	assert_eq!(lock_provider(&body["manga"], "title"), Some(&json!(format!("source:{source_id}"))));

	// The match survives a later source refresh
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		lock_provider(&body, "title"),
		Some(&json!(format!("source:{source_id}")))
	);
}

#[tokio::test]
async fn metadata_endpoints_validate_citations_and_selections() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let source_id = example_source_id(&app).await;
	let key = detail(&app, manga_id).await["key"].as_str().expect("key").to_string();

	// Citation forms
	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/metadata/preview?provider=foo&remoteId=1"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	let (status, _) = request(
		&app,
		"GET",
		&format!("/api/v1/manga/{manga_id}/metadata/preview?provider=source:99999&remoteId=x"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(
		&app,
		"GET",
		"/api/v1/manga/999999/metadata/preview?provider=tracker:anilist&remoteId=1",
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// bind is tracker-only, an empty selection is rejected, and unknown fields answer 400
	for body in [
		json!({"provider": format!("source:{source_id}"), "remoteId": key, "fields": ["title"], "bind": true}),
		json!({"provider": format!("source:{source_id}"), "remoteId": key, "fields": []}),
		json!({"provider": format!("source:{source_id}"), "remoteId": key, "fields": ["nope"]}),
	] {
		let (status, _) = request(
			&app,
			"POST",
			&format!("/api/v1/manga/{manga_id}/metadata/apply"),
			Some(body),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST);
	}

	// Only a title variant the preview offered may be applied
	let (status, _) = request(
		&app,
		"POST",
		&format!("/api/v1/manga/{manga_id}/metadata/apply"),
		Some(json!({
			"provider": format!("source:{source_id}"),
			"remoteId": key,
			"fields": ["title"],
			"values": {"title": "Not Offered"}
		})),
	)
	.await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn restore_without_fields_unlocks_everything() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let source_title = detail(&app, manga_id).await["title"].as_str().expect("title").to_string();

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Temporary", "tags": ["one"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// No body unlocks every locked field
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/manga/{manga_id}/metadata/restore"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["manga"]["metadataLocks"], json!({}));
	let mut unlocked = body["unlockedFields"]
		.as_array()
		.expect("unlocked fields")
		.iter()
		.map(|field| field.as_str().expect("field name"))
		.collect::<Vec<_>>();
	unlocked.sort_unstable();
	assert_eq!(unlocked, ["tags", "title"]);

	// A refresh now takes the source title back
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], source_title);
}

#[tokio::test]
async fn sync_snapshot_carries_metadata_locks() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	let pool = connect_db(&t.db_path).await;

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Synced Title"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let changes = changes_since(&pool, 1, 0, 10_000).await.expect("changes").changes;
	let manga = changes
		.iter()
		.find(|change| {
			change.entity_kind == "manga" && change.entity_key == manga_id.to_string() && !change.deleted
		})
		.unwrap_or_else(|| panic!("missing manga change: {changes:?}"));
	assert_eq!(
		manga.snapshot["metadataLocks"]["title"]["provider"],
		json!("manual"),
		"snapshot: {}",
		manga.snapshot
	);
}

#[tokio::test]
async fn backup_document_round_trips_metadata_locks() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Backup Title"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// The document only exports entries that belong to the library
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	let key = detail(&app, manga_id).await["key"].as_str().expect("key").to_string();
	let (status, doc) = request(&app, "GET", "/api/v1/backups/document", None).await;
	assert_eq!(status, StatusCode::OK);
	let entry = doc["manga"]
		.as_array()
		.expect("manga entries")
		.iter()
		.find(|entry| entry["key"] == key)
		.expect("manga entry");
	assert_eq!(entry["metadataLocks"]["title"]["provider"], "manual");

	// A legacy document without metadataLocks leaves the target's locks alone
	let mut legacy = doc.clone();
	let entry = legacy["manga"]
		.as_array_mut()
		.expect("manga entries")
		.iter_mut()
		.find(|entry| entry["key"] == key)
		.expect("manga entry");
	entry.as_object_mut().expect("manga entry").remove("metadataLocks");
	let (status, _) = request(&app, "POST", "/api/v1/backups/document", Some(legacy)).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(lock_provider(&detail(&app, manga_id).await, "title"), Some(&json!("manual")));

	// A document that carries an empty lock map clears them
	let mut cleared = doc.clone();
	let entry = cleared["manga"]
		.as_array_mut()
		.expect("manga entries")
		.iter_mut()
		.find(|entry| entry["key"] == key)
		.expect("manga entry");
	entry["metadataLocks"] = json!({});
	let (status, _) = request(&app, "POST", "/api/v1/backups/document", Some(cleared)).await;
	assert_eq!(status, StatusCode::OK);
	assert!(detail(&app, manga_id).await["metadataLocks"].get("title").is_none());
}
