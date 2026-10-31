//! Metadata locks: editing metadata records a manual lock, and the owning source
//! refresh must leave locked fields alone.

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, register_example_manga, request, test_app};
use serde_json::{Value, json};
use sumidoku_server::sync::changes_since;

async fn detail(app: &Router, manga_id: i64) -> Value {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body
}

fn lock_provider<'a>(manga: &'a Value, field: &str) -> Option<&'a Value> {
	manga["metadataLocks"].get(field).map(|lock| &lock["provider"])
}

/// Editing metadata is a library feature, so every test shelves its
/// entry before touching metadata.
async fn shelve(app: &Router, manga_id: i64) {
	let (status, body) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn patch_records_manual_locks_and_unlocks_them() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	shelve(&app, manga_id).await;

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
	shelve(&app, manga_id).await;
	// The example source owns authors, so a refresh really has a value to apply
	let source_authors = detail(&app, manga_id).await["authors"].clone();

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"authors": ["Locked Author"]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// A refresh re-reads the source, which would win the authors back if they were not locked
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["authors"], json!(["Locked Author"]));
	assert_eq!(lock_provider(&body, "authors"), Some(&json!("manual")));

	// Unlocking with refresh takes the source value immediately
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/manga/{manga_id}/metadata/restore"),
		Some(json!({"fields": ["authors"], "refresh": true})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["manga"]["authors"], source_authors);
	assert!(body["manga"]["metadataLocks"].get("authors").is_none());
	assert_eq!(body["unlockedFields"], json!(["authors"]));
}

#[tokio::test]
async fn restore_without_fields_unlocks_everything() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	shelve(&app, manga_id).await;
	// The example source owns authors and tags, so a refresh has values to apply
	let source = detail(&app, manga_id).await;

	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"authors": ["Temporary"], "tags": ["one"]})),
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
	assert_eq!(unlocked, ["authors", "tags"]);

	// A refresh now takes the source values back
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["authors"], source["authors"]);
	assert_eq!(body["tags"], source["tags"]);
}

#[tokio::test]
async fn sync_snapshot_carries_metadata_locks() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;
	shelve(&app, manga_id).await;
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
	// changes_since returns the log in sequence order: the last match is the PATCH
	let manga = changes
		.iter()
		.rev()
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
	shelve(&app, manga_id).await;

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
	let entry = doc["library"]
		.as_array()
		.expect("library entries")
		.iter()
		.find(|entry| entry["key"] == key)
		.expect("library entry");
	assert_eq!(entry["metadataLocks"]["title"]["provider"], "manual");

	// A legacy document without metadataLocks leaves the target's locks alone
	let mut legacy = doc.clone();
	let entry = legacy["library"]
		.as_array_mut()
		.expect("library entries")
		.iter_mut()
		.find(|entry| entry["key"] == key)
		.expect("library entry");
	entry.as_object_mut().expect("library entry").remove("metadataLocks");
	let (status, _) = request(&app, "POST", "/api/v1/backups/document", Some(legacy)).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(lock_provider(&detail(&app, manga_id).await, "title"), Some(&json!("manual")));

	// A document that carries an empty lock map clears them
	let mut cleared = doc.clone();
	let entry = cleared["library"]
		.as_array_mut()
		.expect("library entries")
		.iter_mut()
		.find(|entry| entry["key"] == key)
		.expect("library entry");
	entry["metadataLocks"] = json!({});
	let (status, _) = request(&app, "POST", "/api/v1/backups/document", Some(cleared)).await;
	assert_eq!(status, StatusCode::OK);
	assert!(detail(&app, manga_id).await["metadataLocks"].get("title").is_none());
}

#[tokio::test]
async fn unshelved_entries_reject_metadata_changes() {
	let t = test_app().await;
	let app = t.app.clone();
	let manga_id = register_example_manga(&app).await;

	// A stray search result is not editable or restored
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"title": "Hand Written"})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	let (status, _) = request(&app, "POST", &format!("/api/v1/manga/{manga_id}/metadata/restore"), None).await;
	assert_eq!(status, StatusCode::CONFLICT);

	// Update strategy and incognito still apply to an unshelved entry
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/manga/{manga_id}"),
		Some(json!({"updateStrategy": "Never"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["update_strategy"], "Never");
}
