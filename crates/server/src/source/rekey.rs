//! breaking-change key 迁移：源更新跨过 manifest `config.breakingChangeVersion` 时，
//! 有自迁移能力的源逐 key 调 `handle_key_migration` 原地重键（行 id 不动，
//! 进度/分类/下载标记全保留）；无能力的源不碰库行，只标 `migrationRequired`
//! 让界面引导同源 flow A（对齐 Aidoku AppDelegate.handleSourceMigration 与
//! SourceManager 的 installedVersion < breakingChangeVersion 检测）

use crate::error::ApiResult;
use crate::state::AppState;
use aidoku_wasm::SourceHandle;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct RekeyReport {
	/// 跨 breaking 且源无自迁移能力：需要用户走 flow A 同源迁移
	pub required: bool,
	pub warnings: Vec<String>,
}

/// install_from_repo 成功后调用；old_version 是更新前已装版本。
/// 迁移引导按触发用户的库判定：只在别人的库里的条目不向当前用户推 flow A
pub async fn migrate_after_update(
	state: &Arc<AppState>,
	source_id: i64,
	old_version: i64,
	user_id: i64,
) -> ApiResult<RekeyReport> {
	let row: Option<(i64, i64)> =
		sqlx::query_as("SELECT version, breaking_change_version FROM source WHERE id = ?")
			.bind(source_id)
			.fetch_optional(&state.pool)
			.await?;
	let Some((new_version, breaking)) = row else {
		return Ok(RekeyReport::default());
	};
	// Aidoku 只判 old < breaking；breaking <= new_version 防御把版本填乱的 manifest
	if !(old_version < breaking && breaking <= new_version) {
		return Ok(RekeyReport::default());
	}
	let (manga_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_one(&state.pool)
		.await?;
	if manga_count == 0 {
		return Ok(RekeyReport::default());
	}
	let handle = state.get_handle(source_id).await?;
	let features = handle.features();
	if !features.migration {
		// 对齐 Aidoku forceMigrate：库里没条目就不引导（AppDelegate 里
		// sourceManga.isEmpty 不弹迁移页）
		let (in_library,): (i64,) = sqlx::query_as(
			"SELECT COUNT(*) FROM manga m
			 JOIN manga_user mu ON mu.manga_id = m.id AND mu.user_id = ? AND mu.in_library = 1
			 WHERE m.source_id = ?",
		)
		.bind(user_id)
		.bind(source_id)
		.fetch_one(&state.pool)
		.await?;
		return Ok(RekeyReport {
			required: in_library > 0,
			warnings: Vec::new(),
		});
	}
	rekey_in_place(state, source_id, &handle, features.notification).await
}

/// 先把全部 key 映射收齐（wasm 调用可能走网络，不进事务），再单事务改写；
/// chapter 迁移传旧 manga key，与 Aidoku 一致
async fn rekey_in_place(
	state: &Arc<AppState>,
	source_id: i64,
	handle: &SourceHandle,
	notification: bool,
) -> ApiResult<RekeyReport> {
	let manga_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, key FROM manga WHERE source_id = ?")
		.bind(source_id)
		.fetch_all(&state.pool)
		.await?;
	let chapter_rows: Vec<(i64, String, String)> = sqlx::query_as(
		"SELECT c.id, c.key, m.key FROM chapter c JOIN manga m ON m.id = c.manga_id WHERE m.source_id = ?",
	)
	.bind(source_id)
	.fetch_all(&state.pool)
	.await?;

	if notification
		&& let Err(e) = handle.handle_notification("system.startMigration".into()).await
	{
		tracing::warn!(source_id, "startMigration notification failed: {e}");
	}

	let mut warnings = Vec::new();
	let mut manga_map: Vec<(i64, String, String)> = Vec::new();
	for (id, key) in &manga_rows {
		match handle.handle_key_migration(0, key.clone(), None).await {
			Ok(new_key) if new_key.is_empty() => {
				warnings.push(format!("manga {key}: source returned an empty key, left unchanged"));
			}
			Ok(new_key) if new_key != *key => manga_map.push((*id, key.clone(), new_key)),
			Ok(_) => {}
			Err(e) => warnings.push(format!("manga {key}: migration call failed: {e}")),
		}
	}
	let mut chapter_map: Vec<(i64, String, String)> = Vec::new();
	for (id, key, manga_key) in &chapter_rows {
		match handle.handle_key_migration(1, manga_key.clone(), Some(key.clone())).await {
			Ok(new_key) if new_key.is_empty() => {
				warnings.push(format!("chapter {key}: source returned an empty key, left unchanged"));
			}
			Ok(new_key) if new_key != *key => chapter_map.push((*id, key.clone(), new_key)),
			Ok(_) => {}
			Err(e) => warnings.push(format!("chapter {key}: migration call failed: {e}")),
		}
	}

	if notification
		&& let Err(e) = handle.handle_notification("system.endMigration".into()).await
	{
		tracing::warn!(source_id, "endMigration notification failed: {e}");
	}

	let mut tx = state.pool.begin().await?;
	apply_rekeys(&mut tx, "manga", manga_map, &mut warnings).await?;
	apply_rekeys(&mut tx, "chapter", chapter_map, &mut warnings).await?;
	tx.commit().await?;
	Ok(RekeyReport {
		required: false,
		warnings,
	})
}

/// 撞 UNIQUE 的行挂起重试：占位行可能自己也在被重键，fixpoint 收敛后仍撞的
/// 保留旧 key 进 warnings。SQLite 单语句冲突只回滚该语句，事务继续有效
async fn apply_rekeys(
	tx: &mut sqlx::SqliteConnection,
	table: &str,
	pending: Vec<(i64, String, String)>,
	warnings: &mut Vec<String>,
) -> ApiResult<()> {
	let sql = format!("UPDATE {table} SET key = ? WHERE id = ?");
	let mut pending = pending;
	loop {
		let mut progressed = false;
		let mut blocked = Vec::new();
		for (id, old_key, new_key) in pending {
			match sqlx::query(&sql).bind(&new_key).bind(id).execute(&mut *tx).await {
				Ok(_) => progressed = true,
				Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
					blocked.push((id, old_key, new_key));
				}
				Err(e) => return Err(e.into()),
			}
		}
		if blocked.is_empty() {
			return Ok(());
		}
		if !progressed {
			for (_, old_key, new_key) in blocked {
				warnings.push(format!(
					"{table} {old_key}: target key {new_key} already exists, left unchanged"
				));
			}
			return Ok(());
		}
		pending = blocked;
	}
}
