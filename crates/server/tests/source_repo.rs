mod common;

use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ---- 本地 mock 源仓库：index.json 可随时换版，.aix 走 /files/* ----

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

	fn set_raw_index(&self, raw: &[u8]) {
		self.state.write().unwrap().index = raw.to_vec();
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

/// 绑定后立即放开端口：连接必 refused，扮演不可达 repo
async fn dead_url() -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	drop(listener);
	format!("http://{addr}/index.json")
}

fn aix_bytes(key: &str, name: &str, version: i64, langs: &[&str], rating: i64) -> Vec<u8> {
	let langs: Vec<String> = langs.iter().map(|l| format!("\"{l}\"")).collect();
	let manifest = format!(
		r#"{{"info": {{"id": "{key}", "name": "{name}", "version": {version}, "languages": [{}], "contentRating": {rating}}}}}"#,
		langs.join(", ")
	);
	let wasm = std::fs::read(fixture_dir().join("main.wasm")).expect("fixture wasm");
	let mut cursor = std::io::Cursor::new(Vec::new());
	{
		let mut writer = zip::ZipWriter::new(&mut cursor);
		let options = zip::write::SimpleFileOptions::default()
			.compression_method(zip::CompressionMethod::Deflated);
		writer.start_file("Payload/source.json", options).unwrap();
		std::io::Write::write_all(&mut writer, manifest.as_bytes()).unwrap();
		writer.start_file("Payload/main.wasm", options).unwrap();
		std::io::Write::write_all(&mut writer, &wasm).unwrap();
		writer.finish().unwrap();
	}
	cursor.into_inner()
}

fn repo_entry(key: &str, name: &str, version: i64, langs: &[&str], rating: i64) -> Value {
	json!({
		"id": key,
		"name": name,
		"version": version,
		"iconURL": format!("files/{key}-v{version}.png"),
		"downloadURL": format!("files/{key}-v{version}.aix"),
		"languages": langs,
		"contentRating": rating,
	})
}

/// 三源仓库：en.repo-source（主线装卸对象）、en.other、zh.nsfw（rating 2）
fn stage_repo(repo: &RepoServer, repo_version: i64, other_version: i64) {
	repo.mock.set_index(json!([
		repo_entry("en.repo-source", "Repo Source", repo_version, &["en"], 0),
		repo_entry("en.other", "Other Source", other_version, &["en"], 1),
		repo_entry("zh.nsfw", "Nsfw Source", 2, &["zh"], 2),
	]));
	repo.mock.set_file(
		&format!("files/en.repo-source-v{repo_version}.aix"),
		aix_bytes("en.repo-source", "Repo Source", repo_version, &["en"], 0),
	);
	repo.mock.set_file(
		&format!("files/en.other-v{other_version}.aix"),
		aix_bytes("en.other", "Other Source", other_version, &["en"], 1),
	);
}

async fn add_repo(app: &axum::Router, url: &str) -> (StatusCode, Value) {
	request(app, "POST", "/api/v1/source-repos", Some(json!({"url": url}))).await
}

async fn browse(app: &axum::Router, query: &str) -> Vec<Value> {
	let (status, body) = request(app, "GET", &format!("/api/v1/source-repos/sources{query}"), None).await;
	assert_eq!(status, StatusCode::OK, "browse {query}: {body}");
	body.as_array().expect("entries array").clone()
}

async fn source_by_key(app: &axum::Router, key: &str) -> Value {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["key"] == key)
		.cloned()
		.unwrap_or_else(|| panic!("source {key} installed: {body}"))
}

#[tokio::test]
async fn repo_lifecycle_install_and_update() {
	let app = test_app().await;
	let repo = serve_repo().await;
	stage_repo(&repo, 1, 3);

	// 添加 repo：拉 index 落缓存
	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let repo_id = body["id"].as_i64().expect("repo id");
	assert_eq!(body["name"], "Mock Repo");
	assert!(body["fetchedAt"].as_i64().unwrap_or(0) > 0, "{body}");
	assert!(body["fetchError"].is_null(), "{body}");

	// 列表里有它
	let (status, body) = request(&app.app, "GET", "/api/v1/source-repos", None).await;
	assert_eq!(status, StatusCode::OK);
	let repos = body.as_array().expect("repos array");
	assert_eq!(repos.len(), 1);
	assert_eq!(repos[0]["sourceCount"], 3, "{repos:?}");

	// 跨 repo 浏览：三项都带安装状态
	let entries = browse(&app.app, "").await;
	assert_eq!(entries.len(), 3, "{entries:?}");
	let main_entry = entries.iter().find(|e| e["key"] == "en.repo-source").expect("entry");
	assert_eq!(main_entry["version"], 1);
	assert_eq!(main_entry["repoId"], repo_id);
	assert!(main_entry["installedVersion"].is_null(), "{main_entry}");
	assert!(main_entry["updateVersion"].is_null(), "{main_entry}");
	assert!(
		main_entry["iconUrl"].as_str().unwrap_or_default().ends_with("files/en.repo-source-v1.png"),
		"iconUrl resolved against repo url: {main_entry}"
	);

	// 过滤：语言（可多选）/ 名字 / 内容分级（Aidoku 三档）
	assert_eq!(browse(&app.app, "?lang=en").await.len(), 2);
	assert_eq!(browse(&app.app, "?lang=zh").await.len(), 1);
	assert_eq!(browse(&app.app, "?lang=en,zh").await.len(), 3);
	let found = browse(&app.app, "?query=other").await;
	assert_eq!(found.len(), 1);
	assert_eq!(found[0]["key"], "en.other");
	assert_eq!(browse(&app.app, "?contentRating=0,1").await.len(), 2);
	assert_eq!(browse(&app.app, "?contentRating=2").await.len(), 1);
	assert_eq!(browse(&app.app, "?contentRating=0,1,2").await.len(), 3);
	assert_eq!(browse(&app.app, "?contentRating=").await.len(), 0);

	// 一键安装：.aix 落 sources 目录并即时可搜
	let (status, body) = request(
		&app.app,
		"POST",
		"/api/v1/source-repos/install",
		Some(json!({"repoId": repo_id, "sourceKey": "en.repo-source"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["key"], "en.repo-source");
	assert_eq!(body["version"], 1);
	let source_id = body["id"].as_i64().expect("source id");
	assert!(
		app.tmp.path().join("sources/en.repo-source.aix").is_file(),
		"aix landed in sources dir"
	);
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "installed source loads: {body}");
	assert!(!body["entries"].as_array().unwrap().is_empty());

	// 浏览反映已装：installedVersion=1，暂无更新
	let entries = browse(&app.app, "").await;
	let main_entry = entries.iter().find(|e| e["key"] == "en.repo-source").expect("entry");
	assert_eq!(main_entry["installedId"], source_id);
	assert_eq!(main_entry["installedVersion"], 1);
	assert!(main_entry["updateVersion"].is_null(), "{main_entry}");

	// repo 侧发布 v2，refresh 后检测到更新
	stage_repo(&repo, 2, 3);
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/source-repos/{repo_id}/refresh"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let entries = browse(&app.app, "").await;
	let main_entry = entries.iter().find(|e| e["key"] == "en.repo-source").expect("entry");
	assert_eq!(main_entry["version"], 2);
	assert_eq!(main_entry["installedVersion"], 1);
	assert_eq!(main_entry["updateVersion"], 2, "{main_entry}");

	// 已装源列表同样带更新标识
	let installed = source_by_key(&app.app, "en.repo-source").await;
	assert_eq!(installed["updateVersion"], 2, "{installed}");

	// 单源更新：版本前进且新包即时生效
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/update"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["version"], 2, "{body}");
	assert!(body["updateVersion"].is_null(), "{body}");
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"page": 1})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "updated source reloads: {body}");

	// 再更新一次：没有新版本
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/sources/{source_id}/update"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "no_update");

	// update-all：en.other 装 v3 → 发 v4 → 一键全更
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/source-repos/install",
		Some(json!({"repoId": repo_id, "sourceKey": "en.other"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	stage_repo(&repo, 2, 4);
	let (status, _) = request(
		&app.app,
		"POST",
		&format!("/api/v1/source-repos/{repo_id}/refresh"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app.app, "POST", "/api/v1/source-repos/update-all", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	assert_eq!(body["updated"], json!(["en.other"]), "{body}");
	assert_eq!(body["failed"], json!([]), "{body}");
	let other = source_by_key(&app.app, "en.other").await;
	assert_eq!(other["version"], 4);

	repo.serve.abort();
}

#[tokio::test]
async fn unreachable_repo_keeps_cache_and_installed_list() {
	let app = test_app().await;
	// 目录扫描装 example-source，与 repo 装的源并列验证互不影响
	let (status, _) = request(&app.app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let repo = serve_repo().await;
	stage_repo(&repo, 1, 3);
	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::OK);
	let repo_id = body["id"].as_i64().unwrap();
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/source-repos/install",
		Some(json!({"repoId": repo_id, "sourceKey": "en.repo-source"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// repo 宕机：refresh 失败但缓存照用、已装源列表照出
	repo.serve.abort();
	let (status, body) = request(
		&app.app,
		"POST",
		&format!("/api/v1/source-repos/{repo_id}/refresh"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");

	let (status, body) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let keys: Vec<&str> = body
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|s| s["key"].as_str())
		.collect();
	assert!(keys.contains(&"en.example-source") && keys.contains(&"en.repo-source"), "{keys:?}");

	let entries = browse(&app.app, "").await;
	assert_eq!(entries.len(), 3, "cached index still served: {entries:?}");

	// repo 行的错误状态对外可见
	let (status, body) = request(&app.app, "GET", "/api/v1/source-repos", None).await;
	assert_eq!(status, StatusCode::OK);
	assert!(body[0]["fetchError"].is_string(), "{body}");

	// 添加时即不可达：不入库
	let (status, body) = add_repo(&app.app, &dead_url().await).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
	let (status, body) = request(&app.app, "GET", "/api/v1/source-repos", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().unwrap().len(), 1, "dead repo not stored: {body}");
}

#[tokio::test]
async fn bad_index_json_rejected() {
	let app = test_app().await;
	let repo = serve_repo().await;
	repo.mock.set_raw_index(b"{not json");
	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");

	// 合法 JSON 但缺 sources 数组同样拒收
	repo.mock.set_raw_index(br#"{"name": "empty"}"#);
	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");

	let (status, body) = request(&app.app, "GET", "/api/v1/source-repos", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body.as_array().unwrap().len(), 0, "{body}");
	// 已装源列表不受影响
	let (status, _) = request(&app.app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn add_repo_validation() {
	let app = test_app().await;

	for bad in ["ftp://example.com/index.json", "not a url", "file:///etc/index.json"] {
		let (status, body) = add_repo(&app.app, bad).await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
	}

	let repo = serve_repo().await;
	stage_repo(&repo, 1, 3);
	let (status, _) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::CONFLICT, "{body}");
	assert_eq!(body["error"]["code"], "duplicate");

	// 删除后浏览为空；404 删不存在
	let (status, body) = request(&app.app, "GET", "/api/v1/source-repos", None).await;
	assert_eq!(status, StatusCode::OK);
	let repo_id = body[0]["id"].as_i64().unwrap();
	let (status, _) = request(
		&app.app,
		"DELETE",
		&format!("/api/v1/source-repos/{repo_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NO_CONTENT);
	assert!(browse(&app.app, "").await.is_empty());
	let (status, _) = request(
		&app.app,
		"DELETE",
		&format!("/api/v1/source-repos/{repo_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	repo.serve.abort();
}

#[tokio::test]
async fn legacy_index_fields_resolve() {
	let app = test_app().await;
	let repo = serve_repo().await;
	// Aidoku 旧格式：lang/nsfw 单数 + file/icon 相对路径
	repo.mock.set_index(json!([{
		"id": "en.legacy",
		"name": "Legacy Source",
		"version": 7,
		"lang": "en",
		"nsfw": 1,
		"file": "en.legacy-v7.aix",
		"icon": "en.legacy.png"
	}]));
	repo.mock.set_file(
		"sources/en.legacy-v7.aix",
		aix_bytes("en.legacy", "Legacy Source", 7, &["en"], 1),
	);

	let (status, body) = add_repo(&app.app, &repo.url).await;
	assert_eq!(status, StatusCode::OK, "{body}");
	let repo_id = body["id"].as_i64().unwrap();

	let entries = browse(&app.app, "?lang=en").await;
	assert_eq!(entries.len(), 1, "{entries:?}");
	assert_eq!(entries[0]["contentRating"], 1);
	assert!(
		entries[0]["iconUrl"].as_str().unwrap().ends_with("/icons/en.legacy.png"),
		"{entries:?}"
	);

	let (status, body) = request(
		&app.app,
		"POST",
		"/api/v1/source-repos/install",
		Some(json!({"repoId": repo_id, "sourceKey": "en.legacy"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "legacy file resolves under sources/: {body}");
	assert_eq!(body["version"], 7);

	repo.serve.abort();
}
