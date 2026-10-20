mod common;

use aidoku::{Chapter, Manga, MangaStatus, Viewer};
use axum::Router;
use axum::http::StatusCode;
use common::{connect_db, register_example_manga, request, test_app};
use sumidoku_server::model::{chapter_from_row, manga_from_row};
use sumidoku_server::source::{chapter_row, manga_row, reconcile_chapters, upsert_manga};

async fn scan_example_source(app: &Router) -> i64 {
	let (status, _) = request(app, "POST", "/api/v1/sources/scan", None).await;
	assert_eq!(status, StatusCode::OK);
	let (status, body) = request(app, "GET", "/api/v1/sources", None).await;
	assert_eq!(status, StatusCode::OK);
	body[0]["id"].as_i64().expect("source id")
}

#[tokio::test]
async fn upsert_persists_viewer_and_chapter_thumbnail() {
	let app = test_app().await;
	let source_id = scan_example_source(&app.app).await;
	let pool = connect_db(&app.db_path).await;
	let mut conn = pool.acquire().await.expect("connection");

	let manga = Manga {
		key: "m1".into(),
		title: "Manga 1".into(),
		status: MangaStatus::Ongoing,
		viewer: Viewer::RightToLeft,
		..Manga::default()
	};
	let manga_id = upsert_manga(&mut conn, source_id, &manga, true, true, true)
		.await
		.expect("upsert manga");
	let chapters = vec![
		Chapter {
			key: "c1".into(),
			chapter_number: Some(1.0),
			thumbnail: Some("https://img.example/c1.png".into()),
			..Chapter::default()
		},
		Chapter {
			key: "c2".into(),
			chapter_number: Some(2.0),
			..Chapter::default()
		},
	];
	reconcile_chapters(&mut conn, manga_id, &chapters, &[])
		.await
		.expect("reconcile chapters");

	let row = manga_row(&pool, manga_id).await.expect("manga row");
	assert_eq!(manga_from_row(&row).viewer, Viewer::RightToLeft);

	// 重复 upsert 走 ON CONFLICT 分支，viewer 更新同样要落库
	let updated = Manga {
		viewer: Viewer::Webtoon,
		..manga.clone()
	};
	upsert_manga(&mut conn, source_id, &updated, true, true, false)
		.await
		.expect("re-upsert manga");
	let row = manga_row(&pool, manga_id).await.expect("manga row");
	assert_eq!(manga_from_row(&row).viewer, Viewer::Webtoon);

	let ids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM chapter WHERE manga_id = ? ORDER BY key")
		.bind(manga_id)
		.fetch_all(&pool)
		.await
		.expect("chapter ids");
	assert_eq!(ids.len(), 2);
	let first = chapter_row(&pool, ids[0].0).await.expect("chapter row");
	assert_eq!(
		chapter_from_row(&first).thumbnail.as_deref(),
		Some("https://img.example/c1.png")
	);
	let second = chapter_row(&pool, ids[1].0).await.expect("chapter row");
	assert_eq!(chapter_from_row(&second).thumbnail, None);
}

#[tokio::test]
async fn single_language_source_fills_missing_chapter_language() {
	// example fixture 恰好声明一种语言（["en"]）
	let app = test_app().await;
	let source_id = scan_example_source(&app.app).await;
	let pool = connect_db(&app.db_path).await;
	let mut conn = pool.acquire().await.expect("connection");

	let manga = Manga {
		key: "m1".into(),
		title: "Manga 1".into(),
		..Manga::default()
	};
	let manga_id = upsert_manga(&mut conn, source_id, &manga, true, true, true)
		.await
		.expect("upsert manga");
	let chapters = vec![
		Chapter {
			key: "c1".into(),
			..Chapter::default()
		},
		Chapter {
			key: "c2".into(),
			language: Some("".into()),
			..Chapter::default()
		},
		Chapter {
			key: "c3".into(),
			language: Some("ja".into()),
			..Chapter::default()
		},
	];
	reconcile_chapters(&mut conn, manga_id, &chapters, &[])
		.await
		.expect("reconcile chapters");

	let rows: Vec<(String, Option<String>)> =
		sqlx::query_as("SELECT key, language FROM chapter WHERE manga_id = ? ORDER BY key")
			.bind(manga_id)
			.fetch_all(&pool)
			.await
			.expect("chapter languages");
	assert_eq!(
		rows,
		vec![
			("c1".to_string(), Some("en".to_string())),
			("c2".to_string(), Some("en".to_string())),
			("c3".to_string(), Some("ja".to_string())),
		]
	);
}

#[tokio::test]
async fn multi_language_source_keeps_chapter_language_missing() {
	let app = test_app().await;
	let pool = connect_db(&app.db_path).await;
	let wasm = common::fixture_dir().join("main.wasm");
	let result = sqlx::query(
		"INSERT INTO source (key, name, version, languages, wasm_path)
		 VALUES ('multi.example-source', 'Multi', 1, '[\"en\",\"ja\"]', ?)",
	)
	.bind(wasm.to_string_lossy().to_string())
	.execute(&pool)
	.await
	.expect("insert source");
	let source_id = result.last_insert_rowid();
	let mut conn = pool.acquire().await.expect("connection");

	let manga = Manga {
		key: "m1".into(),
		title: "Manga 1".into(),
		..Manga::default()
	};
	let manga_id = upsert_manga(&mut conn, source_id, &manga, true, true, true)
		.await
		.expect("upsert manga");
	let chapters = vec![Chapter {
		key: "c1".into(),
		..Chapter::default()
	}];
	reconcile_chapters(&mut conn, manga_id, &chapters, &[])
		.await
		.expect("reconcile chapters");

	let row = chapter_row(
		&pool,
		sqlx::query_scalar("SELECT id FROM chapter WHERE manga_id = ?")
			.bind(manga_id)
			.fetch_one(&pool)
			.await
			.expect("chapter id"),
	)
	.await
	.expect("chapter row");
	assert_eq!(chapter_from_row(&row).language, None);
}

#[tokio::test]
async fn chapter_language_filled_on_register_via_api() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let (status, body) = request(&app.app, "GET", &format!("/api/v1/manga/{manga_id}/chapters"), None).await;
	assert_eq!(status, StatusCode::OK);
	let chapters = body.as_array().expect("chapters");
	assert_eq!(chapters.len(), 8);
	for chapter in chapters {
		assert_eq!(
			chapter["language"],
			serde_json::json!("en"),
			"chapter {} language",
			chapter["key"]
		);
	}
}
