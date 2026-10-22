mod common;

use axum::http::StatusCode;
use common::{base_config, connect_db, register_example_manga, request, request_full, test_app};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::path::Path;
use std::time::Duration;
use sumidoku_server::backup::{BackupScheduler, backups_dir, list_backup_files, prune};
use sumidoku_server::{build_app, db};

async fn set_setting(pool: &SqlitePool, key: &str, value: Value) {
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES (?, ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(key)
	.bind(serde_json::to_string(&value).expect("encode setting"))
	.execute(pool)
	.await
	.expect("set setting");
}

async fn last_run(pool: &SqlitePool) -> Option<i64> {
	let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = 'backup.last_run'")
		.fetch_optional(pool)
		.await
		.expect("last_run query");
	row.and_then(|(raw,)| serde_json::from_str(&raw).ok())
}

/// Waits until at least min backup files appear in the directory
async fn wait_for_backups(dir: &Path, min: usize, timeout: Duration) -> Vec<String> {
	let start = std::time::Instant::now();
	loop {
		let names: Vec<String> = list_backup_files(dir)
			.await
			.expect("list backups")
			.into_iter()
			.map(|f| f.name)
			.collect();
		if names.len() >= min {
			return names;
		}
		assert!(
			start.elapsed() < timeout,
			"timed out waiting for {min} backup(s) in {dir:?}, have {names:?}"
		);
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}

/// Waits for last_run to be persisted: the timestamp is recorded only after the file is written, so keying on it leaves no race window across restarts
async fn wait_for_last_run(pool: &SqlitePool, timeout: Duration) -> i64 {
	let start = std::time::Instant::now();
	loop {
		if let Some(at) = last_run(pool).await {
			return at;
		}
		assert!(start.elapsed() < timeout, "timed out waiting for backup.last_run");
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}

#[tokio::test]
async fn scheduled_backup_runs_and_respects_due() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	let pool = db::create_pool(&config.database_url).await.expect("pool");
	set_setting(&pool, "backup.enabled", json!(true)).await;
	set_setting(&pool, "backup.interval_hours", json!(1)).await;
	let dir = config.default_backups_dir();

	// enabled with no last_run (never backed up) → overdue already at the first startup check
	let scheduler =
		BackupScheduler::with_tick_interval(pool.clone(), config.clone(), Duration::from_millis(50));
	let task = tokio::spawn(scheduler.run());
	let names = wait_for_backups(&dir, 1, Duration::from_secs(5)).await;
	task.abort();

	let bytes = std::fs::read(dir.join(&names[0])).expect("read backup");
	let doc: Value = serde_json::from_slice(&bytes).expect("backup json");
	assert_eq!(doc["version"], 1);
	assert!(doc["createdAt"].is_i64());
	for key in [
		"sources",
		"library",
		"categories",
		"progress",
		"settings",
		"sourceSettings",
	] {
		assert!(doc.get(key).is_some(), "missing section {key}");
	}
	assert!(last_run(&pool).await.expect("last_run recorded") > 0);

	// a new scheduler instance on the same DB (restart semantics): last_run was just written, so it doesn't run again before the interval
	let restarted = BackupScheduler::new(pool.clone(), config.clone());
	assert!(!restarted.run_if_due().await.expect("due check"));
	assert_eq!(list_backup_files(&dir).await.expect("list").len(), 1);

	// with the switch off, it doesn't run even when overdue
	set_setting(&pool, "backup.enabled", json!(false)).await;
	set_setting(&pool, "backup.last_run", json!(0)).await;
	assert!(!restarted.run_if_due().await.expect("due check"));

	// re-enabled and overdue → immediately makes one, updating last_run
	set_setting(&pool, "backup.enabled", json!(true)).await;
	assert!(restarted.run_if_due().await.expect("due check"));
	assert!(last_run(&pool).await.expect("last_run") > 0);
	// a same-second name collision overwrites the same-named file, so the count is 1 or 2
	let files = list_backup_files(&dir).await.expect("list");
	assert!((1..=2).contains(&files.len()), "files: {files:?}");
}

#[tokio::test]
async fn scheduled_backup_matches_export_and_lists() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	// take the HTTP export first: once run_once lands, the setting table gains backup.last_run and the snapshots are no longer identical
	let (status, _, body) = request_full(&app.app, "GET", "/api/v1/backups/document", None).await;
	assert_eq!(status, StatusCode::OK);
	let mut from_http: Value = serde_json::from_slice(&body).expect("http json");

	let pool = connect_db(&app.db_path).await;
	let scheduler = BackupScheduler::new(pool, base_config(&app.tmp));
	let path = scheduler.run_once().await.expect("run once");
	let bytes = std::fs::read(&path).expect("read backup file");
	let mut from_file: Value = serde_json::from_slice(&bytes).expect("file json");

	// createdAt is each document's own construction time; with it removed the two documents match field by field
	from_file.as_object_mut().expect("object").remove("createdAt");
	from_http.as_object_mut().expect("object").remove("createdAt");
	assert_eq!(from_file, from_http);
	assert_eq!(from_file["library"].as_array().expect("library").len(), 1);

	let (status, body) = request(&app.app, "GET", "/api/v1/backups/files", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(
		body["dir"],
		json!(app.tmp.path().join("backups").display().to_string())
	);
	let files = body["files"].as_array().expect("files");
	assert_eq!(files.len(), 1);
	assert_eq!(
		files[0]["name"],
		path.file_name().expect("name").to_str().expect("utf8")
	);
	assert_eq!(files[0]["size"].as_u64().expect("size"), bytes.len() as u64);
	assert!(files[0]["createdAt"].as_i64().expect("createdAt") > 0);
}

#[tokio::test]
async fn prune_keeps_newest_and_ignores_foreign_files() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let dir = tmp.path().join("backups");
	std::fs::create_dir_all(&dir).expect("mkdir");
	let ours = [
		"sumidoku-backup-20260101-090000.json",
		"sumidoku-backup-20260102-090000.json",
		"sumidoku-backup-20260103-090000.json",
		"sumidoku-backup-20260104-090000.json",
		"sumidoku-backup-20260105-090000.json",
	];
	for name in ours {
		std::fs::write(dir.join(name), b"{}").expect("write");
	}
	// files not matching the naming format are out of pruning scope (including close-shaped ones with an invalid date)
	let foreign = [
		"notes.txt",
		"sumidoku-backup-garbage.json",
		"sumidoku-backup-20261301-090000.json",
	];
	for name in foreign {
		std::fs::write(dir.join(name), b"x").expect("write foreign");
	}

	let removed = prune(&dir, 2).await.expect("prune");
	assert_eq!(removed, 3);
	let remaining: Vec<String> = list_backup_files(&dir)
		.await
		.expect("list")
		.into_iter()
		.map(|f| f.name)
		.collect();
	assert_eq!(remaining, [ours[4], ours[3]]);
	for name in foreign {
		assert!(dir.join(name).exists(), "foreign file {name} removed");
	}

	// keep=0 still keeps the newest one
	let removed = prune(&dir, 0).await.expect("prune");
	assert_eq!(removed, 1);
	assert!(dir.join(ours[4]).exists(), "newest backup must survive");

	assert_eq!(prune(&dir.join("missing"), 2).await.expect("prune missing"), 0);
}

#[tokio::test]
async fn restore_from_listed_backup() {
	let app = test_app().await;
	let manga_id = register_example_manga(&app.app).await;
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/library",
		Some(json!({"mangaId": manga_id})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"backup.test": "on"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let pool = connect_db(&app.db_path).await;
	let scheduler = BackupScheduler::new(pool, base_config(&app.tmp));
	let path = scheduler.run_once().await.expect("run once");
	let name = path
		.file_name()
		.expect("name")
		.to_str()
		.expect("utf8")
		.to_string();

	// change state after the backup lands; the restore must pull it back
	let (status, _) = request(
		&app.app,
		"PUT",
		"/api/v1/settings",
		Some(json!({"backup.test": "off"})),
	)
	.await;
	assert_eq!(status, StatusCode::OK);

	let (status, body) = request(&app.app, "GET", "/api/v1/backups/files", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["files"][0]["name"], json!(name));

	let (status, body) = request(
		&app.app,
		"POST",
		"/api/v1/backups/files/restore",
		Some(json!({"name": name})),
	)
	.await;
	assert_eq!(status, StatusCode::OK, "restore failed: {body}");
	assert_eq!(body["warnings"], json!([]));
	assert_eq!(body["restored"]["library"], 1);
	assert!(body["restored"]["settings"].as_i64().expect("settings") >= 1);
	let (_, settings) = request(&app.app, "GET", "/api/v1/settings", None).await;
	assert_eq!(settings["backup.test"], "on");

	// filename whitelist: anything outside the pattern 400s, including path escapes and invalid dates
	for bad in [
		"..",
		"../secret",
		"foo.json",
		"sumidoku-backup-20261301-090000.json",
		"sumidoku-backup-20260101-090000.json.bak",
		"",
	] {
		let (status, body) = request(
			&app.app,
			"POST",
			"/api/v1/backups/files/restore",
			Some(json!({"name": bad})),
		)
		.await;
		assert_eq!(status, StatusCode::BAD_REQUEST, "name {bad:?}: {body}");
		assert_eq!(body["error"]["code"], "bad_request");
	}
	// well-formed but nonexistent file → 404
	let (status, _) = request(
		&app.app,
		"POST",
		"/api/v1/backups/files/restore",
		Some(json!({"name": "sumidoku-backup-19990101-000000.json"})),
	)
	.await;
	assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn startup_runs_overdue_backup_and_survives_restart() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	// preset enabled + last_run far older than one interval → overdue at startup
	{
		let pool = db::create_pool(&config.database_url).await.expect("pool");
		set_setting(&pool, "backup.enabled", json!(true)).await;
		set_setting(&pool, "backup.interval_hours", json!(1)).await;
		set_setting(&pool, "backup.last_run", json!(0)).await;
		pool.close().await;
	}
	let dir = config.default_backups_dir();

	let app = build_app(config.clone()).await.expect("build app");
	let pool = connect_db(&tmp.path().join("test.db")).await;
	wait_for_last_run(&pool, Duration::from_secs(5)).await;
	assert_eq!(wait_for_backups(&dir, 1, Duration::from_secs(1)).await.len(), 1);
	pool.close().await;
	drop(app);

	// restart: last_run is already in the DB, the new instance doesn't re-run; the file persists and shows in the list
	let app = build_app(config).await.expect("rebuild app");
	tokio::time::sleep(Duration::from_secs(1)).await;
	let files = list_backup_files(&dir).await.expect("list");
	assert_eq!(files.len(), 1, "restart produced an extra backup");
	let (status, body) = request(&app, "GET", "/api/v1/backups/files", None).await;
	assert_eq!(status, StatusCode::OK);
	assert_eq!(body["files"].as_array().expect("files").len(), 1);
}

#[tokio::test]
async fn backup_dir_resolution_prefers_env_then_setting() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let config = base_config(&tmp);
	let pool = db::create_pool(&config.database_url).await.expect("pool");

	assert_eq!(
		backups_dir(&pool, &config).await.expect("dir"),
		tmp.path().join("backups")
	);

	let custom = tmp.path().join("custom-backups");
	set_setting(&pool, "backup.dir", json!(custom.display().to_string())).await;
	assert_eq!(backups_dir(&pool, &config).await.expect("dir"), custom);

	// env (config layer) takes precedence over the setting table
	let mut override_config = config.clone();
	let env_dir = tmp.path().join("env-backups");
	override_config.backup_dir = Some(env_dir.clone());
	assert_eq!(backups_dir(&pool, &override_config).await.expect("dir"), env_dir);
}
