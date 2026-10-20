//! 定时自动备份：按 setting 表 backup.* 周期落盘、按保留数裁剪，启动补跑 overdue。
//! 文档构造复用 api::backup::build_backup_doc，与 GET /api/v1/backup 下载同构。

use crate::api::backup::build_backup_doc;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::model::unix_now;
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 到期检查的唤醒粒度；interval 以小时计，分钟级漂移无感
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

	/// 测试注入短周期驱动 run()；生产走 new 的默认粒度
	pub fn with_tick_interval(pool: SqlitePool, config: Config, tick: Duration) -> Arc<Self> {
		Arc::new(Self { pool, config, tick })
	}

	pub async fn run(self: Arc<Self>) {
		let mut ticker = tokio::time::interval(self.tick);
		loop {
			// interval 首 tick 立即完成，启动时的 overdue 检查就是第一次迭代；
			// 每轮重读 setting，运行期改配置下个周期即生效
			ticker.tick().await;
			if let Err(e) = self.run_if_due().await {
				tracing::warn!("scheduled backup failed: {e:?}");
			}
		}
	}

	/// 到点才跑：enabled 且（从未备份过 或 距 last_run 已满一个周期）
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

	/// 无条件产出一份备份并裁剪（手动触发与测试用；last_run 随之更新）
	pub async fn run_once(&self) -> ApiResult<PathBuf> {
		let settings = BackupSettings::load(&self.pool, &self.config).await?;
		self.write_backup(&settings).await
	}

	async fn write_backup(&self, settings: &BackupSettings) -> ApiResult<PathBuf> {
		// 定时备份是服务端全量导出：固定内置 admin 身份构造，按请求者角色裁剪
		// （reader 只有 self-data）只管 GET /api/v1/backup，管不到这里
		let doc = build_backup_doc(&self.pool, crate::USER_ID, true).await?;
		let body = serde_json::to_vec(&doc)?;
		tokio::fs::create_dir_all(&settings.dir)
			.await
			.map_err(|e| ApiError::internal(format!("create backup dir: {e}")))?;
		let name = format!(
			"sumidoku-backup-{}.json",
			chrono::Utc::now().format("%Y%m%d-%H%M%S")
		);
		let path = settings.dir.join(&name);
		// 先写临时文件再改名：中途崩溃不留半截 .json 被列表当成可恢复备份
		let tmp = settings.dir.join(format!("{name}.tmp"));
		tokio::fs::write(&tmp, &body)
			.await
			.map_err(|e| ApiError::internal(format!("write backup: {e}")))?;
		tokio::fs::rename(&tmp, &path)
			.await
			.map_err(|e| ApiError::internal(format!("rename backup: {e}")))?;
		set_last_run(&self.pool, unix_now()).await?;
		// 裁剪失败不拖垮本次备份，留到下个周期再试
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
	/// env（config 层）优先，其次 setting 表，最后内置默认
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
			// 0 会把每次检查都判成 overdue 等于每分钟写一份；上限防 u64 乘法溢出
			interval_hours: interval_hours.clamp(1, 8760),
			retention_count,
			dir: backups_dir(pool, config).await?,
		})
	}
}

/// 单键读取；通用 PUT 允许任意 JSON，类型不合时按未设置处理
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

/// 备份落点：env（config.backup_dir）> setting backup.dir > 默认 <config_dir>/backups
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

/// 只认 sumidoku-backup-<yyyymmdd>-<HHMMSS>.json（UTC）；返回文件名里的时间戳。
/// 恢复端点拿它当文件名白名单：pattern 锚定首尾，「..」之类永远匹配不上
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

/// 目录里符合命名格式的备份，新到旧排序；目录不存在按空列表
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
	// 固定格式下名字序即时间序
	out.sort_by(|a, b| b.name.cmp(&a.name));
	Ok(out)
}

/// 保留最新 max(keep, 1) 份，删掉更老的：最新一个永不删，keep=0 也保底。
/// 单个删除失败只记日志不中断；返回删除份数
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
