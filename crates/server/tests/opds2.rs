mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use common::{TestApp, base_config, connect_db, fixture_dir, register_example_manga, request};
use serde_json::{Value, json};
use sumidoku_server::build_app;
use tower::ServiceExt;

const ACQUISITION: &str = "http://opds-spec.org/acquisition";

/// OPDS assertions inspect absolute URLs, so requests always carry Host + X-Forwarded-Proto
async fn opds_get(app: &Router, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
	opds_get_auth(app, path, None).await
}

async fn opds_get_auth(app: &Router, path: &str, auth: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
	let mut builder = Request::builder()
		.method("GET")
		.uri(path)
		.header(header::HOST, "opds.test")
		.header("x-forwarded-proto", "https");
	if let Some(auth) = auth {
		builder = builder.header(header::AUTHORIZATION, auth);
	}
	let request = builder.body(Body::empty()).expect("build request");
	let response = app.clone().oneshot(request).await.expect("response");
	let status = response.status();
	let headers = response.headers().clone();
	let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.expect("read body");
	(status, headers, bytes.to_vec())
}

/// Boots the app and registers + shelves one manga; downloads_dir points into tmp, so build_app's create_dir_all doesn't touch the repo directory
async fn opds2_app() -> (TestApp, i64) {
	let tmp = tempfile::tempdir().expect("tempdir");
	let source_dir = tmp.path().join("sources").join("en.example-source");
	std::fs::create_dir_all(&source_dir).expect("mkdir source dir");
	for file in ["source.json", "main.wasm"] {
		std::fs::copy(fixture_dir().join(file), source_dir.join(file)).expect("copy fixture");
	}
	let mut config = base_config(&tmp);
	config.downloads_dir = tmp.path().join("downloads");
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let manga_id = register_example_manga(&app).await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	(TestApp { app, tmp, db_path }, manga_id)
}

async fn first_chapters(app: &Router, manga_id: i64) -> Vec<Value> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array().expect("chapters").clone()
}

async fn feed(app: &Router, path: &str) -> Value {
	let (status, headers, body) = opds_get(app, path).await;
	assert_eq!(status, StatusCode::OK, "GET {path}");
	assert_eq!(headers[header::CONTENT_TYPE], "application/opds+json", "GET {path}");
	serde_json::from_slice(&body).expect("feed json")
}

fn link<'a>(links: &'a [Value], rel: &str) -> Option<&'a Value> {
	links.iter().find(|l| l["rel"].as_str() == Some(rel))
}

#[tokio::test]
async fn catalog_feeds() {
	let (test_app, manga_id) = opds2_app().await;
	let TestApp { app, tmp: _tmp, db_path } = &test_app;
	let pool = connect_db(db_path).await;
	let (_, manga) = request(app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	let title = manga["title"].as_str().expect("title").to_string();

	// root: metadata + self/start/search links + five navigation entries
	let root = feed(app, "/opds/v2/root").await;
	assert_eq!(root["metadata"]["title"], "Sumidoku");
	let links = root["links"].as_array().expect("links");
	let self_link = link(links, "self").expect("self link");
	assert_eq!(self_link["href"], "https://opds.test/opds/v2/root");
	assert_eq!(self_link["type"], "application/opds+json");
	assert_eq!(link(links, "start").expect("start link")["href"], "https://opds.test/opds/v2/root");
	let search_link = link(links, "search").expect("search link");
	assert_eq!(search_link["href"], "https://opds.test/opds/v2/search{?query}");
	assert_eq!(search_link["type"], "application/opds+json");
	assert_eq!(search_link["templated"], true);
	let nav = root["navigation"].as_array().expect("navigation");
	let titles: Vec<&str> = nav.iter().map(|l| l["title"].as_str().expect("nav title")).collect();
	assert_eq!(titles, ["Library", "Categories", "Sources", "History", "Updates"]);
	for (entry, path) in nav.iter().zip(["library", "categories", "sources", "history", "updates"]) {
		assert_eq!(entry["href"], format!("https://opds.test/opds/v2/{path}"));
		assert_eq!(entry["type"], "application/opds+json");
	}

	// library: publication shape for a single manga + absolute URLs
	let library = feed(app, "/opds/v2/library").await;
	assert_eq!(library["metadata"]["title"], "Library");
	let pubs = library["publications"].as_array().expect("publications");
	assert_eq!(pubs.len(), 1, "library: {library}");
	let pub0 = &pubs[0];
	assert_eq!(pub0["metadata"]["identifier"], format!("urn:sumidoku:manga:{manga_id}"));
	assert_eq!(pub0["metadata"]["title"], title);
	assert!(pub0["metadata"]["modified"].as_str().expect("modified").ends_with('Z'));
	let images = pub0["images"].as_array().expect("images");
	assert_eq!(images[0]["href"], format!("https://opds.test/opds/v1.2/cover/{manga_id}"));
	assert_eq!(images[0]["type"], "image/jpeg");
	let pub_links = pub0["links"].as_array().expect("publication links");
	let pub_self = link(pub_links, "self").expect("publication self link");
	assert_eq!(pub_self["href"], format!("https://opds.test/opds/v2/series/{manga_id}"));
	assert_eq!(pub_self["type"], "application/opds+json");

	// author/artist/description are only present when set
	sqlx::query("UPDATE manga SET authors = '[\"Alice\", \"Alicia\"]', artists = '[\"Bob\"]', description = 'Desc & <b>' WHERE id = ?")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update credits");
	let library = feed(app, "/opds/v2/library").await;
	let meta = &library["publications"][0]["metadata"];
	assert_eq!(meta["author"], json!(["Alice", "Alicia"]));
	assert_eq!(meta["artist"], json!(["Bob"]));
	assert_eq!(meta["description"], "Desc & <b>");

	// escaping: special characters follow JSON string semantics and must come back verbatim after parsing
	sqlx::query("UPDATE manga SET title = ? WHERE id = ?")
		.bind("A & B <C> \"D\"")
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("update title");
	let library = feed(app, "/opds/v2/library").await;
	assert_eq!(library["publications"][0]["metadata"]["title"], "A & B <C> \"D\"");

	// categories: navigation + category feed
	let row = sqlx::query("INSERT INTO category (user_id, name, sort_order) VALUES (1, 'Drama', 0)")
		.execute(&pool)
		.await
		.expect("insert category");
	let category_id = row.last_insert_rowid();
	sqlx::query("INSERT INTO category_manga (category_id, manga_id, user_id) VALUES (?, ?, 1)")
		.bind(category_id)
		.bind(manga_id)
		.execute(&pool)
		.await
		.expect("link category");
	let categories = feed(app, "/opds/v2/categories").await;
	let nav = categories["navigation"].as_array().expect("navigation");
	assert_eq!(nav.len(), 1, "categories: {categories}");
	assert_eq!(nav[0]["title"], "Drama");
	assert_eq!(nav[0]["href"], format!("https://opds.test/opds/v2/categories/{category_id}"));
	let category = feed(app, &format!("/opds/v2/categories/{category_id}")).await;
	assert_eq!(category["metadata"]["title"], "Drama");
	assert_eq!(category["publications"].as_array().expect("publications").len(), 1);

	// sources: navigation + source feed
	let (source_id, source_name): (i64, String) =
		sqlx::query_as("SELECT s.id, s.name FROM source s JOIN manga m ON m.source_id = s.id WHERE m.id = ?")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("source row");
	let sources = feed(app, "/opds/v2/sources").await;
	let nav = sources["navigation"].as_array().expect("navigation");
	// the built-in local source is persisted by the migration and appears in the source catalog even with no manga (same contract as an empty aidoku source)
	assert_eq!(nav.len(), 2, "sources: {sources}");
	assert_eq!(nav[0]["title"], "Local");
	let entry = nav
		.iter()
		.find(|n| n["title"] == source_name)
		.expect("source nav entry");
	assert_eq!(entry["href"], format!("https://opds.test/opds/v2/sources/{source_id}"));
	let source = feed(app, &format!("/opds/v2/sources/{source_id}")).await;
	assert_eq!(source["publications"].as_array().expect("publications").len(), 1);

	// series: populate the page cache for one chapter first (page_count=2), then insert a finished download row
	let chapters = first_chapters(app, manga_id).await;
	let chapter_id = chapters[0]["id"].as_i64().expect("chapter id");
	let other_chapter_id = chapters[1]["id"].as_i64().expect("chapter id");
	let (status, _) = request(app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	sqlx::query(
		"INSERT INTO download (chapter_id, user_id, status, format, path, created_at, updated_at)
		 VALUES (?, 1, 'finished', 'cbz', 'ch1.cbz', 0, 0)",
	)
	.bind(chapter_id)
	.execute(&pool)
	.await
	.expect("insert download");

	let series = feed(app, &format!("/opds/v2/series/{manga_id}")).await;
	assert_eq!(series["metadata"]["title"], "A & B <C> \"D\"");
	let pubs = series["publications"].as_array().expect("publications");
	assert_eq!(pubs.len(), chapters.len(), "series: {series}");
	let downloaded = pubs
		.iter()
		.find(|p| p["metadata"]["identifier"] == format!("urn:sumidoku:chapter:{chapter_id}"))
		.expect("downloaded chapter publication");
	assert!(!downloaded["metadata"]["title"].as_str().expect("chapter title").is_empty());
	assert!(downloaded["metadata"]["modified"].as_str().expect("modified").ends_with('Z'));
	let dl_links = downloaded["links"].as_array().expect("chapter links");
	let acq = link(dl_links, ACQUISITION).expect("acquisition link");
	assert_eq!(acq["href"], format!("https://opds.test/opds/v1.2/file/{chapter_id}"));
	assert_eq!(acq["type"], "application/zip");
	assert_eq!(
		downloaded["images"][0]["href"],
		format!("https://opds.test/opds/v1.2/stream/{chapter_id}/0")
	);
	assert_eq!(downloaded["images"][0]["type"], "image/jpeg");
	// a chapter neither downloaded nor with a loaded page list: no acquisition link, no images, but still listed as usual
	let other = pubs
		.iter()
		.find(|p| p["metadata"]["identifier"] == format!("urn:sumidoku:chapter:{other_chapter_id}"))
		.expect("other chapter publication");
	assert!(other["links"].is_null(), "other chapter: {other}");
	assert!(other["images"].is_null(), "other chapter: {other}");
	let acq_count = pubs
		.iter()
		.filter(|p| {
			p["links"]
				.as_array()
				.is_some_and(|links| link(links, ACQUISITION).is_some())
		})
		.count();
	assert_eq!(acq_count, 1, "only the downloaded chapter gets a zip link: {series}");

	// updates: chapter publications, count matches the total chapter count
	let updates = feed(app, "/opds/v2/updates").await;
	let pubs = updates["publications"].as_array().expect("publications");
	assert_eq!(pubs.len(), chapters.len(), "updates: {updates}");
	for p in pubs {
		assert!(
			p["metadata"]["identifier"]
				.as_str()
				.expect("identifier")
				.starts_with("urn:sumidoku:chapter:"),
			"updates publication: {p}"
		);
	}

	// history: the manga appears after progress is recorded
	let (status, _) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let history = feed(app, "/opds/v2/history").await;
	let pubs = history["publications"].as_array().expect("publications");
	assert_eq!(pubs.len(), 1, "history: {history}");
	assert_eq!(pubs[0]["metadata"]["identifier"], format!("urn:sumidoku:manga:{manga_id}"));

	// search: hit and miss
	let search = feed(app, "/opds/v2/search?q=B").await;
	let pubs = search["publications"].as_array().expect("publications");
	assert_eq!(pubs.len(), 1, "search: {search}");
	assert_eq!(pubs[0]["metadata"]["title"], "A & B <C> \"D\"");
	let search = feed(app, "/opds/v2/search?q=zzz-no-match").await;
	assert_eq!(search["publications"].as_array().expect("publications").len(), 0);

	// 404: nonexistent series/category/source return JSON errors
	let (status, _, _) = opds_get(app, "/opds/v2/series/99999").await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _, _) = opds_get(app, "/opds/v2/categories/99999").await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	let (status, _, _) = opds_get(app, "/opds/v2/sources/99999").await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	// pagination: shelve 50 more, page 1 has next, page 2 is the tail with no next
	let (source_id,): (i64,) = sqlx::query_as("SELECT source_id FROM manga WHERE id = ?")
		.bind(manga_id)
		.fetch_one(&pool)
		.await
		.expect("source id");
	for i in 0..50 {
		let row = sqlx::query("INSERT INTO manga (source_id, key, title) VALUES (?, ?, ?)")
			.bind(source_id)
			.bind(format!("bulk-{i}"))
			.bind(format!("Bulk {i}"))
			.execute(&pool)
			.await
			.expect("insert manga");
		sqlx::query("INSERT INTO manga_user (manga_id, user_id, in_library, in_library_at) VALUES (?, 1, 1, ?)")
			.bind(row.last_insert_rowid())
			.bind(1000 + i)
			.execute(&pool)
			.await
			.expect("insert manga_user");
	}
	let page1 = feed(app, "/opds/v2/library").await;
	assert_eq!(page1["metadata"]["currentPage"], 1);
	assert_eq!(page1["metadata"]["itemsPerPage"], 50);
	assert_eq!(page1["publications"].as_array().expect("publications").len(), 50);
	let links = page1["links"].as_array().expect("links");
	assert_eq!(link(links, "self").expect("self")["href"], "https://opds.test/opds/v2/library?page=1");
	assert_eq!(
		link(links, "next").expect("next link")["href"],
		"https://opds.test/opds/v2/library?page=2"
	);
	let page2 = feed(app, "/opds/v2/library?page=2").await;
	assert_eq!(page2["metadata"]["currentPage"], 2);
	assert_eq!(page2["publications"].as_array().expect("publications").len(), 1);
	assert!(link(page2["links"].as_array().expect("links"), "next").is_none());
	let (status, _, _) = opds_get(app, "/opds/v2/library?page=0").await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn basic_auth_guard() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.basic_password = Some("secret".into());
	let app = build_app(config).await.expect("build app");

	// no credentials: 401 + WWW-Authenticate
	let (status, headers, _) = opds_get(&app, "/opds/v2/root").await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);
	assert_eq!(
		headers[header::WWW_AUTHENTICATE],
		"Basic realm=\"Sumidoku OPDS\""
	);

	// wrong password: 401 (admin:wrong)
	let (status, _, _) = opds_get_auth(&app, "/opds/v2/root", Some("Basic YWRtaW46d3Jvbmc=")).await;
	assert_eq!(status, StatusCode::UNAUTHORIZED);

	// real username+password: 200 (admin:secret)
	let (status, _, _) = opds_get_auth(&app, "/opds/v2/root", Some("Basic YWRtaW46c2VjcmV0")).await;
	assert_eq!(status, StatusCode::OK);

	// API routes are unaffected by OPDS Basic
	let (status, body) = request(&app, "GET", "/health", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["status"], "ok");
}
