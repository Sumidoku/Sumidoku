//! Scheduled automatic backups: writes on the backup.* setting cadence, prunes by retention count, catches up overdue runs at startup.
//! The document is built with api::backup::build_backup_doc, the same shape as the GET /api/v1/backups/document export.

use crate::api::backup::build_backup_doc;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Wake granularity of the due check; intervals are in hours, so minute-level drift is imperceptible
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

pub struct BackupScheduler {
	pool: SqlitePool,
	config: Config,
	tick: Duration,
}

impl BackupScheduler {
	pub fn new(pool: SqlitePool, config: Config) -> Arc<Self> {
		Self::with_tick_interval(pool, config, CHECK_INTERVAL)
	}

	/// Tests inject a short tick to drive run(); production uses new's default granularity
	pub fn with_tick_interval(pool: SqlitePool, config: Config, tick: Duration) -> Arc<Self> {
		Arc::new(Self { pool, config, tick })
	}

	pub async fn run(self: Arc<Self>) {
		let mut ticker = tokio::time::interval(self.tick);
		loop {
			// interval's first tick completes immediately, so the startup overdue check is the first iteration;
			// settings are re-read every round, so runtime config changes take effect next cycle
			ticker.tick().await;
			if let Err(e) = self.run_if_due().await {
				tracing::warn!("scheduled backup failed: {e:?}");
			}
		}
	}

	/// Runs only when due: enabled and (never backed up or a full interval has passed since last_run)
	pub async fn run_if_due(&self) -> ApiResult<bool> {
		let settings = BackupSettings::load(&self.pool, &self.config).await?;
		if !settings.enabled {
			return Ok(false);
		}
		let due = match last_run(&self.pool).await? {
			None => true,
			Some(at) => unix_now() - at >= (settings.interval_hours * 3600) as i64,
		};
		if !due {
			return Ok(false);
		}
		self.write_backup(&settings).await?;
		Ok(true)
	}

	/// Unconditionally produces one backup and prunes (manual trigger and tests; last_run is updated too)
	pub async fn run_once(&self) -> ApiResult<PathBuf> {
		let settings = BackupSettings::load(&self.pool, &self.config).await?;
		self.write_backup(&settings).await
	}

	async fn write_backup(&self, settings: &BackupSettings) -> ApiResult<PathBuf> {
		// Scheduled backup is a server-side full export built with the built-in admin identity
		let doc = build_backup_doc(&self.pool).await?;
		let body = serde_json::to_vec(&doc)?;
		tokio::fs::create_dir_all(&settings.dir)
			.await
			.map_err(|e| ApiError::internal(format!("create backup dir: {e}")))?;
		let name = format!(
			"sumidoku-backup-{}.json",
			chrono::Utc::now().format("%Y%m%d-%H%M%S")
		);
		let path = settings.dir.join(&name);
		// Write a temp file then rename: a crash midway must not leave a half-written .json the list would treat as restorable
		let tmp = settings.dir.join(format!("{name}.tmp"));
		tokio::fs::write(&tmp, &body)
			.await
			.map_err(|e| ApiError::internal(format!("write backup: {e}")))?;
		tokio::fs::rename(&tmp, &path)
			.await
			.map_err(|e| ApiError::internal(format!("rename backup: {e}")))?;
		set_last_run(&self.pool, unix_now()).await?;
		// A prune failure must not sink this backup; retry next cycle
		if let Err(e) = prune(&settings.dir, settings.retention_count).await {
			tracing::warn!("backup prune failed: {e:?}");
		}
		Ok(path)
	}
}

struct BackupSettings {
	enabled: bool,
	interval_hours: u64,
	retention_count: u32,
	dir: PathBuf,
}

impl BackupSettings {
	/// env (config layer) first, then the setting table, then built-in defaults
	async fn load(pool: &SqlitePool, config: &Config) -> ApiResult<Self> {
		let enabled = match config.backup_enabled {
			Some(v) => v,
			None => setting_json(pool, "backup.enabled")
				.await?
				.and_then(|v| v.as_bool())
				.unwrap_or(false),
		};
		let interval_hours = match config.backup_interval_hours {
			Some(v) => v,
			None => setting_json(pool, "backup.interval_hours")
				.await?
				.and_then(|v| v.as_u64())
				.unwrap_or(24),
		};
		let retention_count = match config.backup_retention_count {
			Some(v) => v,
			None => setting_json(pool, "backup.retention_count")
				.await?
				.and_then(|v| v.as_u64())
				.map(|v| u32::try_from(v).unwrap_or(u32::MAX))
				.unwrap_or(10),
		};
		Ok(Self {
			enabled,
			// 0 would judge every check overdue, writing one backup per minute; the cap prevents u64 multiply overflow
			interval_hours: interval_hours.clamp(1, 8760),
			retention_count,
			dir: backups_dir(pool, config).await?,
		})
	}
}

/// Single-key read; the generic PUT accepts any JSON, so a wrong type is treated as unset
async fn setting_json(pool: &SqlitePool, key: &str) -> ApiResult<Option<Value>> {
	let row: Option<(String,)> = sqlx::query_as("SELECT value FROM setting WHERE key = ?")
		.bind(key)
		.fetch_optional(pool)
		.await?;
	Ok(row.and_then(|(raw,)| serde_json::from_str(&raw).ok()))
}

async fn last_run(pool: &SqlitePool) -> ApiResult<Option<i64>> {
	Ok(setting_json(pool, "backup.last_run")
		.await?
		.and_then(|v| v.as_i64()))
}

async fn set_last_run(pool: &SqlitePool, at: i64) -> ApiResult<()> {
	sqlx::query(
		"INSERT INTO setting (key, value) VALUES ('backup.last_run', ?)
		 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
	)
	.bind(serde_json::to_string(&at)?)
	.execute(pool)
	.await?;
	Ok(())
}

/// Backup destination: env (config.backup_dir) > setting backup.dir > default <config_dir>/backups
pub async fn backups_dir(pool: &SqlitePool, config: &Config) -> ApiResult<PathBuf> {
	if let Some(dir) = &config.backup_dir {
		return Ok(dir.clone());
	}
	if let Some(dir) = setting_json(pool, "backup.dir")
		.await?
		.and_then(|v| v.as_str().map(str::to_string))
		&& !dir.trim().is_empty()
	{
		return Ok(PathBuf::from(dir));
	}
	Ok(config.default_backups_dir())
}

/// Accepts only sumidoku-backup-<yyyymmdd>-<HHMMSS>.json (UTC); returns the timestamp from the file name.
/// The restore endpoint uses it as a file-name whitelist: the pattern anchors both ends, so ".." can never match
pub fn parse_backup_name(name: &str) -> Option<i64> {
	let stamp = name.strip_prefix("sumidoku-backup-")?.strip_suffix(".json")?;
	let dt = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").ok()?;
	Some(dt.and_utc().timestamp())
}

#[derive(Debug)]
pub struct BackupFileInfo {
	pub name: String,
	pub size: u64,
	pub created_at: i64,
}

/// Backups in the directory matching the naming format, newest first; a missing directory reads as an empty list
pub async fn list_backup_files(dir: &Path) -> ApiResult<Vec<BackupFileInfo>> {
	let mut read_dir = match tokio::fs::read_dir(dir).await {
		Ok(read_dir) => read_dir,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(e) => return Err(ApiError::internal(format!("read backup dir: {e}"))),
	};
	let mut out = Vec::new();
	while let Some(entry) = read_dir
		.next_entry()
		.await
		.map_err(|e| ApiError::internal(format!("read backup dir: {e}")))?
	{
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		let Some(created_at) = parse_backup_name(name) else {
			continue;
		};
		let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
		out.push(BackupFileInfo {
			name: name.to_string(),
			size,
			created_at,
		});
	}
	// With the fixed format, name order is time order
	out.sort_by(|a, b| b.name.cmp(&a.name));
	Ok(out)
}

/// Keeps the newest max(keep, 1) files and deletes older ones: the newest is never deleted, even with keep=0.
/// A single delete failure is only logged, not fatal; returns the number deleted
pub async fn prune(dir: &Path, keep: u32) -> ApiResult<usize> {
	let files = list_backup_files(dir).await?;
	let keep = (keep as usize).max(1);
	let mut removed = 0;
	for file in files.into_iter().skip(keep) {
		match tokio::fs::remove_file(dir.join(&file.name)).await {
			Ok(()) => removed += 1,
			Err(e) => tracing::warn!(file = file.name, "backup prune remove failed: {e}"),
		}
	}
	Ok(removed)
}
