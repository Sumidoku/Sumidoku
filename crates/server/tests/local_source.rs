//! 内置本地源契约：迁移落库不可删 / 实扫去重与跳过规则 / 打开才落库 /
//! 页字节与页序（图片章与 CBZ 一致）/ 章节号排序 / ComicInfo 映射 /
//! 封面回退链 / 刷新重扫 / 失败目录保护 / local_dir 配置覆盖 /
//! 浏览搜索臂（实扫 + 子串过滤，单源与跨源聚合同形）/ listings 404 unsupported
mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{TestApp, base_config, request, request_full, source_by_key, test_app};
use serde_json::json;
use std::time::{Duration, Instant};
use sumidoku_server::build_app;
use sumidoku_server::config::Config;

const COMIC_INFO_ALPHA: &str = r#"<?xml version="1.0"?>
<ComicInfo xmlns:ty="http://www.w3.org/2001/XMLSchema">
  <Series>Alpha Series</Series>
  <Summary>alpha desc</Summary>
  <Writer>w1, w2</Writer>
  <Penciller>p1</Penciller>
  <Genre>Action, Adventure</Genre>
  <ty:PublishingStatusTachiyomi>Completed</ty:PublishingStatusTachiyomi>
</ComicInfo>"#;

const COMIC_INFO_DELTA: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Delta Series</Series>
  <PublishingStatusTachiyomi>Ongoing</PublishingStatusTachiyomi>
</ComicInfo>"#;

/// 3000x4000 真图：降采样分支要真解码
fn big_png() -> Vec<u8> {
	let image = image::RgbImage::from_fn(3000, 4000, |x, y| {
		image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 255) as u8])
	});
	let mut out = Vec::new();
	image::DynamicImage::ImageRgb8(image)
		.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
		.expect("encode png");
	out
}

fn write_page(dir: &std::path::Path, name: &str, bytes: &[u8]) {
	std::fs::create_dir_all(dir).expect("mkdir chapter");
	std::fs::write(dir.join(name), bytes).expect("write page");
}

fn write_cbz(path: &std::path::Path, entries: &[(&str, &[u8])]) {
	let file = std::fs::File::create(path).expect("create cbz");
	let mut writer = zip::ZipWriter::new(file);
	for (name, bytes) in entries {
		writer
			.start_file(*name, zip::write::SimpleFileOptions::default())
			.expect("start entry");
		std::io::Write::write_all(&mut writer, bytes).expect("write entry");
	}
	writer.finish().expect("finish cbz");
}

/// 造好 local 树并起 app；local_dir 用 <config_dir>/local 缺省派生
async fn local_app() -> TestApp {
	local_app_with(|_| {}).await
}

async fn local_app_with(configure: impl FnOnce(&mut Config)) -> TestApp {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_local_tree(&tmp.path().join("local"));
	let mut config = base_config(&tmp);
	configure(&mut config);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	TestApp { app, tmp, db_path }
}

/// Alpha 的 ch 2 单页大图，降采样断言要用；返回磁盘上的原图字节
fn seed_local_tree(root: &std::path::Path) {
	let alpha = root.join("Alpha");
	std::fs::create_dir_all(&alpha).expect("mkdir alpha");
	std::fs::write(alpha.join("ComicInfo.xml"), COMIC_INFO_ALPHA).expect("write comicinfo");
	write_page(&alpha.join("ch 1"), "p2.png", b"alpha-p2");
	write_page(&alpha.join("ch 1"), "P1.png", b"alpha-p1");
	write_page(&alpha.join("ch 1"), "p10.png", b"alpha-p10");
	write_page(&alpha.join("ch 1"), ".hidden.png", b"hidden");
	write_page(&alpha.join("ch 2"), "big.png", &big_png());
	write_page(&alpha.join("第3话"), "001.png", b"alpha-3-1");
	write_cbz(
		&alpha.join("ch 4.cbz"),
		&[
			("b.png", b"alpha-4-b"),
			("a.png", b"alpha-4-a"),
			("ComicInfo.xml", b"<ComicInfo/>"),
			("__MACOSX/x.png", b"junk"),
		],
	);
	write_page(&alpha.join("ch 10"), "1.png", b"alpha-10-1");
	write_page(&alpha.join("extras"), "1.png", b"alpha-extras-1");

	let beta = root.join("Beta");
	write_page(&beta.join("ch 1"), "1.png", b"beta-1-1");
	std::fs::write(beta.join("cover.jpg"), b"beta-sidecar-cover").expect("write sidecar");

	let gamma = root.join("Gamma");
	write_page(&gamma.join("ch 1"), "g1.png", b"gamma-1-g1");
	write_page(&gamma.join("ch 1"), "g2.png", b"gamma-1-g2");
	write_page(&gamma.join("ch 2"), "h1.png", b"gamma-2-h1");

	let delta = root.join("Delta");
	std::fs::create_dir_all(&delta).expect("mkdir delta");
	write_cbz(
		&delta.join("ch 1.cbz"),
		&[("z.png", b"delta-1-z"), ("m.png", b"delta-1-m"), ("ComicInfo.xml", COMIC_INFO_DELTA.as_bytes())],
	);

	std::fs::create_dir_all(root.join("Epsilon")).expect("mkdir epsilon");

	// 跳过规则：点开头目录、@eaDir、根目录散文件都不算漫画
	std::fs::create_dir_all(root.join(".hidden")).expect("mkdir hidden");
	std::fs::create_dir_all(root.join("@eaDir")).expect("mkdir eaDir");
	std::fs::write(root.join("note.txt"), b"not a manga").expect("write note");
}

async fn local_source_id(app: &Router) -> i64 {
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	source_by_key(&body, "local.local")["id"]
		.as_i64()
		.expect("local source id")
}

async fn register_local(app: &Router, manga_key: &str) -> i64 {
	let source_id = local_source_id(app).await;
	let (status, body) = request(
		app,
		"POST",
		"/api/v1/manga",
		Some(json!({"sourceId": source_id, "mangaKey": manga_key})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "register {manga_key}: {body}");
	body["id"].as_i64().expect("manga id")
}

async fn chapters_of(app: &Router, manga_id: i64) -> Vec<(i64, String, Option<f64>, String)> {
	let (status, body) = request(app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	body.as_array()
		.expect("chapters array")
		.iter()
		.map(|c| {
			(
				c["id"].as_i64().expect("chapter id"),
				c["key"].as_str().expect("chapter key").to_string(),
				c["chapter_number"].as_f64(),
				c["title"].as_str().unwrap_or_default().to_string(),
			)
		})
		.collect()
}

async fn chapter_id_of(app: &Router, manga_id: i64, key: &str) -> i64 {
	chapters_of(app, manga_id)
		.await
		.into_iter()
		.find(|(_, k, _, _)| k == key)
		.map(|(id, _, _, _)| id)
		.unwrap_or_else(|| panic!("chapter {key}"))
}

#[tokio::test]
async fn migration_seeds_builtin_local_source() {
	let TestApp { app, tmp: _tmp, .. } = test_app().await;
	let (status, _) = request(&app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(&app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	let local = source_by_key(&body, "local.local");
	assert_eq!(local["name"], "Local");
	assert_eq!(local["kind"], "local");
	assert_eq!(source_by_key(&body, "en.example-source")["kind"], "aidoku");
	let local_id = local["id"].as_i64().expect("local id");

	// 内置源不可删
	let (status, body) = request(&app, "DELETE", &format!("/api/v1/sources/{local_id}"), None).await;
	assert_eq!(status, StatusCode::BAD_REQUEST);
	assert_eq!(body["error"]["code"], "bad_request");

	// 没有 wasm 实例：动态能力按端点约定 404 unsupported
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{local_id}/filters"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");
}

#[tokio::test]
async fn scan_lists_only_real_manga_dirs() {
	let tmp = tempfile::tempdir().expect("tempdir");
	seed_local_tree(&tmp.path().join("local"));
	let dirs = sumidoku_server::local::scan_manga_dirs(&tmp.path().join("local"))
		.await
		.expect("scan");
	assert_eq!(dirs, ["Alpha", "Beta", "Delta", "Epsilon", "Gamma"]);
}

#[tokio::test]
async fn register_persists_scan_metadata_and_chapter_order() {
	let TestApp { app, tmp: _tmp, db_path: _ } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Alpha Series");
	assert_eq!(body["authors"], json!(["w1", "w2"]));
	assert_eq!(body["artists"], json!(["p1"]));
	assert_eq!(body["description"], "alpha desc");
	assert_eq!(body["tags"], json!(["Action", "Adventure"]));
	assert_eq!(body["status"], "Completed");

	// 章节号降序，未解析出的 extras 沉底
	let chapters = chapters_of(&app, manga_id).await;
	let keys: Vec<&str> = chapters.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["ch 10", "ch 4.cbz", "第3话", "ch 2", "ch 1", "extras"]);
	let numbers: Vec<Option<f64>> = chapters.iter().map(|(_, _, n, _)| *n).collect();
	assert_eq!(
		numbers,
		[Some(10.0), Some(4.0), Some(3.0), Some(2.0), Some(1.0), None]
	);
	// 压缩包章展示名去扩展名
	assert_eq!(chapters[1].3, "ch 4");
}

#[tokio::test]
async fn image_chapter_pages_natural_order_and_bytes() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 1").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["P1.png", "p2.png", "p10.png"]);

	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/1"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, b"alpha-p2");
	let (status, _) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/9"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);

	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let page_count = body
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["key"] == "ch 1")
		.expect("ch 1 row")["pageCount"]
		.as_i64()
		.unwrap();
	assert_eq!(page_count, 3);
}

#[tokio::test]
async fn downsample_reuses_local_original_bytes() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 2").await;

	// 无 width：原图字节直通
	let original = std::fs::read(tmp.path().join("local/Alpha/ch 2/big.png")).expect("read big.png");
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/0"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, original);

	// ?width=512：与在线页同一条降采样管线（宽度 256 量化桶）
	let (status, headers, bytes) = request_full(
		&app,
		"GET",
		&format!("/api/v1/chapter/{chapter_id}/page/0?width=512"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	let image = image::ImageReader::new(std::io::Cursor::new(bytes))
		.with_guessed_format()
		.expect("format")
		.decode()
		.expect("decode");
	assert_eq!((image.width(), image.height()), (512, 682));
}

#[tokio::test]
async fn cbz_chapter_pages_and_bytes_match_image_chapter() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 4.cbz").await;

	let (status, body) = request(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/pages"), None).await;
	assert_eq!(status, StatusCode::OK);
	let entries: Vec<&str> = body
		.as_array()
		.expect("pages array")
		.iter()
		.map(|p| p["content"]["Local"].as_str().expect("local entry"))
		.collect();
	assert_eq!(entries, ["a.png", "b.png"]);

	for (index, want) in [(0, b"alpha-4-a"), (1, b"alpha-4-b")] {
		let (status, headers, bytes) =
			request_full(&app, "GET", &format!("/api/v1/chapter/{chapter_id}/page/{index}"), None).await;
		assert_eq!(status, StatusCode::OK);
		assert_eq!(headers["content-type"], "image/png");
		assert_eq!(bytes, want);
	}

	// OPDS-PSE 同一条 Local 分支
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/opds/v1.2/stream/{chapter_id}/0"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"alpha-4-a");
}

#[tokio::test]
async fn comic_info_extracted_from_cbz() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Delta").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["title"], "Delta Series");
	assert_eq!(body["status"], "Ongoing");
}

#[tokio::test]
async fn cover_sidecar_wins() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Beta").await;
	let (status, headers, bytes) =
		request_full(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/jpeg");
	assert_eq!(bytes, b"beta-sidecar-cover");
}

#[tokio::test]
async fn cover_falls_back_to_last_chapter_first_page() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	// 章节列表按号降序，末尾 = 最旧一章（ch 1），取其第一页
	let gamma_id = register_local(&app, "Gamma").await;
	let (status, _, bytes) = request_full(&app, "GET", &format!("/api/v1/manga/{gamma_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"gamma-1-g1");

	// 压缩包章同样回退：取包内自然序首个图片条目
	let delta_id = register_local(&app, "Delta").await;
	let (status, headers, bytes) = request_full(&app, "GET", &format!("/api/v1/manga/{delta_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(headers["content-type"], "image/png");
	assert_eq!(bytes, b"delta-1-m");

	// OPDS 封面同一缓存同一分支
	let (status, _, bytes) = request_full(
		&app,
		"GET",
		&format!("/opds/v1.2/cover/{delta_id}"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"delta-1-m");
}

#[tokio::test]
async fn cover_missing_answers_404() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Epsilon").await;
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn refresh_rescans_added_and_removed_chapters() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;

	write_page(&tmp.path().join("local/Alpha/ch 11"), "1.png", b"alpha-11-1");
	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<String> = chapters_of(&app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert_eq!(keys[0], "ch 11");

	std::fs::remove_dir_all(tmp.path().join("local/Alpha/ch 2")).expect("remove ch 2");
	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let keys: Vec<String> = chapters_of(&app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	assert!(!keys.iter().any(|k| k == "ch 2"), "真删除的章节应对账掉: {keys:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_manga_dir_never_deletes_rows() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let before = chapters_of(&app, manga_id).await;
	assert_eq!(before.len(), 6);

	// 模拟挂载抖断：漫画目录读不了时刷新报错，已有行绝不当已删除
	let alpha = tmp.path().join("local/Alpha");
	let mut perms = std::fs::metadata(&alpha).expect("metadata").permissions();
	use std::os::unix::fs::PermissionsExt;
	perms.set_mode(0o000);
	std::fs::set_permissions(&alpha, perms).expect("chmod 000");

	let (status, _) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

	let mut perms = std::fs::metadata(&alpha).expect("metadata").permissions();
	perms.set_mode(0o755);
	std::fs::set_permissions(&alpha, perms).expect("chmod 755");

	let after = chapters_of(&app, manga_id).await;
	assert_eq!(after.len(), before.len(), "读失败保护：章节行必须原样保留");
	let keys: Vec<&str> = after.iter().map(|(_, k, _, _)| k.as_str()).collect();
	assert_eq!(keys, ["ch 10", "ch 4.cbz", "第3话", "ch 2", "ch 1", "extras"]);
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_chapter_entry_never_deletes_rows() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let sigma = tmp.path().join("local/Sigma");
	write_page(&sigma.join("ch 1"), "1.png", b"sigma-1-1");
	write_page(&sigma.join("ch 3"), "1.png", b"sigma-3-1");
	// ch 2 是指向真实目录的 symlink：注册时 metadata 跟随目标成功，章节正常落库
	let target = tmp.path().join("target-ch2");
	write_page(&target, "1.png", b"sigma-2-1");
	std::os::unix::fs::symlink(&target, sigma.join("ch 2")).expect("symlink ch 2");
	let config = base_config(&tmp);
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Sigma").await;
	let before = chapters_of(&t.app, manga_id).await;
	assert_eq!(before.len(), 3);

	// 刷新前：ch 3 真删（对照组，照常对账掉）；ch 2 的目标删掉使 symlink 悬垂，
	// entry.metadata() 跟随 symlink 必失败 → 条目级 failed → protected 对账保护
	std::fs::remove_dir_all(sigma.join("ch 3")).expect("remove ch 3");
	std::fs::remove_dir_all(&target).expect("remove ch 2 target");

	// 条目级失败是降级不是报错：刷新照常 OK；被保护行不参与本轮重排（保留旧 source_order），
	// 断言按内容集合不按位置
	let (status, _) = request(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters?refresh=1"), None).await;
	assert_eq!(status, StatusCode::OK);
	let mut keys: Vec<String> = chapters_of(&t.app, manga_id)
		.await
		.into_iter()
		.map(|(_, k, _, _)| k)
		.collect();
	keys.sort();
	assert_eq!(keys, ["ch 1", "ch 2"], "悬垂 symlink 的章节行必须保留，真删的对账掉: {keys:?}");
}

#[tokio::test]
async fn invalid_or_unknown_keys_rejected() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;
	for (key, want) in [("../escape", 400), ("a/b", 400), ("Nope", 404)] {
		let (status, body) = request(
			&app,
			"POST",
			"/api/v1/manga",
			Some(json!({"sourceId": source_id, "mangaKey": key})),
		)
		.await;
		assert_eq!(status, want as u16, "{key}: {body}");
	}
}

#[tokio::test]
async fn local_dir_config_override() {
	let outside = tempfile::tempdir().expect("outside dir");
	seed_local_tree(&outside.path().join("manga"));
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut config = base_config(&tmp);
	config.local_dir = Some(outside.path().join("manga"));
	let db_path = tmp.path().join("test.db");
	let app = build_app(config).await.expect("build app");
	let t = TestApp { app, tmp, db_path };
	let manga_id = register_local(&t.app, "Beta").await;
	let (status, _, bytes) = request_full(&t.app, "GET", &format!("/api/v1/manga/{manga_id}/cover"), None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(bytes, b"beta-sidecar-cover");
	drop(outside);
}

#[tokio::test]
async fn shelving_progress_and_library_filters_behave_like_online() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let chapter_id = chapter_id_of(&app, manga_id, "ch 1").await;

	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app,
		"PATCH",
		&format!("/api/v1/chapter/{chapter_id}"),
		Some(json!({"lastPageRead": 2})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, body) = request(&app, "GET", "/api/v1/library", None).await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("library entries");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["id"], manga_id);

	let source_id = local_source_id(&app).await;
	let (status, body) = request(&app, "GET", "/api/v1/library/source-counts", None).await;
	assert_eq!(status, StatusCode::OK);
	let entry = body
		.as_array()
		.expect("counts array")
		.iter()
		.find(|e| e["sourceId"] == source_id)
		.expect("local count");
	assert_eq!(entry["count"], 1);

	// 已读过滤三态与在线源同一查询
	let (status, body) = request(&app, "GET", "/api/v1/library?filter=unread", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"].as_array().unwrap().len(), 1);
	let (status, body) = request(&app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let ch1 = body
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["key"] == "ch 1")
		.expect("ch 1");
	assert_eq!(ch1["lastPageRead"], 2);
	assert!(ch1["lastReadAt"].as_i64().is_some());
}

#[tokio::test]
async fn update_scheduler_rescans_local_library() {
	let TestApp { app, tmp, .. } = local_app().await;
	let manga_id = register_local(&app, "Alpha").await;
	let (status, _) = request(&app, "POST", "/api/v1/library", Some(json!({"mangaId": manga_id}))).await;
	assert_eq!(status, StatusCode::OK);

	write_page(&tmp.path().join("local/Alpha/ch 11"), "1.png", b"alpha-11-1");
	let (status, _) = request(&app, "POST", "/api/v1/update/fetch", None).await;
	assert_eq!(status, StatusCode::OK);
	// fetch 异步跑：直接等行为结果（新章出现），再对账调度器计数
	let deadline = Instant::now() + Duration::from_secs(10);
	loop {
		let keys: Vec<String> = chapters_of(&app, manga_id)
			.await
			.into_iter()
			.map(|(_, k, _, _)| k)
			.collect();
		if keys[0] == "ch 11" {
			break;
		}
		assert!(Instant::now() < deadline, "调度器重扫没看到新章: {keys:?}");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	let (status, body) = request(&app, "GET", "/api/v1/update/summary", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["failed"], 0, "{body}");
	assert_eq!(body["complete"], 1, "{body}");
}

#[tokio::test]
async fn browse_search_scans_dirs_and_filters() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// 空 query（缺省与纯空白）全量：浏览 = 实扫，条目同在线搜索形状（sourceId 附带）
	for body in [json!({}), json!({"query": "  "})] {
		let (status, body) = request(
			&app,
			"POST",
			&format!("/api/v1/sources/{source_id}/search"),
			Some(body),
		)
		.await;
		assert_eq!(status, StatusCode::OK, "{body}");
		assert_eq!(body["hasNextPage"], false);
		let entries = body["entries"].as_array().expect("entries array");
		let titles: Vec<&str> = entries.iter().map(|e| e["title"].as_str().unwrap()).collect();
		assert_eq!(titles, ["Alpha", "Beta", "Delta", "Epsilon", "Gamma"]);
		for entry in entries {
			assert_eq!(entry["sourceId"], source_id);
			assert_eq!(entry["key"], entry["title"]);
			assert!(entry["id"].is_null(), "未入库的条目不带内部 id: {entry}");
		}
	}

	// 子串过滤大小写不敏感
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "aLP"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let entries = body["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["title"], "Alpha");

	// 已入库的条目附内部 id，浏览页据此直接进详情页
	let manga_id = register_local(&app, "Beta").await;
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "beta"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"][0]["id"], manga_id);

	// 扫不到的目录名：空集不是错误
	let (status, body) = request(
		&app,
		"POST",
		&format!("/api/v1/sources/{source_id}/search"),
		Some(json!({"query": "nope"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["entries"], json!([]));
}

#[tokio::test]
async fn aggregate_search_includes_local_source() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// 跨源聚合选中本地源时出结果项，不是 unsupported 的 error 项
	let (status, body) = request(
		&app,
		"POST",
		"/api/v1/search",
		Some(json!({"query": "gamma", "sourceIds": [source_id]})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let sources = body["sources"].as_array().expect("sources array");
	assert_eq!(sources.len(), 1);
	let entry = &sources[0];
	assert_eq!(entry["sourceId"], source_id);
	assert!(entry.get("error").is_none(), "{entry}");
	assert_eq!(entry["hasNextPage"], false);
	let entries = entry["entries"].as_array().expect("entries array");
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0]["title"], "Gamma");
	assert_eq!(entries[0]["sourceId"], source_id);
}

#[tokio::test]
async fn local_listings_answer_unsupported() {
	let TestApp { app, tmp: _tmp, .. } = local_app().await;
	let source_id = local_source_id(&app).await;

	// 本地源没有 listings 概念：与缺该能力的 wasm 源同一契约，404 unsupported
	let (status, body) = request(&app, "GET", &format!("/api/v1/sources/{source_id}/listings"), None).await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");

	let (status, body) = request(
		&app,
		"GET",
		&format!("/api/v1/sources/{source_id}/listings/latest"),
		None,
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
	assert_eq!(body["error"]["code"], "unsupported");
}
