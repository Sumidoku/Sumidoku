mod common;

use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ---- 本地 mock 源仓库（同 source_repo.rs 模式）：index 可换版，.aix 走 /files/* ----

#[derive(Clone, Default)]
struct MockRepo {
	state: Arc<RwLock<MockRepoState>>,
}

#[derive(Default)]
struct MockRepoState {
	index: Vec<u8>,
	files: HashMap<String, Vec<u8>>,
}

impl MockRepo {
	fn set_index(&self, sources: Value) {
		self.state.write().unwrap().index =
			serde_json::to_vec(&json!({"name": "Mock Repo", "sources": sources})).unwrap();
	}

	fn set_file(&self, path: &str, body: Vec<u8>) {
		self.state.write().unwrap().files.insert(path.to_string(), body);
	}
}

struct RepoServer {
	url: String,
	mock: MockRepo,
	serve: tokio::task::JoinHandle<()>,
}

async fn index_handler(AxumState(mock): AxumState<MockRepo>) -> Vec<u8> {
	mock.state.read().unwrap().index.clone()
}

async fn file_handler(
	AxumState(mock): AxumState<MockRepo>,
	AxumPath(path): AxumPath<String>,
) -> Result<Vec<u8>, StatusCode> {
	mock.state
		.read()
		.unwrap()
		.files
		.get(&path)
		.cloned()
		.ok_or(StatusCode::NOT_FOUND)
}

async fn serve_repo() -> RepoServer {
	let mock = MockRepo::default();
	let app = axum::Router::new()
		.route("/index.json", axum::routing::get(index_handler))
		.route("/{*path}", axum::routing::get(file_handler))
		.with_state(mock.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind mock repo");
	let addr = listener.local_addr().expect("local addr");
	let serve = tokio::spawn(async move {
		axum::serve(listener, app).await.expect("serve mock repo");
	});
	RepoServer {
		url: format!("http://{addr}/index.json"),
		mock,
		serve,
	}
}

/// 指定 wasm 载荷与可选 breakingChangeVersion 的 .aix 包
fn aix_bytes(wasm: &[u8], key: &str, version: i64, breaking: Option<i64>) -> Vec<u8> {
	let config = match breaking {
		Some(v) => format!(r#", "config": {{"breakingChangeVersion": {v}}}"#),
		None => String::new(),
	};
	let manifest =
		format!(r#"{{"info": {{"id": "{key}", "name": "{key}", "version": {version}, "languages": ["en"]}}{config}}}"#);
	let mut cursor = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut cursor);
		let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
		writer.start_file("Payload/source.json", options).unwrap();
		std::io::Write::write_all(&mut writer, manifest.as_bytes()).unwrap();
		writer.start_file("Payload/main.wasm", options).unwrap();
		std::io::Write::write_all(&mut writer, wasm).unwrap();
		writer.finish().unwrap();
	}
	cursor.into_inner()
}

/// 上架 key 的某个版本：index 指向 files/{key}-v{version}.aix
fn stage(repo: &RepoServer, key: &str, version: i64, wasm: &[u8], breaking: Option<i64>) {
	repo.mock.set_index(json!([{
		"id": key,
		"name": key,
		"version": version,
		"downloadURL": format!("files/{key}-v{version}.aix"),
		"languages": ["en"],
	}]));
	repo.mock.set_file(
		&format!("files/{key}-v{version}.aix"),
		aix_bytes(wasm, key, version, breaking),
	);
}

async fn add_repo(app: &axum::Router, url: &str) -> i64 {
	let (status, body) = request(app, "POST", "/api/v1/source-repos", Some(json!({"url": url}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["id"].as_i64().expect("repo id")
}

async fn refresh(app: &axum::Router, repo_id: i64) {
	let (status, body) = request(app, "POST", &format!("/api/v1/source-repos/{repo_id}/refresh"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

async fn install(app: &axum::Router, repo_id: i64, key: &str) -> i64 {
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/source-repos/install",
		Some(json!({"repoId": repo_id, "sourceKey": key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	body["id"].as_i64().expect("source id")
}

/// 搜索并按标题注册一本 manga 且入书库，返回 (manga_id, key)
async fn register_library_manga(app: &axum::Router, source_id: i64, title: &str) -> (i64, String) {
	let (status, body) = request(
		app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entry = body["entries"]
		.as_array()
		.expect("entries")
		.iter()
		.find(|e| e["title"] == title)
		.unwrap_or_else(|| panic!("entry {title}: {body}"))
		.clone();
	let key = entry["key"].as_str().expect("manga key").to_string();
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "manga": entry})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let id = body["id"].as_i64().expect("manga id");
	let (status, body) = request(app, "POST", "/api/v1/library", Some(json!({"mangaId": id}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	(id, key)
}

/// 章节列表里按章节号取 (id, key)
async fn chapter_by_number(app: &axum::Router, manga_id: i64, number: f64) -> (i64, String) {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let chapter = body
		.as_array()
		.expect("chapters array")
		.iter()
		.find(|c| c["chapter_number"].as_f64() == Some(number))
		.unwrap_or_else(|| panic!("chapter {number}: {body}"))
		.clone();
	(
		chapter["id"].as_i64().expect("chapter id"),
		chapter["key"].as_str().expect("chapter key").to_string(),
	)
}

async fn set_progress(app: &axum::Router, chapter_id: i64, last_page: i64, is_read: bool) {
	let (status, body) = request(
		app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": last_page, "isRead": is_read})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
}

const KEY: &str = "en.rekey-source";

/// 跨 breaking 带 MigrationHandler：原地重键，id 不变，进度/分类/下载标记全保留；
/// old/broken 一支 handler 返回空串，整支留旧 key 进 warnings
#[tokio::test]
async fn breaking_update_rekeys_in_place() {
	let app = test_app().await;
	let repo = serve_repo().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	stage(&repo, KEY, 1, &rekey_wasm, None);
	let repo_id = add_repo(&app.app, &repo.url).await;
	let source_id = install(&app.app, repo_id, KEY).await;

	// 两本正常 + 一本映射失败，全部入库并带齐用户数据
	let (manga1, key1) = register_library_manga(&app.app, source_id, "Rekey One").await;
	let (manga2, key2) = register_library_manga(&app.app, source_id, "Rekey Two").await;
	let (broken, broken_key) = register_library_manga(&app.app, source_id, "Broken Three").await;
	assert_eq!((key1.as_str(), key2.as_str(), broken_key.as_str()), ("old/manga-1", "old/manga-2", "old/broken"));

	let (ch1, ch1_key) = chapter_by_number(&app.app, manga1, 1.0).await;
	let (ch2, ch2_key) = chapter_by_number(&app.app, manga1, 2.0).await;
	assert_eq!((ch1_key.as_str(), ch2_key.as_str()), ("old/manga-1/ch-1", "old/manga-1/ch-2"));
	set_progress(&app.app, ch1, 7, true).await;
	set_progress(&app.app, ch2, 3, false).await;

	let (status, body) = request(&app.app, "POST", "/api/v1/categories", Some(json!({"name": "rekey"}))).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let category_id = body["id"].as_i64().expect("category id");
	let (status, body) = request(
		&app.app,
		"PUT",
		&format!("/api/v1/manga/{manga1}/categories"),
		Some(json!({"categoryIds": [category_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");

	// 下载标记直接落 chapter_user.is_downloaded（下载完成后的稳定态）
	let pool = connect_db(&app.db_path).await;
	sqlx::query("UPDATE chapter_user SET is_downloaded = 1 WHERE chapter_id = ?")
		.bind(ch1)
		.execute(&pool)
		.await
		.expect("mark downloaded");

	// repo 发布 v2：同 wasm 带 handler，manifest 跨 breakingChangeVersion=2
	stage(&repo, KEY, 2, &rekey_wasm, Some(2));
	refresh(&app.app, repo_id).await;
	let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{source_id}/update"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");
	let warnings = body["migrationWarnings"].as_array().expect("warnings array").clone();
	assert_eq!(warnings.len(), 3, "broken manga + 2 chapters: {warnings:?}");
	assert!(warnings.iter().all(|w| w.as_str().unwrap_or_default().contains("old/broken")), "{warnings:?}");

	// manga：key 换血、id 不动、入库态与分类保留
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "new/manga-1", "{body}");
	assert_eq!(body["inLibrary"], true, "{body}");
	assert_eq!(body["categoryIds"], json!([category_id]), "{body}");
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga2}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "new/manga-2", "{body}");

	// 章节：key 换血、id 不动、进度与下载标记保留
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let chapters = body.as_array().expect("chapters array");
	let ch1_after = chapters.iter().find(|c| c["id"] == ch1).expect("ch1 present");
	assert_eq!(ch1_after["key"], "new/manga-1/ch-1", "{ch1_after}");
	assert_eq!(ch1_after["lastPageRead"], 7, "{ch1_after}");
	assert_eq!(ch1_after["isRead"], true, "{ch1_after}");
	assert_eq!(ch1_after["isDownloaded"], true, "{ch1_after}");
	let ch2_after = chapters.iter().find(|c| c["id"] == ch2).expect("ch2 present");
	assert_eq!(ch2_after["key"], "new/manga-1/ch-2", "{ch2_after}");
	assert_eq!(ch2_after["lastPageRead"], 3, "{ch2_after}");

	// 映射失败的一支原样保留
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{broken}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "old/broken", "{body}");
	assert_eq!(body["inLibrary"], true, "{body}");
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{broken}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let keys: Vec<&str> = body
		.as_array()
		.expect("chapters array")
		.iter()
		.filter_map(|c| c["key"].as_str())
		.collect();
	assert_eq!(keys, ["old/broken/ch-1", "old/broken/ch-2"], "{keys:?}");

	// source_json 带上解析到的 breakingChangeVersion
	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let row = body
		.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == KEY)
		.expect("source row")
		.clone();
	assert_eq!(row["breakingChangeVersion"], 2, "{row}");

	repo.serve.abort();
}

/// 跨 breaking 但新包无 MigrationHandler：标 migrationRequired，库行一个字节不动
#[tokio::test]
async fn breaking_update_without_handler_flags_migration() {
	let app = test_app().await;
	let repo = serve_repo().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	stage(&repo, KEY, 1, &rekey_wasm, None);
	let repo_id = add_repo(&app.app, &repo.url).await;
	let source_id = install(&app.app, repo_id, KEY).await;
	let (manga1, _) = register_library_manga(&app.app, source_id, "Rekey One").await;
	let (ch1, _) = chapter_by_number(&app.app, manga1, 1.0).await;
	set_progress(&app.app, ch1, 5, true).await;

	// v2 换无 handler 的 mutation 载荷，跨 breaking
	let mutation_wasm = std::fs::read(mutation_wasm_path()).expect("mutation wasm");
	stage(&repo, KEY, 2, &mutation_wasm, Some(2));
	refresh(&app.app, repo_id).await;
	let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{source_id}/update"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], true, "{body}");

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "old/manga-1", "{body}");
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let ch1_after = body
		.as_array()
		.expect("chapters array")
		.iter()
		.find(|c| c["id"] == ch1)
		.expect("ch1 present")
		.clone();
	assert_eq!(ch1_after["key"], "old/manga-1/ch-1", "{ch1_after}");
	assert_eq!(ch1_after["lastPageRead"], 5, "{ch1_after}");

	repo.serve.abort();
}

/// 不跨 breaking（无字段 / 库内无条目）就是一次普通更新
#[tokio::test]
async fn non_breaking_update_is_plain_update() {
	let app = test_app().await;
	let repo = serve_repo().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	stage(&repo, KEY, 1, &rekey_wasm, None);
	let repo_id = add_repo(&app.app, &repo.url).await;
	let source_id = install(&app.app, repo_id, KEY).await;
	let (manga1, _) = register_library_manga(&app.app, source_id, "Rekey One").await;

	// v2 不带 breakingChangeVersion：正常更新不触发迁移
	stage(&repo, KEY, 2, &rekey_wasm, None);
	refresh(&app.app, repo_id).await;
	let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{source_id}/update"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "old/manga-1", "{body}");

	// v3 跨 breaking 但库里没有条目：无 handler 也不标 migrationRequired
	let mutation_wasm = std::fs::read(mutation_wasm_path()).expect("mutation wasm");
	stage(&repo, "en.empty", 1, &rekey_wasm, None);
	repo.mock.set_index(json!([
		{"id": KEY, "name": KEY, "version": 2, "downloadURL": format!("files/{KEY}-v2.aix"), "languages": ["en"]},
		{"id": "en.empty", "name": "en.empty", "version": 1, "downloadURL": "files/en.empty-v1.aix", "languages": ["en"]},
	]));
	repo.mock.set_file("files/en.empty-v1.aix", aix_bytes(&rekey_wasm, "en.empty", 1, None));
	refresh(&app.app, repo_id).await;
	let empty_id = install(&app.app, repo_id, "en.empty").await;
	stage(&repo, "en.empty", 2, &mutation_wasm, Some(2));
	repo.mock.set_index(json!([
		{"id": KEY, "name": KEY, "version": 2, "downloadURL": format!("files/{KEY}-v2.aix"), "languages": ["en"]},
		{"id": "en.empty", "name": "en.empty", "version": 2, "downloadURL": "files/en.empty-v2.aix", "languages": ["en"]},
	]));
	repo.mock.set_file("files/en.empty-v2.aix", aix_bytes(&mutation_wasm, "en.empty", 2, Some(2)));
	refresh(&app.app, repo_id).await;
	let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{empty_id}/update"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert_eq!(body["migrationRequired"], false, "{body}");

	repo.serve.abort();
}

/// 重键目标已被别的行占用：占不动的行留旧 key 进 warnings，其余照迁，请求不炸
#[tokio::test]
async fn rekey_target_collision_warns_and_keeps_old_key() {
	let app = test_app().await;
	let repo = serve_repo().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	stage(&repo, KEY, 1, &rekey_wasm, None);
	let repo_id = add_repo(&app.app, &repo.url).await;
	let source_id = install(&app.app, repo_id, KEY).await;
	let (manga1, _) = register_library_manga(&app.app, source_id, "Rekey One").await;
	let (manga2, _) = register_library_manga(&app.app, source_id, "Rekey Two").await;

	// 抢先占用 old/manga-1 的重键目标 key
	let pool = connect_db(&app.db_path).await;
	sqlx::query("INSERT INTO manga (source_id, key, title) VALUES (?, 'new/manga-1', 'squatter')")
		.bind(source_id)
		.execute(&pool)
		.await
		.expect("insert squatter");

	stage(&repo, KEY, 2, &rekey_wasm, Some(2));
	refresh(&app.app, repo_id).await;
	let (status, body) = request(&app.app, "POST", &format!("/api/v1/sources/{source_id}/update"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let warnings = body["migrationWarnings"].as_array().expect("warnings array").clone();
	assert_eq!(warnings.len(), 1, "{warnings:?}");
	assert!(warnings[0].as_str().unwrap_or_default().contains("new/manga-1"), "{warnings:?}");

	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga1}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "old/manga-1", "colliding row keeps old key: {body}");
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga2}"), None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "new/manga-2", "{body}");

	repo.serve.abort();
}

/// update-all 按源带出 migrationRequired 与重键 warnings
#[tokio::test]
async fn update_all_reports_migration_per_source() {
	let app = test_app().await;
	let repo = serve_repo().await;
	let rekey_wasm = std::fs::read(rekey_wasm_path()).expect("rekey wasm");
	let mutation_wasm = std::fs::read(mutation_wasm_path()).expect("mutation wasm");

	// 两个源都装 v1 并入库条目：en.rekey-source v2 带 handler，en.plain v2 无 handler
	stage(&repo, KEY, 1, &rekey_wasm, None);
	repo.mock.set_index(json!([
		{"id": KEY, "name": KEY, "version": 1, "downloadURL": format!("files/{KEY}-v1.aix"), "languages": ["en"]},
		{"id": "en.plain", "name": "en.plain", "version": 1, "downloadURL": "files/en.plain-v1.aix", "languages": ["en"]},
	]));
	repo.mock.set_file("files/en.plain-v1.aix", aix_bytes(&rekey_wasm, "en.plain", 1, None));
	let repo_id = add_repo(&app.app, &repo.url).await;
	let rekey_id = install(&app.app, repo_id, KEY).await;
	let plain_id = install(&app.app, repo_id, "en.plain").await;
	register_library_manga(&app.app, rekey_id, "Broken Three").await;
	register_library_manga(&app.app, plain_id, "Rekey One").await;

	repo.mock.set_index(json!([
		{"id": KEY, "name": KEY, "version": 2, "downloadURL": format!("files/{KEY}-v2.aix"), "languages": ["en"]},
		{"id": "en.plain", "name": "en.plain", "version": 2, "downloadURL": "files/en.plain-v2.aix", "languages": ["en"]},
	]));
	repo.mock.set_file(&format!("files/{KEY}-v2.aix"), aix_bytes(&rekey_wasm, KEY, 2, Some(2)));
	repo.mock.set_file("files/en.plain-v2.aix", aix_bytes(&mutation_wasm, "en.plain", 2, Some(2)));
	refresh(&app.app, repo_id).await;

	let (status, body) = request(&app.app, "POST", "/api/v1/source-repos/update-all", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["updated"], json!([KEY, "en.plain"]), "{body}");
	assert_eq!(body["migrationRequired"], json!(["en.plain"]), "{body}");
	let warnings = body["migrationWarnings"].as_array().expect("warnings array").clone();
	assert!(!warnings.is_empty(), "broken branch warns: {warnings:?}");
	assert!(
		warnings
			.iter()
			.all(|w| w["key"] == KEY && w["message"].as_str().unwrap_or_default().contains("old/broken")),
		"{warnings:?}"
	);

	repo.serve.abort();
}
