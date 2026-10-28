//! Aidoku-style filter groups: CRUD/reorder, rule validation, groupId evaluation in the
//! library query, the default-unread unreadCount alignment, and the library display settings.

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, connect_db, request, source_by_key, test_app};
use serde_json::{Value, json};

/// Registers the first three search results, shelves them, and returns their ids
async fn register_three_shelved(app: &Router) -> Vec<i64> {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let (_, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	let entries = body["entries"].as_array().expect("entries");
	let mut ids = Vec::new();
	for entry in entries.iter().take(3) {
		let (status, body) = request(
			app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "manga": entry})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
		let id = body["id"].as_i64().expect("manga id");
		let (status, _) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
		assert_eq!(status, StatusCode::OK);
		ids.push(id);
	}
	ids
}

async fn chapter_ids(app: &Router, manga_id: i64) -> Vec<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters")
		.iter()
		.map(|c| c["id"].as_i64().expect("chapter id"))
		.collect()
}

async fn put_chapter_user(
	pool: &sqlx::SqlitePool,
	chapter_id: i64,
	is_read: bool,
	is_downloaded: bool,
	last_read_at: Option<i64>,
) {
	sqlx::query(
		"INSERT INTO chapter_user (chapter_id, user_id, is_read, is_downloaded, last_read_at)
		 VALUES (?, 1, ?, ?, ?)",
	)
	.bind(chapter_id)
	.bind(is_read)
	.bind(is_downloaded)
	.bind(last_read_at)
	.execute(pool)
	.await
	.expect("insert chapter_user");
}

async fn create_category(app: &Router, name: &str) -> i64 {
	let (status, body) = request(app, "POST", "/api/v1/categories", Some(json!({"name": name}))).await;
	assert_eq!(status, StatusCode::OK);
	body["id"].as_i64().expect("category id")
}

async fn create_group(app: &Router, name: &str, rules: Value) -> (StatusCode, Value) {
	request(
		app,
		"POST",
		"/api/v1/filter-groups",
		Some(json!({"name": name, "rules": rules})),
	)
	.await
}

/// The entry id set a library response carries, order-insensitive
fn entry_set(body: &Value) -> std::collections::HashSet<i64> {
	body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["id"].as_i64().expect("id"))
		.collect()
}

async fn group_library_set(app: &Router, group_id: i64) -> std::collections::HashSet<i64> {
	let (status, body) = request(app, "GET", &format!("/api/v1/library?groupId={group_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	entry_set(&body)
}

#[tokio::test]
async fn filter_group_crud_reorder() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;

	let (status, body) = request(&app, "GET", "/api/v1/filter-groups", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().expect("groups").len(), 0);

	// create with a mixed rule set; rules echo back with valued rules as numbers
	let (status, body) = create_group(
		&app,
		"Unread action",
		json!([
			{"type": "has_unread"},
			{"type": "content_rating", "value": 2, "exclude": true},
			{"type": "source", "value": "1"}
		]),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let group_a = body["id"].as_i64().expect("group id");
	assert_eq!(body["sortOrder"], 0);
	let rules = body["rules"].as_array().expect("rules");
	assert_eq!(rules.len(), 3);
	assert_eq!(rules[0], json!({"type": "has_unread", "value": null, "exclude": false}));
	assert_eq!(rules[1], json!({"type": "content_rating", "value": 2, "exclude": true}));
	assert_eq!(rules[2], json!({"type": "source", "value": 1, "exclude": false}));

	// duplicate name is a 409, same as categories
	let (status, body) = create_group(&app, "Unread action", json!([])).await;
	assert_eq!(status, StatusCode::CONFLICT);
	assert_eq!(body["error"]["code"], "duplicate");
	let (status, _) = create_group(&app, "  ", json!([])).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);

	let (status, body) = create_group(&app, "Finished", json!([{"type": "completed"}])).await;
	assert_eq!(status, StatusCode::OK);
	let group_b = body["id"].as_i64().expect("group id");
	assert_eq!(body["sortOrder"], 1);

	// rule validation: unknown type, a flag carrying a value, a valued rule without one,
	// an out-of-range rating, a non-positive id, and a duplicated (type, value) pair all 400
	for rules in [
		json!([{"type": "bogus"}]),
		json!([{"type": "downloaded", "value": 1}]),
		json!([{"type": "source"}]),
		json!([{"type": "content_rating", "value": 4}]),
		json!([{"type": "category", "value": 0}]),
		json!([{"type": "source", "value": "abc"}]),
		json!([{"type": "started"}, {"type": "started", "exclude": true}]),
		json!([{"type": "source", "value": 1}, {"type": "source", "value": 1, "exclude": true}]),
	] {
		let (status, body) = create_group(&app, "bad", rules).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	}

	// rename + replace rules through PATCH
	let (status, body) = request(
		&app,
		"PATCH",
		&format!("/api/v1/filter-groups/{group_a}"),
		Some(json!({"name": "Unread", "rules": [{"type": "has_unread"}]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["name"], "Unread");
	assert_eq!(body["rules"].as_array().expect("rules").len(), 1);
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/filter-groups/{group_a}"),
		Some(json!({"name": "Finished"})),
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT);
	// an empty patch is a no-op; patch/delete of an unknown id is a 404
	let (status, _) = request(&app, "PATCH", &format!("/api/v1/filter-groups/{group_a}"), None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(&app, "PATCH", "/api/v1/filter-groups/999", Some(json!({"name": "x"}))).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(&app, "DELETE", "/api/v1/filter-groups/999", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// reorder swaps the strip order
	let (status, body) = request(
		&app,
		"PATCH",
		"/api/v1/filter-groups/reorder",
		Some(json!({"ids": [group_b, group_a]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let groups = body.as_array().expect("groups");
	assert_eq!(groups[0]["id"], group_b);
	assert_eq!(groups[0]["sortOrder"], 0);
	assert_eq!(groups[1]["id"], group_a);
	assert_eq!(groups[1]["sortOrder"], 1);

	// delete cascades the rule rows; the deleted id 404s on the library query
	let (status, _) = request(&app, "DELETE", &format!("/api/v1/filter-groups/{group_b}"), None).await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	let pool = connect_db(&db_path).await;
	let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM filter_group_rule WHERE group_id = ?")
		.bind(group_b)
		.fetch_one(&pool)
		.await
		.expect("count rules");
	assert_eq!(count, 0);
	let (status, _) = request(&app, "GET", &format!("/api/v1/library?groupId={group_b}"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _) = request(&app, "GET", "/api/v1/library?groupId=999", None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A: never opened (no chapter_user rows), completed, rating 1, category X.
/// B: one chapter read + one downloaded, ongoing, rating 2, category Y.
/// C: all chapters read, ongoing, rating 3, uncategorized.
struct Fixture {
	a: i64,
	b: i64,
	c: i64,
	cat_x: i64,
	cat_y: i64,
	source_id: i64,
}

async fn library_fixture(app: &Router, pool: &sqlx::SqlitePool) -> Fixture {
	let ids = register_three_shelved(app).await;
	let (a, b, c) = (ids[0], ids[1], ids[2]);
	let (_, body) = request(app, "GET", "/api/v1/sources", None).await;
	let source_id = source_by_key(&body, "en.example-source")["id"]
		.as_i64()
		.expect("source id");
	let cat_x = create_category(app, "X").await;
	let cat_y = create_category(app, "Y").await;
	for (id, cat) in [(a, cat_x), (b, cat_y)] {
		let (status, _) = request(
			app,
			"PUT",
			&format!("/api/v1/manga/{id}/categories"),
			Some(json!({"categoryIds": [cat]})),
		)
		.await;
		assert_eq!(status, StatusCode::OK);
	}
	let b_ch = chapter_ids(app, b).await;
	put_chapter_user(pool, b_ch[0], true, false, Some(100)).await;
	put_chapter_user(pool, b_ch[1], false, true, None).await;
	for ch in chapter_ids(app, c).await {
		put_chapter_user(pool, ch, true, false, Some(100)).await;
	}
	for (id, status, rating) in [(a, 2, 1), (b, 1, 2), (c, 1, 3)] {
		sqlx::query("UPDATE manga SET status = ?, content_rating = ? WHERE id = ?")
			.bind(status)
			.bind(rating)
			.bind(id)
			.execute(pool)
			.await
			.expect("set manga columns");
	}
	Fixture {
		a,
		b,
		c,
		cat_x,
		cat_y,
		source_id,
	}
}

async fn must_group(app: &Router, name: &str, rules: Value) -> i64 {
	let (status, body) = create_group(app, name, rules).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["id"].as_i64().expect("group id")
}

#[tokio::test]
async fn library_group_flag_rules() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let f = library_fixture(&app, &pool).await;

	// has_unread, Aidoku's default-unread semantics: a chapter with no chapter_user row counts as
	// unread, so the never-opened A matches include and the partially-read B too; excluding means
	// "every chapter is read", which only the fully-read C matches — never A
	let group = must_group(&app, "unread", json!([{"type": "has_unread"}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.b].into());
	let (status, _) = request(&app, "PATCH", &format!("/api/v1/filter-groups/{group}"), Some(json!({"rules": [{"type": "has_unread", "exclude": true}]}))).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(group_library_set(&app, group).await, [f.c].into());

	// downloaded: B has a downloaded chapter; A and C have none
	let group = must_group(&app, "dl", json!([{"type": "downloaded"}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.b].into());
	let group = must_group(&app, "dlx", json!([{"type": "downloaded", "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.c].into());

	// started: a history stamp on any chapter; A was never opened
	let group = must_group(&app, "started", json!([{"type": "started"}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.b, f.c].into());
	let group = must_group(&app, "startedx", json!([{"type": "started", "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a].into());

	// completed: manga status; only A
	let group = must_group(&app, "done", json!([{"type": "completed"}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a].into());
	let group = must_group(&app, "donex", json!([{"type": "completed", "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.b, f.c].into());

	// a group with no rules matches the whole library
	let group = must_group(&app, "empty", json!([])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.b, f.c].into());
}

#[tokio::test]
async fn library_group_dimensions_or_and() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let f = library_fixture(&app, &pool).await;

	// same-dimension includes are OR'd: ratings 1 and 3 together match A and C, not B
	let group = must_group(
		&app,
		"ratings",
		json!([{"type": "content_rating", "value": 1}, {"type": "content_rating", "value": 3}]),
	)
	.await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.c].into());
	// excludes apply per value: excluding rating 2 drops B
	let group = must_group(&app, "ratingx", json!([{"type": "content_rating", "value": 2, "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.c].into());

	// category includes OR, excludes per value
	let group = must_group(
		&app,
		"cats",
		json!([{"type": "category", "value": f.cat_x}, {"type": "category", "value": f.cat_y}]),
	)
	.await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.b].into());
	let group = must_group(&app, "catx", json!([{"type": "category", "value": f.cat_y, "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.c].into());

	// source: every fixture manga shares one source, so include matches all and exclude none
	let group = must_group(&app, "src", json!([{"type": "source", "value": f.source_id}])).await;
	assert_eq!(group_library_set(&app, group).await, [f.a, f.b, f.c].into());
	let group = must_group(&app, "srcx", json!([{"type": "source", "value": f.source_id, "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await.len(), 0);

	// different dimensions AND: has_unread ORs nothing with the rating set — A and B have unread,
	// ratings 1|2 match A and B, but the category X rule leaves only A
	let group = must_group(
		&app,
		"cross",
		json!([
			{"type": "has_unread"},
			{"type": "content_rating", "value": 1},
			{"type": "content_rating", "value": 2},
			{"type": "category", "value": f.cat_x}
		]),
	)
	.await;
	assert_eq!(group_library_set(&app, group).await, [f.a].into());
}

#[tokio::test]
async fn library_group_composes_with_adhoc_filters() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let f = library_fixture(&app, &pool).await;
	let group = must_group(&app, "unread", json!([{"type": "has_unread"}])).await;

	// group AND categoryId: A and B have unread, only B is under category Y
	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?groupId={group}&categoryId={}", f.cat_y),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_set(&body), [f.b].into());

	// group AND the ad-hoc boolean filter: unread ∩ downloaded = B
	let (status, body) = request(&app, "GET", &format!("/api/v1/library?groupId={group}&filter=downloaded"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_set(&body), [f.b].into());

	// group AND search: the search term narrows the group's own result further
	let (_, body) = request(&app, "GET", &format!("/api/v1/manga/{}", f.a), None).await;
	let title = body["title"].as_str().expect("title");
	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/library?groupId={group}&search={}", urlencoding(title)),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(entry_set(&body), [f.a].into());
}

/// Text-valued rules (author/tag): a string value matched exactly, case-insensitively, against
/// the manga's JSON array column; validated on write, echoing back as a JSON string
#[tokio::test]
async fn library_group_text_rules() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let ids = register_three_shelved(&app).await;
	let (a, b, c) = (ids[0], ids[1], ids[2]);
	let pool = connect_db(&db_path).await;
	for (id, authors, tags) in [
		(a, r#"["Asanagi"]"#, r#"["action","romance"]"#),
		(b, r#"["Asanagi","Murasaki"]"#, r#"["Action"]"#),
		(c, r#"["Murasaki"]"#, r#"[]"#),
	] {
		sqlx::query("UPDATE manga SET authors = ?, tags = ? WHERE id = ?")
			.bind(authors)
			.bind(tags)
			.bind(id)
			.execute(&pool)
			.await
			.expect("set authors/tags");
	}

	// rule validation: a text rule without a value, with a blank one, with a non-string one,
	// and a duplicated (type, value) pair all 400
	for rules in [
		json!([{"type": "author"}]),
		json!([{"type": "author", "value": ""}]),
		json!([{"type": "tag", "value": "  "}]),
		json!([{"type": "author", "value": 1}]),
		json!([{"type": "tag", "value": "action"}, {"type": "tag", "value": "action", "exclude": true}]),
	] {
		let (status, body) = create_group(&app, "bad-text", rules).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
	}

	// the value echoes back as a JSON string, unlike the numeric valued rules
	let (status, body) = create_group(
		&app,
		"authors",
		json!([{"type": "author", "value": "asanagi"}]),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["rules"][0], json!({"type": "author", "value": "asanagi", "exclude": false}));

	// include matches exactly but case-insensitively; OR within the type
	let group = body["id"].as_i64().expect("group id");
	assert_eq!(group_library_set(&app, group).await, [a, b].into());
	let group = must_group(
		&app,
		"authors-or",
		json!([{"type": "author", "value": "Team X"}, {"type": "author", "value": "Murasaki"}]),
	)
	.await;
	assert_eq!(group_library_set(&app, group).await, [b, c].into());
	// exclude per value; tag include; the two dimensions AND
	let group = must_group(&app, "tagx", json!([{"type": "tag", "value": "Action"}])).await;
	assert_eq!(group_library_set(&app, group).await, [a, b].into());
	let group = must_group(
		&app,
		"authorx",
		json!([{"type": "author", "value": "Asanagi"}, {"type": "tag", "value": "romance"}]),
	)
	.await;
	assert_eq!(group_library_set(&app, group).await, [a].into());
	let group = must_group(&app, "murasakix", json!([{"type": "author", "value": "Murasaki", "exclude": true}])).await;
	assert_eq!(group_library_set(&app, group).await, [a].into());
}

fn urlencoding(raw: &str) -> String {
	let mut out = String::new();
	for byte in raw.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

#[tokio::test]
async fn library_card_unread_count_is_default_unread() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;
	let pool = connect_db(&db_path).await;
	let ids = register_three_shelved(&app).await;
	let (a, b, c) = (ids[0], ids[1], ids[2]);
	let chapter_total = chapter_ids(&app, a).await.len() as i64;
	assert!(chapter_total > 0);

	// A is never opened: the card badge counts every chapter, same as the detail page
	let b_ch = chapter_ids(&app, b).await;
	put_chapter_user(&pool, b_ch[0], true, false, Some(100)).await;
	for ch in chapter_ids(&app, c).await {
		put_chapter_user(&pool, ch, true, false, Some(100)).await;
	}
	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let counts: std::collections::HashMap<i64, i64> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| {
			(
				e["id"].as_i64().expect("id"),
				e["unreadCount"].as_i64().expect("unreadCount"),
			)
		})
		.collect();
	assert_eq!(counts[&a], chapter_total);
	assert_eq!(counts[&b], chapter_total - 1);
	assert_eq!(counts[&c], 0);

	// sort=unread orders by the same count: A ahead of B ahead of the fully-read C
	let (_, body) = request(&app, "GET", "/api/v1/library?sort=unread", None).await;
	let order: Vec<i64> = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.map(|e| e["id"].as_i64().expect("id"))
		.collect();
	assert_eq!(order, vec![a, b, c]);
}

#[tokio::test]
async fn library_display_settings() {
	let TestApp {
		app,
		tmp: _tmp,
		db_path,
	} = test_app().await;

	// round-trip through the settings endpoint
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.default_category": "uncategorized", "library.show_uncategorized": false})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["library"]["defaultCategory"], "uncategorized");
	assert_eq!(body["library"]["showUncategorized"], false);
	let (_, body) = request(&app, "GET", "/api/v1/settings", None).await;
	assert_eq!(body["library"]["defaultCategory"], "uncategorized");
	assert_eq!(body["library"]["showUncategorized"], false);

	// a category id in string form stores; "all" is the neutral default
	let cat = create_category(&app, "X").await;
	let (status, body) = request(
		&app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"library.default_category": cat.to_string()})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["library"]["defaultCategory"], cat.to_string());

	// shape validation: anything but "all"/"uncategorized"/a positive id 400s, as do non-booleans
	for value in [json!("bogus"), json!("0"), json!("-1"), json!(1), json!(null)] {
		let (status, body) = request(&app, "PUT", "/api/v1/settings", Some(json!({"library.default_category": value}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}
	for value in [json!("yes"), json!(1)] {
		let (status, body) = request(&app, "PUT", "/api/v1/settings", Some(json!({"library.show_uncategorized": value}))).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{value}: {body}");
	}

	// user display preferences live in the caller's namespace, never as bare global keys
	let pool = connect_db(&db_path).await;
	let (namespaced,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM setting WHERE key = 'user.1.library.default_category'")
			.fetch_one(&pool)
			.await
			.expect("count namespaced");
	assert_eq!(namespaced, 1);
	let (bare,): (i64,) =
		sqlx::query_as("SELECT COUNT(*) FROM setting WHERE key = 'library.default_category'")
			.fetch_one(&pool)
			.await
			.expect("count bare");
	assert_eq!(bare, 0);
}
